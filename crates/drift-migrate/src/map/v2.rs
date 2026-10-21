//! opencode v2 rows (`session_message`) to this engine's conversation shapes.
//! A v2 row holds its whole content; there are no part rows, so a row's pieces become its parts.

use super::{Ids, kept, model_ref, native_id, reasoning, slim, usage};
use crate::source::{KEPT_BYTES, OcMessage, truncate};
use drift_engine::session::types::{
    Ending, Message, MessageStatus, MessageWithParts, Part, PartRow, Role, ToolMetadata, ToolStatus, Usage,
};
use serde_json::{Value, json};

/// Rows that only mark where a turn ended or what the next one runs on; each Drift message carries its agent and model.
const DROPPED: [&str; 4] = ["idle", "agent-switched", "model-switched", "location-switched"];

/// Maps a row to what it stands for: nothing for bookkeeping, a boundary and a summary for a compaction, else one message.
/// A row with no Drift shape (`synthetic`, `system`, `shell`, `skill`, an unfinished compaction) is one user message holding it raw.
pub(crate) fn messages(
    row: &OcMessage,
    data: &str,
    oversized: bool,
    session_id: &str,
    ids: &mut Ids,
) -> Vec<MessageWithParts> {
    let value: Value = serde_json::from_str(data).unwrap_or(Value::Null);
    let created = value["time"]["created"].as_i64().unwrap_or(row.created);
    let user = || base(session_id, Role::User, created);

    match value["type"].as_str().unwrap_or_default() {
        kind if DROPPED.contains(&kind) => Vec::new(),
        "user" => vec![build(ids, &row.id, row.created, user(), user_parts(&value))],
        "assistant" => {
            let (info, parts) = assistant(&value, oversized, session_id, created);
            vec![build(ids, &row.id, row.created, info, parts)]
        }
        "compaction" if summarised(&value) => compaction(row, &value, session_id, created, ids),
        _ => vec![build(ids, &row.id, row.created, user(), vec![kept(data)])],
    }
}

/// Mints the message's id in written order and its parts' ids from the row id and their place in it.
fn build(ids: &mut Ids, old: &str, written: i64, mut info: Message, parts: Vec<Part>) -> MessageWithParts {
    let (id, stamp) = ids.mint(old, written);
    let parts = parts
        .into_iter()
        .enumerate()
        .map(|(index, part)| PartRow {
            id: native_id("prt", stamp + index as i64, &format!("{old}/{index}")),
            message_id: id.clone(),
            session_id: info.session_id.clone(),
            provider_signature: None,
            part,
        })
        .collect();
    info.id = id;

    MessageWithParts { info, parts }
}

fn base(session_id: &str, role: Role, created: i64) -> Message {
    Message {
        id: String::new(),
        session_id: session_id.into(),
        role,
        status: MessageStatus::Done,
        model: None,
        agent: None,
        usage: Usage::default(),
        cost: 0.0,
        error: None,
        created_at: created,
        finished_at: Some(created),
        generation_ms: None,
        account: None,
        summary: false,
        ending: None,
        refusal: None,
        cache_miss: None,
    }
}

/// The prompt's text and attached files; mentioned agents and skills are kept raw, since the text already names them.
fn user_parts(value: &Value) -> Vec<Part> {
    let mut parts: Vec<Part> = value["text"]
        .as_str()
        .filter(|text| !text.is_empty())
        .map(|text| Part::Text { text: text.into() })
        .into_iter()
        .collect();

    for file in value["files"].as_array().into_iter().flatten() {
        parts.push(attachment(file).unwrap_or_else(|| kept(&file.to_string())));
    }
    for key in ["agents", "skills"] {
        if value[key].as_array().is_some_and(|list| !list.is_empty()) {
            parts.push(kept(&json!({ key: value[key] }).to_string()));
        }
    }

    parts
}

/// v2 keeps an attachment's bytes as base64 beside its type, which is a data URL here.
fn attachment(file: &Value) -> Option<Part> {
    let mime = file["mime"].as_str()?;

    Some(Part::File {
        mime: mime.into(),
        name: file["name"].as_str().unwrap_or_default().into(),
        url: format!("data:{mime};base64,{}", file["data"].as_str()?),
        path: None,
        image: None,
    })
}

fn assistant(value: &Value, oversized: bool, session_id: &str, created: i64) -> (Message, Vec<Part>) {
    let model = &value["model"];
    let anthropic = model["providerID"] == "anthropic";
    let (status, error, ending) = outcome(value);

    let mut info = base(session_id, Role::Assistant, created);
    info.status = status;
    info.error = error;
    info.ending = ending;
    info.model = model_ref(&model["providerID"], &model["id"]);
    info.agent = value["agent"].as_str().map(String::from);
    info.usage = usage(&value["tokens"]);
    info.cost = value["cost"].as_f64().unwrap_or(0.0);
    info.finished_at = value["time"]["completed"].as_i64();

    let parts = value["content"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| content(item, anthropic, oversized))
        .collect();

    (info, parts)
}

/// v2 errors are typed, `aborted` among them; a finish of `length` or `content-filter` ends a reply that is otherwise done.
fn outcome(value: &Value) -> (MessageStatus, Option<String>, Option<Ending>) {
    let error = &value["error"];
    let message = error["message"].as_str().or(error["type"].as_str()).map(String::from);

    match (error["type"].as_str(), value["finish"].as_str()) {
        (Some("aborted"), _) => (MessageStatus::Aborted, message, None),
        (Some(_), _) => (MessageStatus::Error, message, None),
        (None, Some("length")) => (MessageStatus::Done, None, Some(Ending::Length)),
        (None, Some("content-filter")) => (MessageStatus::Done, None, Some(Ending::Refused)),
        (None, _) if value["time"]["completed"].is_i64() => (MessageStatus::Done, None, None),
        (None, _) => (MessageStatus::Aborted, None, None),
    }
}

/// An item of a reply; one of no native shape, or a call missing its id, is kept raw.
fn content(item: &Value, anthropic: bool, oversized: bool) -> Part {
    let mapped = match item["type"].as_str() {
        Some("text") => item["text"].as_str().map(|text| Part::Text { text: text.into() }),
        Some("reasoning") => reasoning(&json!({ "text": item["text"], "metadata": item["state"] }), anthropic),
        Some("tool") => tool(item, oversized),
        _ => None,
    };

    mapped.unwrap_or_else(|| kept(&item.to_string()))
}

/// A call keeps the name and input v2 gave it. In a row past the size limit its input over 64 KB, its output past
/// 64 KB and its metadata are left behind, as for an oversized v1 call.
fn tool(item: &Value, oversized: bool) -> Option<Part> {
    let state = &item["state"];
    let name = item["name"].as_str()?;
    let text = result_text(&state["content"]);
    let (status, output) = match state["status"].as_str() {
        Some("completed") => (ToolStatus::Done, text),
        Some("error") => (
            ToolStatus::Error,
            state["error"]["message"].as_str().map(String::from).or(text),
        ),
        _ => (ToolStatus::Error, None),
    };
    let input = Some(state["input"].clone())
        .filter(Value::is_object)
        .filter(|input| !oversized || input.to_string().len() <= KEPT_BYTES as usize)
        .unwrap_or_else(|| json!({}));
    let metadata = Some(state["metadata"].clone())
        .filter(|_| !oversized)
        .map(|metadata| slim(name, metadata))
        .filter(Value::is_object)
        .map(|metadata| Box::new(ToolMetadata::from(metadata)));

    Some(Part::ToolCall {
        call_id: item["id"].as_str()?.into(),
        name: name.into(),
        input,
        status,
        title: None,
        output: if oversized { truncate(output) } else { output },
        metadata,
        started_at: item["time"]["created"].as_i64(),
        finished_at: item["time"]["completed"].as_i64(),
    })
}

/// A result's text items in order; a file it returned is named, its bytes left behind.
fn result_text(content: &Value) -> Option<String> {
    let lines: Vec<String> = content
        .as_array()?
        .iter()
        .filter_map(|item| match item["type"].as_str() {
            Some("text") => item["text"].as_str().map(String::from),
            Some("file") => Some(format!(
                "[file {}]",
                item["name"].as_str().or(item["mime"].as_str()).unwrap_or_default()
            )),
            _ => None,
        })
        .collect();

    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Only a finished compaction with a written summary is a boundary; a native one carries provider state Drift cannot send.
fn summarised(value: &Value) -> bool {
    value["status"] == "completed"
        && value["summary"]
            .as_str()
            .is_some_and(|summary| !summary.trim().is_empty())
}

/// A boundary keeping no message, then a summary holding v2's summary and the recent history it kept as text.
fn compaction(row: &OcMessage, value: &Value, session_id: &str, created: i64, ids: &mut Ids) -> Vec<MessageWithParts> {
    let boundary = Part::Compaction {
        auto: value["reason"] == "auto",
        tail_from: None,
    };

    let mut summary = base(session_id, Role::Assistant, created);
    summary.summary = true;
    summary.model = model_ref(&value["model"]["providerID"], &value["model"]["id"]);
    summary.usage = usage(&value["tokens"]);
    summary.cost = value["cost"].as_f64().unwrap_or(0.0);
    let texts = ["summary", "recent"]
        .into_iter()
        .filter_map(|key| value[key].as_str().filter(|text| !text.trim().is_empty()))
        .map(|text| Part::Text { text: text.into() })
        .collect();

    vec![
        build(
            ids,
            &row.id,
            row.created,
            base(session_id, Role::User, created),
            vec![boundary],
        ),
        build(ids, &format!("{}/summary", row.id), row.created, summary, texts),
    ]
}
