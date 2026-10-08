use crate::llm::catalog::Reasoning;
use crate::llm::{Block, ChatMessage, Request, Role};
use serde_json::{Value, json};

pub(super) fn body(request: &Request, subscription: bool) -> Value {
    let input: Vec<Value> = request.messages.iter().flat_map(items).collect();
    let mut body = json!({
        "model": request.model,
        "instructions": request.system,
        "input": input,
        "stream": true,
        "store": false,
        "include": ["reasoning.encrypted_content"],
    });

    if !request.tools.is_empty() {
        body["tools"] = request.tools.iter().map(tool_definition).collect();
        body["tool_choice"] = json!(if request.no_tool_calls { "none" } else { "auto" });
    }
    if let Some(Reasoning::Effort { level }) = &request.reasoning {
        body["reasoning"] = json!({ "effort": level, "summary": "auto" });
    }
    if let Some(verbosity) = request.verbosity {
        body["text"] = json!({ "verbosity": verbosity });
    }

    // Codex uses the same key on every request of a conversation to share the prompt cache.
    if let Some(key) = &request.cache_key {
        body["prompt_cache_key"] = json!(key);
    }
    if !subscription {
        body["max_output_tokens"] = json!(request.max_tokens);
        if let Some(temperature) = request.temperature {
            body["temperature"] = json!(temperature);
        }
    }

    crate::llm::apply_mode(&mut body, request);

    body
}

fn tool_definition(tool: &crate::llm::ToolSpec) -> Value {
    json!({
        "type": "function", "name": tool.name, "description": tool.description,
        "parameters": tool.input_schema, "strict": false,
    })
}

pub(super) fn items(message: &ChatMessage) -> Vec<Value> {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let mut items = Vec::new();
    let mut content: Vec<Value> = Vec::new();
    let flush = |content: &mut Vec<Value>, items: &mut Vec<Value>| {
        if !content.is_empty() {
            items.push(json!({ "role": role, "content": std::mem::take(content) }));
        }
    };

    for block in &message.blocks {
        match block.unsigned() {
            Block::Signed { .. } => unreachable!("unsigned blocks cannot be signed"),
            Block::Text(text) if message.role == Role::User => {
                content.push(json!({ "type": "input_text", "text": text }));
            }
            Block::Text(text) => content.push(json!({ "type": "output_text", "text": text })),
            Block::Image { mime, base64 } => {
                let image_url = format!("data:{mime};base64,{base64}");
                content.push(json!({ "type": "input_image", "image_url": image_url }));
            }
            Block::Reasoning {
                text,
                signature: Some(encrypted),
                ..
            } => {
                flush(&mut content, &mut items);
                let summary = json!([{ "type": "summary_text", "text": text }]);
                items.push(json!({ "type": "reasoning", "summary": summary, "encrypted_content": encrypted }));
            }
            Block::Pdf { base64 } => {
                let file_data = format!("data:application/pdf;base64,{base64}");
                content.push(json!({ "type": "input_file", "filename": "document.pdf", "file_data": file_data }));
            }
            Block::Reasoning { .. } | Block::Stored { .. } => {}
            Block::ToolUse { id, name, input } => {
                flush(&mut content, &mut items);
                let arguments = input.to_string();
                items.push(json!({ "type": "function_call", "call_id": id, "name": name, "arguments": arguments }));
            }
            Block::ToolResult {
                call_id,
                content: output,
                ..
            } => {
                flush(&mut content, &mut items);
                items.push(json!({ "type": "function_call_output", "call_id": call_id, "output": output }));
            }
        }
    }

    flush(&mut content, &mut items);

    items
}
