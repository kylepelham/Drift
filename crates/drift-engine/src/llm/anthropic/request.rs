use crate::llm::catalog::Reasoning;
use crate::llm::{Block, ChatMessage, Request, Role};
use serde_json::{Value, json};

/// Anthropic allows four breakpoints: tools, system, and these two in the conversation.
const CONVERSATION_BREAKPOINTS: usize = 2;

/// Shared by every Anthropic route (key, subscription, gateway base URLs), so all of them cache.
pub(super) fn body(request: &Request) -> Value {
    let mut messages: Vec<Value> = request.messages.iter().map(message).collect();
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

    crate::llm::apply_mode(&mut body, request);

    body
}

fn tool_definition(tool: &crate::llm::ToolSpec) -> Value {
    json!({ "name": tool.name, "description": tool.description, "input_schema": tool.input_schema })
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

fn message(message: &ChatMessage) -> Value {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let content: Vec<Value> = message
        .blocks
        .iter()
        .filter(|block| sendable(block))
        .map(block)
        .collect();

    json!({ "role": role, "content": content })
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
fn wire_id(id: &str) -> String {
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
        Block::Text(text) => json!({ "type": "text", "text": text }),
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
    }
}
