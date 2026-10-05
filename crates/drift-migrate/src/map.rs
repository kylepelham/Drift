//! opencode rows to this engine's conversation shapes.

use std::collections::HashMap;

use drift_engine::session::types::{Ending, Message, MessageStatus, MessageWithParts, ModelRef, Part, PartRow, Role, Session, Todo, TodoStatus, ToolStatus, Usage, Visibility};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::source::{OcMessage, OcPart, OcSession, OcTodo};

/// opencode's per-step bookkeeping: its token counts are already on the message and its snapshots
/// name a shadow repository this engine never reads.
const DROPPED: [&str; 3] = ["step-start", "step-finish", "snapshot"];

/// Undo records built for imported calls (`crate::undo`), by opencode part id.
pub type Records = HashMap<String, Value>;

/// The conversation as listed; it stays hidden until its last page is written.
pub fn session(session: &OcSession, workspace_id: &str) -> Session {
    let model: Value = session.model.as_deref().and_then(|json| serde_json::from_str(json).ok()).unwrap_or(Value::Null);
    Session {
        id: session.id.clone(),
        workspace_id: workspace_id.into(),
        parent_id: session.parent_id.clone(),
        visibility: if session.parent_id.is_some() { Visibility::Hidden } else { Visibility::Sibling },
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

/// New message ids in this engine's form, minted as messages arrive in opencode's written order, so
/// later turns sort after them; earlier ones stay known for compaction boundaries that point back.
#[derive(Default)]
pub struct Ids {
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
    format!("{prefix}_{stamp:016x}{:02x}{:02x}{:02x}{:02x}", hash[0], hash[1], hash[2], hash[3])
}

pub fn message(message: &OcMessage, parts: &[OcPart], session_id: &str, ids: &mut Ids, records: &Records) -> MessageWithParts {
    let (id, stamp) = ids.mint(&message.id, message.created);
    let data: Value = serde_json::from_str(&message.data).unwrap_or(Value::Null);
    let anthropic = data["providerID"] == "anthropic";
    let parts = parts
        .iter()
        .filter_map(|part| part_of(part, &ids.minted, anthropic))
        .enumerate()
        .map(|(at, (old, part))| PartRow { id: native_id("prt", stamp + at as i64, old), message_id: id.clone(), session_id: session_id.into(), provider_signature: None, part: with_record(part, records.get(old), &id) })
        .collect();
    MessageWithParts { info: info(&data, id, session_id, message.created), parts }
}

/// A call's undo record goes in its metadata as a native call's does, stamped with its message.
fn with_record(part: Part, record: Option<&Value>, message_id: &str) -> Part {
    let (Part::ToolCall { call_id, name, input, status, title, output, metadata, started_at, finished_at }, Some(record)) = (part.clone(), record) else { return part };
    let mut metadata = metadata.unwrap_or_else(|| Value::Object(Default::default()));
    for (key, value) in record.as_object().into_iter().flatten() {
        metadata[key] = value.clone();
    }
    metadata["at"] = Value::String(message_id.into());
    Part::ToolCall { call_id, name, input, status, title, output, metadata: Some(metadata), started_at, finished_at }
}

fn info(data: &Value, id: String, session_id: &str, created: i64) -> Message {
    let user = data["role"] == "user";
    let created = data["time"]["created"].as_i64().unwrap_or(created);
    let model = if user { model_ref(&data["model"]["providerID"], &data["model"]["modelID"]) } else { model_ref(&data["providerID"], &data["modelID"]) };
    let (status, error, ending) = if user { (MessageStatus::Done, None, None) } else { outcome(data) };
    let tokens = &data["tokens"];
    let count = |value: &Value| value.as_u64().unwrap_or(0);
    Message {
        id,
        session_id: session_id.into(),
        role: if user { Role::User } else { Role::Assistant },
        status,
        model,
        agent: data["agent"].as_str().or(data["mode"].as_str()).map(String::from),
        usage: Usage { input: count(&tokens["input"]), output: count(&tokens["output"]) + count(&tokens["reasoning"]), cache_read: count(&tokens["cache"]["read"]), cache_write: count(&tokens["cache"]["write"]) },
        cost: data["cost"].as_f64().unwrap_or(0.0),
        error,
        created_at: created,
        finished_at: if user { Some(created) } else { data["time"]["completed"].as_i64() },
        summary: data["summary"] == true,
        ending,
    }
}

/// How an assistant reply ended: opencode marks a stop with an error, a cut-off one with `finish`.
fn outcome(data: &Value) -> (MessageStatus, Option<String>, Option<Ending>) {
    let error = &data["error"];
    let said = error["data"]["message"].as_str().or(error["name"].as_str()).map(String::from);
    match error["name"].as_str() {
        Some("MessageAbortedError") => (MessageStatus::Aborted, said, None),
        Some("MessageOutputLengthError") => (MessageStatus::Done, said, Some(Ending::Length)),
        Some(_) => (MessageStatus::Error, said, None),
        None if data["finish"] == "length" => (MessageStatus::Done, None, Some(Ending::Length)),
        None if data["time"]["completed"].is_i64() => (MessageStatus::Done, None, None),
        None => (MessageStatus::Aborted, None, None),
    }
}

/// A part with no native shape, its original text inside an envelope no native part type matches,
/// so it reads back as unknown even when it looks like one (opencode's synthetic `text`).
pub fn kept(data: &str) -> Part {
    Part::Unknown { raw: serde_json::json!({ "type": "opencode", "data": data }).to_string() }
}

/// The part as this engine stores it, `None` for bookkeeping.
fn part_of<'a>(part: &'a OcPart, minted: &HashMap<String, String>, anthropic: bool) -> Option<(&'a str, Part)> {
    let unknown = || Some((part.id.as_str(), kept(&part.data)));
    let Ok(data) = serde_json::from_str::<Value>(&part.data) else { return unknown() };
    let kind = data["type"].as_str().unwrap_or_default();
    if DROPPED.contains(&kind) {
        return None;
    }
    let mapped = match kind {
        "text" if data["synthetic"] != true => data["text"].as_str().map(|text| Part::Text { text: text.into() }),
        "reasoning" => reasoning(&data, anthropic),
        "tool" => tool_call(&data),
        "file" => file(&data),
        "compaction" => Some(Part::Compaction { auto: data["auto"] == true, tail_from: data["tail_start_id"].as_str().and_then(|old| minted.get(old)).cloned() }),
        _ => None,
    };
    match mapped {
        Some(mapped) => Some((part.id.as_str(), mapped)),
        None => unknown(),
    }
}

/// Claude's signed thinking keeps its signature, so the same model reads its own reasoning back;
/// replay already sends a signature only to the model that wrote it.
fn reasoning(data: &Value, anthropic: bool) -> Option<Part> {
    let text = data["text"].as_str()?.to_string();
    let signed = &data["metadata"]["anthropic"];
    let field = |key: &str| signed[key].as_str().filter(|value| anthropic && !value.is_empty()).map(String::from);
    Some(Part::Reasoning { text, signature: field("signature"), redacted: field("redactedData") })
}

fn tool_call(data: &Value) -> Option<Part> {
    let state = &data["state"];
    let (status, output) = match state["status"].as_str() {
        Some("completed") => (ToolStatus::Done, state["output"].as_str()),
        Some("error") => (ToolStatus::Error, state["error"].as_str()),
        _ => (ToolStatus::Error, None),
    };
    let input = if state["input"].is_object() { state["input"].clone() } else { Value::Object(Default::default()) };
    Some(Part::ToolCall {
        call_id: data["callID"].as_str()?.into(),
        name: data["tool"].as_str()?.into(),
        input,
        status,
        title: state["title"].as_str().map(String::from),
        output: output.map(String::from),
        metadata: Some(slim(data["tool"].as_str()?, state["metadata"].clone())).filter(Value::is_object),
        started_at: state["time"]["start"].as_i64(),
        finished_at: state["time"]["end"].as_i64(),
    })
}

/// A diff past this is no panel anyone reads (one patch to a generated file kept 331 MB); it is dropped.
const MAX_DIFF_BYTES: usize = 1_000_000;

/// Drops display copies no Drift view reads (whole files before and after a patch, a read's file
/// text, oversized diffs), which were most of opencode's tool metadata; a patched file's diff becomes
/// the `patch` its panel draws.
fn slim(tool: &str, mut metadata: Value) -> Value {
    let Some(fields) = metadata.as_object_mut() else { return metadata };
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
    if fields.get(key).and_then(Value::as_str).is_some_and(|text| text.len() > MAX_DIFF_BYTES) {
        fields.remove(key);
    }
}

fn file(data: &Value) -> Option<Part> {
    let path = (data["source"]["type"] == "file").then(|| data["source"]["path"].as_str().map(String::from)).flatten();
    Some(Part::File { mime: data["mime"].as_str()?.into(), name: data["filename"].as_str().unwrap_or_default().into(), url: data["url"].as_str()?.into(), path })
}

pub fn todo(todo: &OcTodo) -> Option<Todo> {
    let status = serde_json::from_value::<TodoStatus>(Value::String(todo.status.clone())).ok()?;
    Some(Todo { content: todo.content.clone(), status, priority: todo.priority.clone() })
}

fn model_ref(provider: &Value, model: &Value) -> Option<ModelRef> {
    Some(ModelRef { provider: provider.as_str()?.into(), model: model.as_str()?.into() })
}
