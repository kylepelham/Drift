use crate::llm::catalog::Reasoning;
use crate::llm::{Block, ChatMessage, Request, Role, TOOL_SEARCH, ToolSpec};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub(super) fn body(request: &Request, subscription: bool) -> Value {
    let loading = Loading::of(request);
    let mut input = Vec::new();
    for (index, message) in request.messages.iter().enumerate() {
        input.extend(items(message, &loading));
        // In effect for the responses after it, until another replaces it.
        for mark in request.effort_marks.iter().filter(|mark| mark.after == index) {
            input.push(json!({ "type": "configuration_update", "reasoning": { "effort": mark.effort } }));
        }
    }
    let mut body = json!({
        "model": request.model,
        "instructions": request.system,
        "input": input,
        "stream": true,
        "store": false,
        "include": ["reasoning.encrypted_content"],
    });

    if !request.tools.is_empty() {
        body["tools"] = request
            .tools
            .iter()
            .map(|tool| tool_definition(tool, loading.searching))
            .collect();
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
        // Explicit mode with no breakpoints writes nothing to the cache, which a one-off request would pay for unread.
        if !request.cache_conversation && request.cache_writes_billed {
            body["prompt_cache_options"] = json!({ "mode": "explicit" });
        }
    }

    crate::llm::apply_mode(&mut body, request);

    body
}

/// `tool_search` goes as the Responses tool of that type, run by Drift, when the request defers tools for it to load.
fn tool_definition(tool: &ToolSpec, searching: bool) -> Value {
    if searching && tool.name == TOOL_SEARCH {
        return json!({
            "type": "tool_search", "execution": "client", "description": tool.description,
            "parameters": tool.parameters(),
        });
    }

    function(tool)
}

fn function(tool: &ToolSpec) -> Value {
    json!({
        "type": "function", "name": tool.name, "description": tool.description,
        "parameters": tool.parameters(), "strict": false,
    })
}

/// How a request's `tool_search` calls go: as `tool_search_call` items answered by `tool_search_output` items
/// that carry the tools each loaded, which the API adds at the end of the context so the cached prefix stays.
#[derive(Default)]
pub(super) struct Loading<'a> {
    searching: bool,
    request: Option<&'a Request>,
    /// The ids of the conversation's `tool_search` calls, whose results are tool lists.
    calls: BTreeSet<&'a str>,
}

impl<'a> Loading<'a> {
    pub(super) fn of(request: &'a Request) -> Self {
        if request.deferred_tools.is_empty() {
            return Self::default();
        }

        let calls = request
            .messages
            .iter()
            .flat_map(|message| &message.blocks)
            .filter_map(|block| match block.unsigned() {
                Block::ToolUse { id, name, .. } if name == TOOL_SEARCH => Some(id.as_str()),
                _ => None,
            })
            .collect();

        Self {
            searching: true,
            request: Some(request),
            calls,
        }
    }

    fn call(&self, id: &str, name: &str, input: &Value) -> Value {
        if self.searching && name == TOOL_SEARCH {
            return json!({
                "type": "tool_search_call", "execution": "client", "call_id": id, "status": "completed",
                "arguments": input,
            });
        }

        json!({ "type": "function_call", "call_id": id, "name": name, "arguments": input.to_string() })
    }

    /// A search's result is the deferred tools it loaded, none when it found nothing; any other is the call's output.
    fn output(&self, call_id: &str, output: &str) -> Value {
        let Some(request) = self.request.filter(|_| self.calls.contains(call_id)) else {
            return json!({ "type": "function_call_output", "call_id": call_id, "output": output });
        };

        let names = request.tool_references.get(call_id).into_iter().flatten();
        let tools: Vec<Value> = names
            .filter_map(|name| request.deferred_tools.iter().find(|tool| &tool.name == name))
            .map(|tool| {
                let mut definition = function(tool);
                definition["defer_loading"] = json!(true);
                definition
            })
            .collect();

        json!({
            "type": "tool_search_output", "execution": "client", "call_id": call_id, "status": "completed",
            "tools": tools,
        })
    }
}

pub(super) fn items(message: &ChatMessage, loading: &Loading) -> Vec<Value> {
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
            Block::Text(text) | Block::Reminder(text) if message.role == Role::User => {
                content.push(json!({ "type": "input_text", "text": text }));
            }
            Block::Text(text) | Block::Reminder(text) => content.push(json!({ "type": "output_text", "text": text })),
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
            Block::Reasoning { .. } | Block::Stored { .. } | Block::Effort(_) => {}
            Block::ToolUse { id, name, input } => {
                flush(&mut content, &mut items);
                items.push(loading.call(id, name, input));
            }
            Block::ToolResult {
                call_id,
                content: output,
                ..
            } => {
                flush(&mut content, &mut items);
                items.push(loading.output(call_id, output));
            }
        }
    }

    flush(&mut content, &mut items);

    items
}
