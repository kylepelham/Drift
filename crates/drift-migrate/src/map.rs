//! opencode rows to this engine's conversation shapes.

use crate::source::{OcMessage, OcPart, OcSession, OcTodo};
use drift_engine::session::types::{
    Ending, Message, MessageStatus, MessageWithParts, ModelRef, Part, PartRow, Role, Session, Todo, TodoStatus,
    ToolMetadata, ToolStatus, Usage, Visibility,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// opencode bookkeeping parts omitted from imported messages.
/// Token counts are already on messages; snapshots belong to a repository Drift does not use.
const DROPPED: [&str; 3] = ["step-start", "step-finish", "snapshot"];
/// Maximum retained display diff size; larger generated-file diffs are dropped from metadata.
const MAX_DIFF_BYTES: usize = 1_000_000;

/// Rebuilt undo records keyed by opencode part ID until attached to native calls.
pub(crate) type Records = HashMap<String, Value>;

/// Maps a listed conversation; the store keeps it hidden until its final page is written.
pub(crate) fn session(session: &OcSession, workspace_id: &str) -> Session {
    let model: Value = session
        .model
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or(Value::Null);

    Session {
        id: session.id.clone(),
        workspace_id: workspace_id.into(),
        parent_id: session.parent_id.clone(),
        visibility: if session.parent_id.is_some() {
            Visibility::Hidden
        } else {
            Visibility::Sibling
        },
        title: session.title.clone(),
        agent: session.agent.clone().unwrap_or_else(|| "build".into()),
        model: model_ref(&model["providerID"], &model["id"]),
        variant: model["variant"].as_str().map(String::from),
        created_at: session.created,
        updated_at: session.updated,
        archived_at: None,
        branch_cutoff: None,
        revert: None,
        auto_accept: false,
        running: false,
    }
}

/// Mints native message IDs in opencode's written order so later turns sort after the imported history.
/// Keeps earlier mappings for compaction boundaries that refer back to them.
#[derive(Default)]
pub(crate) struct Ids {
    last: i64,
    minted: HashMap<String, String>,
}

impl Ids {
    fn mint(&mut self, old: &str, created: i64) -> (String, i64) {
        self.last = (created << 12).max(self.last + 1);
        let id = native_id("msg", self.last, old);
        self.minted.insert(old.to_string(), id.clone());

        (id, self.last)
    }
}

/// Deterministic, so importing the same row twice mints the same id.
fn native_id(prefix: &str, stamp: i64, old: &str) -> String {
    let hash = Sha256::digest(old.as_bytes());

    format!(
        "{prefix}_{stamp:016x}{:02x}{:02x}{:02x}{:02x}",
        hash[0], hash[1], hash[2], hash[3]
    )
}

pub(crate) fn message(
    message: &OcMessage,
    parts: &[OcPart],
    session_id: &str,
    ids: &mut Ids,
    records: &Records,
) -> MessageWithParts {
    let (id, stamp) = ids.mint(&message.id, message.created);
    let data: Value = serde_json::from_str(&message.data).unwrap_or(Value::Null);
    let anthropic = data["providerID"] == "anthropic";

    let parts = parts
        .iter()
        .filter_map(|part| part_of(part, &ids.minted, anthropic))
        .enumerate()
        .map(|(index, (old, part))| PartRow {
            id: native_id("prt", stamp + index as i64, old),
            message_id: id.clone(),
            session_id: session_id.into(),
            provider_signature: None,
            part: with_record(part, records.get(old), &id),
        })
        .collect();

    MessageWithParts {
        info: info(&data, id, session_id, message.created),
        parts,
    }
}

/// Merges a call's undo record into its metadata and stamps it with its native message ID.
fn with_record(mut part: Part, record: Option<&Value>, message_id: &str) -> Part {
    let Some(record) = record else {
        return part;
    };
    let Part::ToolCall { metadata, .. } = &mut part else {
        return part;
    };

    // The undo record's keys go over the imported ones, stamped with the message they belong to.
    let imported = metadata.take().map(|metadata| *metadata).unwrap_or_default();
    let mut merged = imported.merged(Some(record.clone().into())).unwrap_or_default();
    merged.at = Some(message_id.into());
    *metadata = Some(Box::new(merged));

    part
}

fn info(data: &Value, id: String, session_id: &str, created: i64) -> Message {
    let user = data["role"] == "user";
    let created = data["time"]["created"].as_i64().unwrap_or(created);
    let model = if user {
        model_ref(&data["model"]["providerID"], &data["model"]["modelID"])
    } else {
        model_ref(&data["providerID"], &data["modelID"])
    };
    let (status, error, ending) = if user {
        (MessageStatus::Done, None, None)
    } else {
        outcome(data)
    };

    let tokens = &data["tokens"];
    let count = |value: &Value| value.as_u64().unwrap_or(0);

    Message {
        id,
        session_id: session_id.into(),
        role: if user { Role::User } else { Role::Assistant },
        status,
        model,
        agent: data["agent"].as_str().or(data["mode"].as_str()).map(String::from),
        usage: Usage {
            input: count(&tokens["input"]),
            output: count(&tokens["output"]) + count(&tokens["reasoning"]),
            cache_read: count(&tokens["cache"]["read"]),
            cache_write: count(&tokens["cache"]["write"]),
        },
        cost: data["cost"].as_f64().unwrap_or(0.0),
        error,
        created_at: created,
        finished_at: if user {
            Some(created)
        } else {
            data["time"]["completed"].as_i64()
        },
        summary: data["summary"] == true,
        generation_ms: None,
        account: None,
        ending,
    }
}

/// Maps an assistant reply's outcome; opencode reports aborts as errors and token limits as error or finish.
fn outcome(data: &Value) -> (MessageStatus, Option<String>, Option<Ending>) {
    let error = &data["error"];
    let message = error["data"]["message"]
        .as_str()
        .or(error["name"].as_str())
        .map(String::from);

    match error["name"].as_str() {
        Some("MessageAbortedError") => (MessageStatus::Aborted, message, None),
        Some("MessageOutputLengthError") => (MessageStatus::Done, message, Some(Ending::Length)),
        Some(_) => (MessageStatus::Error, message, None),
        None if data["finish"] == "length" => (MessageStatus::Done, None, Some(Ending::Length)),
        None if data["time"]["completed"].is_i64() => (MessageStatus::Done, None, None),
        None => (MessageStatus::Aborted, None, None),
    }
}

/// Keeps an unmapped part's original text in an unknown-part envelope.
/// Parts such as opencode's synthetic text remain unknown even when they resemble native parts.
pub(crate) fn kept(data: &str) -> Part {
    Part::Unknown {
        raw: serde_json::json!({ "type": "opencode", "data": data }).to_string(),
    }
}

/// Maps a part to its native shape, returning None for bookkeeping parts.
fn part_of<'a>(part: &'a OcPart, minted: &HashMap<String, String>, anthropic: bool) -> Option<(&'a str, Part)> {
    let unknown = || Some((part.id.as_str(), kept(&part.data)));
    let Ok(data) = serde_json::from_str::<Value>(&part.data) else {
        return unknown();
    };

    let kind = data["type"].as_str().unwrap_or_default();
    if DROPPED.contains(&kind) {
        return None;
    }

    let mapped = match kind {
        "text" if data["synthetic"] != true => data["text"].as_str().map(|text| Part::Text { text: text.into() }),
        "reasoning" => reasoning(&data, anthropic),
        "tool" => tool_call(&data),
        "file" => file(&data),
        "compaction" => Some(Part::Compaction {
            auto: data["auto"] == true,
            tail_from: data["tail_start_id"].as_str().and_then(|old| minted.get(old)).cloned(),
        }),
        _ => None,
    };

    match mapped {
        Some(mapped) => Some((part.id.as_str(), mapped)),
        None => unknown(),
    }
}

/// Preserves Claude's reasoning signature so the same model can read its signed thinking back.
/// Replay sends the signature only to the model that originally wrote it.
fn reasoning(data: &Value, anthropic: bool) -> Option<Part> {
    let text = data["text"].as_str()?.to_string();
    let signed = &data["metadata"]["anthropic"];
    let field = |key: &str| {
        signed[key]
            .as_str()
            .filter(|value| anthropic && !value.is_empty())
            .map(String::from)
    };

    Some(Part::Reasoning {
        text,
        signature: field("signature"),
        redacted: field("redactedData"),
    })
}

fn tool_call(data: &Value) -> Option<Part> {
    let state = &data["state"];
    let (status, output) = match state["status"].as_str() {
        Some("completed") => (ToolStatus::Done, state["output"].as_str()),
        Some("error") => (ToolStatus::Error, state["error"].as_str()),
        _ => (ToolStatus::Error, None),
    };
    let input = if state["input"].is_object() {
        state["input"].clone()
    } else {
        Value::Object(Default::default())
    };
    let metadata = Some(slim(data["tool"].as_str()?, state["metadata"].clone()))
        .filter(Value::is_object)
        .map(|metadata| Box::new(ToolMetadata::from(metadata)));

    Some(Part::ToolCall {
        call_id: data["callID"].as_str()?.into(),
        name: data["tool"].as_str()?.into(),
        input,
        status,
        title: state["title"].as_str().map(String::from),
        output: output.map(String::from),
        metadata,
        started_at: state["time"]["start"].as_i64(),
        finished_at: state["time"]["end"].as_i64(),
    })
}

/// Drops full-file patch copies, read display text and oversized diffs that no Drift view uses.
/// Renames a patched file's diff to the patch field used by its panel.
fn slim(tool: &str, mut metadata: Value) -> Value {
    let Some(fields) = metadata.as_object_mut() else {
        return metadata;
    };

    drop_oversized(fields, "diff");
    match tool {
        "read" => {
            fields.remove("display");
            fields.remove("preview");
        }
        "apply_patch" => {
            if let Some(files) = fields.get_mut("files").and_then(Value::as_array_mut) {
                files.iter_mut().for_each(slim_file);
            }
        }
        _ => {}
    }

    metadata
}

fn slim_file(file: &mut Value) {
    let Some(fields) = file.as_object_mut() else { return };

    fields.remove("before");
    fields.remove("after");
    if let Some(diff) = fields.remove("diff") {
        fields.entry("patch").or_insert(diff);
    }

    drop_oversized(fields, "patch");
}

fn drop_oversized(fields: &mut serde_json::Map<String, Value>, key: &str) {
    if fields
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|text| text.len() > MAX_DIFF_BYTES)
    {
        fields.remove(key);
    }
}

fn file(data: &Value) -> Option<Part> {
    let path = if data["source"]["type"] == "file" {
        data["source"]["path"].as_str().map(String::from)
    } else {
        None
    };

    Some(Part::File {
        mime: data["mime"].as_str()?.into(),
        name: data["filename"].as_str().unwrap_or_default().into(),
        url: data["url"].as_str()?.into(),
        path,
    })
}

pub(crate) fn todo(todo: &OcTodo) -> Option<Todo> {
    let status = serde_json::from_value::<TodoStatus>(Value::String(todo.status.clone())).ok()?;

    Some(Todo {
        content: todo.content.clone(),
        status,
        priority: todo.priority.clone(),
    })
}

fn model_ref(provider: &Value, model: &Value) -> Option<ModelRef> {
    Some(ModelRef {
        provider: provider.as_str()?.into(),
        model: model.as_str()?.into(),
    })
}
