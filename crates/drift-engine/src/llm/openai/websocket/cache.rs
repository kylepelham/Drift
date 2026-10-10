//! Which part of a request the server already has: a request continues the previous response only when its
//! settings match and its history begins with everything that response saw and said.

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

type Hash = [u8; 32];

/// Transport fields that never go in a WebSocket request.
const TRANSPORT_FIELDS: [&str; 3] = ["stream", "background", "previous_response_id"];

/// Fields a continuation may change freely; anything else must match to continue.
const PER_REQUEST_FIELDS: [&str; 5] = [
    "input",
    "stream",
    "background",
    "previous_response_id",
    "access_programs",
];

/// The last completed response on a connection.
pub(super) struct Previous {
    id: String,
    settings: Hash,
    /// Its input followed by its output, one hash per item.
    history: Vec<Hash>,
}

impl Previous {
    pub(super) fn completed(body: &Value, response: &Value) -> Option<Self> {
        let id = response["id"].as_str().filter(|id| !id.is_empty())?.to_string();

        let mut history = hashes(body["input"].as_array()?);
        history.extend(hashes(response["output"].as_array()?));

        Some(Self {
            id,
            settings: settings(body),
            history,
        })
    }

    /// The items after what this response already covers; `None` when the request does not continue it.
    fn new_items(&self, body: &Value) -> Option<Vec<Value>> {
        let input = body["input"].as_array()?;
        if self.settings != settings(body) || !hashes(input).starts_with(&self.history) {
            return None;
        }

        Some(input[self.history.len()..].to_vec())
    }
}

/// The `response.create` event: only the new items and the previous id when the request continues, else everything.
pub(super) fn payload(body: &Value, previous: Option<&Previous>) -> Value {
    let continuation = previous.and_then(|previous| Some((previous, previous.new_items(body)?)));

    let mut omitted = TRANSPORT_FIELDS.to_vec();
    if continuation.is_some() {
        omitted.push("input");
    }

    let mut fields = fields_without(body, &omitted);
    fields.insert("type".into(), json!("response.create"));

    if let Some((previous, items)) = continuation {
        fields.insert("previous_response_id".into(), json!(previous.id));
        fields.insert("input".into(), json!(items));
    }

    Value::Object(fields)
}

fn settings(body: &Value) -> Hash {
    hash(&Value::Object(fields_without(body, &PER_REQUEST_FIELDS)))
}

fn fields_without(body: &Value, omitted: &[&str]) -> Map<String, Value> {
    body.as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| !omitted.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn hashes(items: &[Value]) -> Vec<Hash> {
    items.iter().map(|item| hash(&comparable(item))).collect()
}

fn hash(value: &Value) -> Hash {
    Sha256::digest(value.to_string()).into()
}

/// An item reduced to what both sides agree on: the server's output carries ids and statuses Drift's replay does not.
fn comparable(item: &Value) -> Value {
    let kind = item["type"].as_str().unwrap_or("message");

    match kind {
        "message" if item["role"].is_string() => {
            let content: Vec<_> = item["content"].as_array().into_iter().flatten().map(content).collect();
            json!({ "type": "message", "role": item["role"], "content": content })
        }
        "function_call" => {
            let arguments = item["arguments"]
                .as_str()
                .and_then(|text| serde_json::from_str::<Value>(text).ok())
                .unwrap_or_else(|| item["arguments"].clone());
            json!({ "type": kind, "call_id": item["call_id"], "name": item["name"], "arguments": arguments })
        }
        "reasoning" => {
            let summary: Vec<_> = item["summary"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|part| part["text"].as_str())
                .collect();
            json!({ "type": kind, "encrypted_content": item["encrypted_content"], "summary": summary.join("\n\n") })
        }
        _ => item.clone(),
    }
}

fn content(part: &Value) -> Value {
    match part["type"].as_str() {
        Some("input_text" | "output_text") => json!({ "type": part["type"], "text": part["text"] }),
        _ => part.clone(),
    }
}
