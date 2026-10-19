use crate::llm::catalog::Reasoning;
use crate::llm::{Block, ChatMessage, Request, Role};
use serde_json::{Value, json};

/// Anthropic allows four breakpoints: tools, system, and these two in the conversation.
const CONVERSATION_BREAKPOINTS: usize = 2;

/// Shared by every Anthropic route (key, subscription, gateway base URLs), so all of them cache.
pub(super) fn body(request: &Request) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    for (index, chat) in request.messages.iter().enumerate() {
        let (sent, notes) = lifted(chat, request.claude.system_messages);
        // Anthropic refuses empty content and joins the same-role turns either side of a dropped message.
        if sent["content"].as_array().is_some_and(|content| !content.is_empty()) {
            messages.push(sent);
            if !notes.is_empty() {
                messages.push(json!({ "role": "system", "content": notes }));
            }
        }
        // Accepted at any position; an empty system message carries only the turn's effort.
        for mark in request.effort_marks.iter().filter(|mark| mark.after == index) {
            messages.push(json!({ "role": "system", "content": [], "output_config": { "effort": mark.effort } }));
        }
    }
    mark_conversation(&mut messages);
    let mut body = json!({
        "model": request.model,
        "max_tokens": request.max_tokens,
        "stream": true,
        "messages": messages,
    });

    if !request.system.is_empty() {
        body["system"] = json!([{ "type": "text", "text": request.system, "cache_control": ephemeral() }]);
    }
    if !request.tools.is_empty() {
        let mut tools: Vec<Value> = request.tools.iter().map(tool_definition).collect();
        if let Some(last) = tools.last_mut() {
            last["cache_control"] = ephemeral();
        }

        body["tools"] = Value::Array(tools);
        if request.no_tool_calls {
            body["tool_choice"] = json!({ "type": "none" });
        }
    }

    match &request.reasoning {
        Some(Reasoning::Budget { tokens }) => body["thinking"] = json!({ "type": "enabled", "budget_tokens": tokens }),
        // Adaptive models otherwise return blank thinking instead of a summary.
        Some(Reasoning::Effort { level }) => {
            body["thinking"] = json!({ "type": "adaptive", "display": "summarized" });
            body["output_config"] = json!({ "effort": level });
        }
        None => {
            if let Some(temperature) = request.temperature {
                body["temperature"] = json!(temperature);
            }
        }
    }
    // A thinking block the API no longer accepts (its prefix changed) is dropped instead of failing the turn.
    if request.claude.drop_stale_thinking && request.reasoning.is_some() {
        body["thinking"]["block_binding"] = json!({ "prefix_mismatch_behavior": "drop_block" });
    }

    crate::llm::apply_mode(&mut body, request);

    body
}

fn tool_definition(tool: &crate::llm::ToolSpec) -> Value {
    json!({ "name": tool.name, "description": tool.description, "input_schema": tool.parameters() })
}

fn ephemeral() -> Value {
    json!({ "type": "ephemeral" })
}

/// The newest user breakpoint writes the prefix; the preceding one reuses the previous request's cache.
/// Keeping both preserves hits when a step adds more blocks than the cache lookback can search.
/// Only user messages receive these conversation breakpoints.
fn mark_conversation(messages: &mut [Value]) {
    for message in messages
        .iter_mut()
        .rev()
        .filter(|message| message["role"] == "user")
        .take(CONVERSATION_BREAKPOINTS)
    {
        let last = message["content"]
            .as_array_mut()
            .and_then(|blocks| blocks.iter_mut().rev().find(|block| cacheable(block)));
        if let Some(block) = last {
            block["cache_control"] = ephemeral();
        }
    }
}

/// Thinking blocks and empty text cannot carry a breakpoint.
fn cacheable(block: &Value) -> bool {
    !matches!(block["type"].as_str(), Some("thinking" | "redacted_thinking")) && block["text"] != ""
}

/// The message as sent, and the engine reminders it carried as the system message that follows it, on a
/// Claude that takes them. A reminder stays in place when nothing else is in the turn: a system message
/// must follow a user turn.
fn lifted(chat: &ChatMessage, native: bool) -> (Value, Vec<Value>) {
    let reminder = |block: &Block| matches!(block, Block::Reminder(_));
    let lift = native && chat.role == Role::User && chat.blocks.iter().any(|block| !reminder(block) && sendable(block));
    if !lift {
        return (message(chat, true), Vec::new());
    }

    let notes = chat
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Reminder(text) => {
                let inner = text.strip_prefix("<system-reminder>\n").unwrap_or(text);
                Some(json!({ "type": "text", "text": inner.strip_suffix("\n</system-reminder>").unwrap_or(inner) }))
            }
            _ => None,
        })
        .collect();

    (message(chat, false), notes)
}

fn message(message: &ChatMessage, reminders: bool) -> Value {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let content: Vec<Value> = message
        .blocks
        .iter()
        .filter(|block| sendable(block) && (reminders || !matches!(block, Block::Reminder(_))))
        .map(block)
        .collect();

    json!({ "role": role, "content": content })
}

/// Every cache breakpoint in `body` kept for an hour instead of five minutes.
pub(super) fn cache_for_an_hour(body: &mut Value) {
    match body {
        Value::Object(fields) => {
            if let Some(control) = fields.get_mut("cache_control").and_then(Value::as_object_mut) {
                control.insert("ttl".into(), json!("1h"));
            }
            fields.values_mut().for_each(cache_for_an_hour);
        }
        Value::Array(items) => items.iter_mut().for_each(cache_for_an_hour),
        _ => {}
    }
}

/// The betas a body needs for what it holds.
pub(super) fn betas(body: &Value) -> Vec<&'static str> {
    let mut betas = Vec::new();
    let messages = body["messages"].as_array().into_iter().flatten();
    if messages
        .clone()
        .any(|message| message["role"] == "system" && message.get("output_config").is_some())
    {
        betas.push(super::EFFORT_MARKS);
    }
    if body["thinking"].get("block_binding").is_some() {
        betas.push(super::THINKING_BINDING);
    }

    betas
}

/// Anthropic refuses reasoning without a signature, including reasoning received from other providers.
fn sendable(block: &Block) -> bool {
    !matches!(
        block.unsigned(),
        Block::Stored { .. }
            | Block::Reasoning {
                signature: None,
                redacted: None,
                ..
            }
    )
}

/// Maps call ids to Anthropic's `[a-zA-Z0-9_-]+` characters on both calls and results.
pub(crate) fn wire_id(id: &str) -> String {
    let id: String = id
        .chars()
        .map(|character| match character {
            character if character.is_ascii_alphanumeric() || character == '_' || character == '-' => character,
            _ => '_',
        })
        .collect();

    if id.is_empty() {
        return "call".into();
    }

    id
}

pub(super) fn block(block: &Block) -> Value {
    match block {
        Block::Signed { part, .. } => self::block(part),
        Block::Text(text) | Block::Reminder(text) => json!({ "type": "text", "text": text }),
        Block::Reasoning {
            redacted: Some(data), ..
        } => json!({ "type": "redacted_thinking", "data": data }),
        Block::Reasoning { text, signature, .. } => {
            let signature = signature.clone().unwrap_or_default();
            json!({ "type": "thinking", "thinking": text, "signature": signature })
        }
        Block::ToolUse { id, name, input } => {
            json!({ "type": "tool_use", "id": wire_id(id), "name": name, "input": input })
        }
        Block::ToolResult {
            call_id,
            content,
            is_error,
        } => {
            let call_id = wire_id(call_id);
            json!({ "type": "tool_result", "tool_use_id": call_id, "content": content, "is_error": is_error })
        }
        Block::Image { mime, base64 } => {
            json!({ "type": "image", "source": { "type": "base64", "media_type": mime, "data": base64 } })
        }
        Block::Pdf { base64 } => {
            let source = json!({ "type": "base64", "media_type": "application/pdf", "data": base64 });
            json!({ "type": "document", "source": source })
        }
        Block::Stored { .. } => json!({ "type": "text", "text": "[file not loaded]" }),
        // Lifted into `effort_marks` before any adapter runs.
        Block::Effort(effort) => json!({ "type": "text", "text": format!("[reasoning effort: {effort}]") }),
    }
}
