//! The `/api/chat` body: Ollama's own messages, tools and thinking, and the window to run the model with.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::llm::catalog::Reasoning;
use crate::llm::{Block, ChatMessage, Request, Role, ToolSpec};

/// The request as `/api/chat` takes it. `num_ctx` is the model's window, so a conversation is never cut at a
/// smaller default the server loaded it with.
pub(super) fn body(request: &Request) -> Value {
    let names = tool_names(&request.messages);

    // The system prompt first, then each message as one or more of Ollama's.
    let mut messages = Vec::new();
    if !request.system.is_empty() {
        messages.push(json!({ "role": "system", "content": request.system }));
    }
    for message in &request.messages {
        messages.extend(message_values(message, &names));
    }

    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": true,
        "options": options(request),
    });

    // With calls off no tools are offered; the engine refuses a call such a reply makes anyway.
    if !request.tools.is_empty() && !request.no_tool_calls {
        body["tools"] = request.tools.iter().map(tool_definition).collect();
    }
    if let Some(think) = think(request) {
        body["think"] = think;
    }

    crate::llm::apply_mode(&mut body, request);
    body
}

/// The run options: the window, the reply's limit and the sampling the model is tuned for.
fn options(request: &Request) -> Value {
    let mut options = json!({ "num_predict": request.max_tokens });

    if let Some(window) = request.context_window {
        options["num_ctx"] = json!(window);
    }
    if let Some(temperature) = request.temperature {
        options["temperature"] = json!(temperature);
    }
    if let Some(top_p) = request.top_p {
        options["top_p"] = json!(top_p);
    }
    if let Some(top_k) = request.top_k {
        options["top_k"] = json!(top_k);
    }

    options
}

/// `think`: a level the model lists, `false` for none, or `true` to see a reasoning model's thinking apart
/// from its answer when no level is chosen. Nothing leaves it to the model.
fn think(request: &Request) -> Option<Value> {
    match &request.reasoning {
        Some(Reasoning::Effort { level }) if level == "none" => Some(json!(false)),
        Some(Reasoning::Effort { level }) => Some(json!(level)),
        Some(Reasoning::Budget { .. }) => Some(json!(true)),
        None if request.show_thinking => Some(json!(true)),
        None => None,
    }
}

fn tool_definition(tool: &ToolSpec) -> Value {
    let function = json!({
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.parameters(),
    });

    json!({ "type": "function", "function": function })
}

/// Each call's tool by its id: Ollama matches a result to its call by the tool's name, not an id.
fn tool_names(messages: &[ChatMessage]) -> HashMap<&str, &str> {
    let blocks = messages.iter().flat_map(|message| &message.blocks);

    blocks
        .filter_map(|block| match block.unsigned() {
            Block::ToolUse { id, name, .. } => Some((id.as_str(), name.as_str())),
            _ => None,
        })
        .collect()
}

/// Tool results become their own `tool` messages, ahead of anything else the message holds, so they follow
/// their calls; the rest folds into one message of the message's role.
pub(super) fn message_values(message: &ChatMessage, names: &HashMap<&str, &str>) -> Vec<Value> {
    let mut messages = Vec::new();
    let mut text: Vec<&str> = Vec::new();
    let mut images: Vec<&str> = Vec::new();
    let mut calls: Vec<Value> = Vec::new();
    let mut thinking: Vec<&str> = Vec::new();

    for block in &message.blocks {
        match block.unsigned() {
            Block::Signed { .. } => unreachable!("unsigned blocks cannot be signed"),
            Block::Text(said) => text.push(said),
            Block::Image { base64, .. } => images.push(base64),
            Block::Reasoning { text: thought, .. } => thinking.push(thought),
            Block::ToolUse { name, input, .. } => calls.push(tool_call(name, input)),
            Block::ToolResult { call_id, content, .. } => {
                let name = names.get(call_id.as_str()).copied().unwrap_or_default();
                messages.push(json!({ "role": "tool", "tool_name": name, "content": content }));
            }
            Block::Pdf { .. } | Block::Stored { .. } => {}
        }
    }

    let assistant = message.role == Role::Assistant;
    let thought = assistant && !thinking.is_empty();
    if text.is_empty() && images.is_empty() && calls.is_empty() && !thought {
        return messages;
    }

    // Everything else in one message of the message's role.
    let role = if assistant { "assistant" } else { "user" };
    let mut item = json!({ "role": role, "content": text.join("\n\n") });
    if !images.is_empty() {
        item["images"] = json!(images);
    }
    if !calls.is_empty() {
        item["tool_calls"] = Value::Array(calls);
    }
    if thought {
        item["thinking"] = json!(thinking.join(""));
    }
    messages.push(item);

    messages
}

/// A call as Ollama takes it back: its arguments an object, as it sent them.
fn tool_call(name: &str, input: &Value) -> Value {
    let arguments = if input.is_object() { input.clone() } else { json!({}) };

    json!({ "function": { "name": name, "arguments": arguments } })
}
