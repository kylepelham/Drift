use crate::llm::catalog::Reasoning;
use crate::llm::{Block, ChatMessage, Request, Role};
use serde_json::{Value, json};

pub(super) fn body(request: &Request) -> Value {
    let mut messages = Vec::new();
    if !request.system.is_empty() {
        messages.push(json!({ "role": "system", "content": request.system }));
    }

    // The session excludes reasoning from completed turns before shaping this request.
    messages.extend(request.messages.iter().flat_map(message));
    let mut body = json!({
        "model": request.model, "messages": messages, "stream": true,
        "stream_options": { "include_usage": true }, "max_tokens": request.max_tokens,
    });

    if !request.tools.is_empty() {
        body["tools"] = request.tools.iter().map(tool_definition).collect();
        if request.no_tool_calls {
            body["tool_choice"] = json!("none");
        }
    }
    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(top_p) = request.top_p {
        body["top_p"] = json!(top_p);
    }

    body
}

fn tool_definition(tool: &crate::llm::ToolSpec) -> Value {
    let function = json!({
        "name": tool.name, "description": tool.description, "parameters": tool.input_schema,
    });

    json!({ "type": "function", "function": function })
}

pub(super) fn reason(body: &mut Value, reasoning: Option<&Reasoning>, object: bool) {
    match (reasoning, object) {
        (Some(Reasoning::Effort { level }), true) => body["reasoning"] = json!({ "effort": level }),
        (Some(Reasoning::Budget { tokens }), true) => body["reasoning"] = json!({ "max_tokens": tokens }),
        (Some(Reasoning::Effort { level }), false) => body["reasoning_effort"] = json!(level),
        _ => {}
    }
}

/// OpenRouter names Claude `anthropic/...`, or `~anthropic/...` for its moving aliases.
pub(super) fn is_claude(model: &str) -> bool {
    model.trim_start_matches('~').starts_with("anthropic/")
}

/// Marks the system prompt and the last two user turns with Anthropic's cache breakpoints.
/// A turn is the run of tool and user messages between assistant replies.
/// This lets a tool loop keep caching beyond the prompt that started it.
pub(super) fn mark_breakpoints(body: &mut Value) {
    let Some(messages) = body["messages"].as_array_mut() else {
        return;
    };

    if let Some(system) = messages.iter_mut().find(|message| message["role"] == "system") {
        let text = system["content"].as_str().unwrap_or_default().to_string();
        system["content"] = json!([{ "type": "text", "text": text, "cache_control": ephemeral() }]);
    }

    let mut marked = 0;
    let mut in_turn = false;
    for message in messages.iter_mut().rev() {
        let turn = message["role"] == "user" || message["role"] == "tool";
        if !turn {
            in_turn = false;
            continue;
        }
        if in_turn || marked == 2 {
            continue;
        }

        if mark_last_text(message) {
            in_turn = true;
            marked += 1;
        }
    }
}

fn ephemeral() -> Value {
    json!({ "type": "ephemeral" })
}

/// Marks a message's last non-empty text part; plain tool content becomes a part to carry the breakpoint.
fn mark_last_text(message: &mut Value) -> bool {
    if let Some(text) = message["content"]
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_string)
    {
        message["content"] = json!([{ "type": "text", "text": text, "cache_control": ephemeral() }]);
        return true;
    }

    let last = message["content"].as_array_mut().and_then(|parts| {
        parts
            .iter_mut()
            .rev()
            .find(|part| part["type"] == "text" && part["text"] != "")
    });
    match last {
        Some(part) => {
            part["cache_control"] = ephemeral();
            true
        }
        None => false,
    }
}

/// Tool results are their own `tool` messages; everything else folds into one message per role.
pub(super) fn message(message: &ChatMessage) -> Vec<Value> {
    let mut messages = Vec::new();
    let mut content: Vec<Value> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut reasoning: Option<String> = None;

    for block in &message.blocks {
        match block.unsigned() {
            Block::Signed { .. } => unreachable!("unsigned blocks cannot be signed"),
            Block::Text(text) => content.push(json!({ "type": "text", "text": text })),
            Block::Image { mime, base64 } => {
                let url = format!("data:{mime};base64,{base64}");
                content.push(json!({ "type": "image_url", "image_url": { "url": url } }));
            }
            Block::Reasoning { text, .. } => reasoning = Some(text.clone()),
            Block::ToolUse { id, name, input } => {
                let function = json!({ "name": name, "arguments": input.to_string() });
                tool_calls.push(json!({ "id": id, "type": "function", "function": function }));
            }
            Block::ToolResult { call_id, content, .. } => {
                messages.push(json!({ "role": "tool", "tool_call_id": call_id, "content": content }));
            }
            Block::Pdf { base64 } => {
                let file_data = format!("data:application/pdf;base64,{base64}");
                content.push(json!({ "type": "file", "file": { "filename": "document.pdf", "file_data": file_data } }));
            }
            Block::Stored { .. } => {}
        }
    }

    if content.is_empty() && tool_calls.is_empty() {
        return messages;
    }

    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let mut item = json!({ "role": role });
    item["content"] = if message.role == Role::Assistant {
        let text = content
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("");
        Value::String(text)
    } else {
        Value::Array(content)
    };
    if !tool_calls.is_empty() {
        item["tool_calls"] = Value::Array(tool_calls);
    }
    if let Some(reasoning) = reasoning.filter(|_| message.role == Role::Assistant) {
        item["reasoning_content"] = Value::String(reasoning);
    }

    // Tool messages must immediately follow their calls; same-turn prose follows those results.
    messages.push(item);

    messages
}
