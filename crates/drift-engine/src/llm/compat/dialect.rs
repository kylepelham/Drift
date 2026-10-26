//! What one route of the Chat Completions dialect changes: how it is told to think, and what it refuses.

use serde_json::{Map, Value, json};

use crate::llm::catalog::{Reasoning, THINKING_OFF, THINKING_ON};

/// How a route takes a reasoning level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Thinking {
    /// `reasoning_effort`, with no field for a budget.
    Effort,
    /// OpenRouter's `reasoning` object, which takes a budget too.
    Object,
    /// Z.ai: thinking always on, with the turn's earlier reasoning kept (`clear_thinking: false`).
    Kept,
    /// Moonshot: `thinking` switches the models that have a switch; the others take an effort.
    Moonshot,
    /// DeepSeek: `thinking` on or off, an effort beside it.
    DeepSeek,
    /// Alibaba: `enable_thinking`, with an effort or a `thinking_budget`.
    Alibaba,
}

pub(super) fn reason(body: &mut Value, reasoning: Option<&Reasoning>, thinking: Thinking) {
    match (thinking, reasoning) {
        (Thinking::Object, Some(Reasoning::Effort { level })) => body["reasoning"] = json!({ "effort": level }),
        (Thinking::Object, Some(Reasoning::Budget { tokens })) => body["reasoning"] = json!({ "max_tokens": tokens }),
        (Thinking::Effort | Thinking::Kept, Some(Reasoning::Effort { level })) => {
            body["reasoning_effort"] = json!(level);
        }
        (Thinking::Moonshot | Thinking::DeepSeek | Thinking::Alibaba, Some(reasoning)) => {
            switched(body, reasoning, thinking);
        }
        _ => {}
    }

    if thinking == Thinking::Kept {
        body["thinking"] = json!({ "type": "enabled", "clear_thinking": false });
    }
}

/// The switch levels turn thinking off or on; an effort or a budget turns it on as well.
fn switched(body: &mut Value, reasoning: &Reasoning, thinking: Thinking) {
    let (on, effort, budget) = match reasoning {
        Reasoning::Effort { level } if level == THINKING_OFF => (false, None, None),
        Reasoning::Effort { level } if level == THINKING_ON => (true, None, None),
        Reasoning::Effort { level } => (true, Some(level), None),
        Reasoning::Budget { tokens } => (true, None, Some(*tokens)),
    };
    if let Some(level) = effort {
        body["reasoning_effort"] = json!(level);
    }

    match thinking {
        Thinking::Alibaba => {
            body["enable_thinking"] = json!(on);
            if let Some(tokens) = budget {
                body["thinking_budget"] = json!(tokens);
            }
        }
        // Kimi K3 always reasons and is sent only its effort, as opencode sends it.
        Thinking::Moonshot if effort.is_some() => {}
        _ => body["thinking"] = json!({ "type": if on { "enabled" } else { "disabled" } }),
    }
}

/// DeepSeek, Kimi and Qwen want `reasoning_content` on every assistant message, empty when it had none.
pub(super) fn reasoning_on_every_reply(body: &mut Value) {
    let replies = body["messages"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .filter(|message| message["role"] == "assistant");
    for reply in replies {
        if reply.get("reasoning_content").is_none() {
            reply["reasoning_content"] = json!("");
        }
    }
}

/// Moonshot refuses an `enum` with no `type` beside it and tuple `items`; both become forms it takes.
pub(super) fn moonshot_schemas(body: &mut Value) {
    for tool in body
        .get_mut("tools")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        let parameters = &mut tool["function"]["parameters"];
        *parameters = moonshot_node(parameters.take());
    }
}

fn moonshot_node(schema: Value) -> Value {
    match schema {
        Value::Array(items) => Value::Array(items.into_iter().map(moonshot_node).collect()),
        Value::Object(fields) if fields.get("$ref").is_some_and(Value::is_string) => json!({ "$ref": fields["$ref"] }),
        Value::Object(fields) => Value::Object(moonshot_object(fields)),
        other => other,
    }
}

fn moonshot_object(fields: Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    let typed = fields.contains_key("type");
    if let Some(kind) = fields
        .get("enum")
        .and_then(Value::as_array)
        .filter(|_| !typed)
        .and_then(|values| enum_type(values))
    {
        out.insert("type".into(), kind);
    }

    let listed = fields.contains_key("items");
    for (key, value) in fields {
        match key.as_str() {
            "items" if value.is_array() => out.insert(key, tuple(value)),
            "prefixItems" if !listed => out.insert("items".into(), tuple(value)),
            "prefixItems" | "unevaluatedItems" => None,
            _ => out.insert(key, moonshot_node(value)),
        };
    }

    out
}

/// A tuple becomes one item schema: its only member, or any of them.
fn tuple(items: Value) -> Value {
    let mut members: Vec<Value> = match items {
        Value::Array(items) => items.into_iter().map(moonshot_node).collect(),
        _ => Vec::new(),
    };

    match members.len() {
        0 => json!({}),
        1 => members.remove(0),
        _ => json!({ "anyOf": members }),
    }
}

/// The values' one JSON type, or one type and null; `None` when they mix more.
fn enum_type(values: &[Value]) -> Option<Value> {
    let mut kinds: Vec<&str> = Vec::new();
    for value in values {
        let kind = match value {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }

    match kinds.as_slice() {
        [one] => Some(json!(one)),
        ["null", other] | [other, "null"] => Some(json!([other, "null"])),
        _ => None,
    }
}
