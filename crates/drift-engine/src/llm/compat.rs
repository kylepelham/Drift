//! OpenAI Chat Completions over SSE: the dialect xAI, Z.ai, OpenRouter, LM Studio and Ollama all speak.

use std::collections::BTreeMap;

use futures_util::StreamExt;
use serde_json::{json, Value};

use super::sse;
use super::{Block, ChatMessage, Chunk, ChunkStream, Credential, Error, Request, Role, StopReason};
use crate::session::types::Usage;

#[derive(Clone, Debug)]
pub struct Compat {
    base_url: String,
    client: reqwest::Client,
    pub timeouts: super::http::Timeouts,
}

impl Compat {
    pub fn new(base_url: &str) -> Self {
        Self { base_url: base_url.trim_end_matches('/').to_string(), client: super::http::client(), timeouts: super::http::Timeouts::default() }
    }

    pub async fn stream(&self, request: &Request, credential: &Credential) -> Result<ChunkStream, Error> {
        let key = match credential {
            Credential::ApiKey { key } => key.clone(),
            Credential::OAuth { access, .. } => access.clone(),
            Credential::Ambient { .. } => return Err(Error::Unauthenticated),
        };
        let sending = self.client.post(format!("{}/chat/completions", self.base_url)).bearer_auth(key).header("accept", "text/event-stream").json(&body(request));
        let response = super::http::send(sending, &self.timeouts).await?;
        let status = response.status();
        if !status.is_success() {
            let headers = response.headers().clone();
            return Err(api_error(status.as_u16(), &super::http::bounded_body(response, &self.timeouts).await).with_headers(&headers));
        }
        let mut state = StreamState::default();
        let events = sse::events(response.bytes_stream(), self.timeouts.idle);
        Ok(Box::pin(events.flat_map(move |event| {
            let items: Vec<Result<Chunk, Error>> = match event {
                Err(error) => vec![Err(Error::Transport(error))],
                Ok(event) => match state.chunks(&event.data) {
                    Ok(chunks) => chunks.into_iter().map(Ok).collect(),
                    Err(error) => vec![Err(error)],
                },
            };
            futures_util::stream::iter(items)
        })))
    }
}

fn body(request: &Request) -> Value {
    let mut messages = Vec::new();
    if !request.system.is_empty() {
        messages.push(json!({ "role": "system", "content": request.system }));
    }
    messages.extend(request.messages.iter().flat_map(message));
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
        "max_tokens": request.max_tokens,
    });
    if !request.tools.is_empty() {
        body["tools"] = request
            .tools
            .iter()
            .map(|tool| json!({ "type": "function", "function": { "name": tool.name, "description": tool.description, "parameters": tool.input_schema } }))
            .collect();
    }
    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    body
}

/// Tool results are their own `tool` messages; everything else folds into one message per role.
fn message(message: &ChatMessage) -> Vec<Value> {
    let mut out = Vec::new();
    let mut content: Vec<Value> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut reasoning: Option<String> = None;
    for block in &message.blocks {
        match block {
            Block::Text(text) => content.push(json!({ "type": "text", "text": text })),
            Block::Image { mime, base64 } => content.push(json!({ "type": "image_url", "image_url": { "url": format!("data:{mime};base64,{base64}") } })),
            Block::Reasoning { text, .. } => reasoning = Some(text.clone()),
            Block::ToolUse { id, name, input } => tool_calls.push(json!({ "id": id, "type": "function", "function": { "name": name, "arguments": input.to_string() } })),
            Block::ToolResult { call_id, content, .. } => out.push(json!({ "role": "tool", "tool_call_id": call_id, "content": content })),
        }
    }
    if content.is_empty() && tool_calls.is_empty() {
        return out;
    }
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let mut item = json!({ "role": role });
    item["content"] = if message.role == Role::Assistant {
        Value::String(content.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join(""))
    } else {
        Value::Array(content)
    };
    if !tool_calls.is_empty() {
        item["tool_calls"] = Value::Array(tool_calls);
    }
    if let Some(reasoning) = reasoning.filter(|_| message.role == Role::Assistant) {
        item["reasoning_content"] = Value::String(reasoning);
    }
    out.insert(0, item);
    out
}

/// Gateways put an HTTP code inside a streamed error object; it classifies like the status it names.
fn api_error(status: u16, text: &str) -> Error {
    let parsed: Value = serde_json::from_str(text).unwrap_or_default();
    let error = if parsed["error"].is_object() { &parsed["error"] } else { &parsed };
    let kind = error["type"].as_str().or(error["code"].as_str()).unwrap_or("api_error").to_string();
    let message = error["message"].as_str().unwrap_or(text).to_string();
    let named = error["code"].as_u64().and_then(|code| u16::try_from(code).ok()).filter(|_| status == super::STREAMED);
    match named.unwrap_or(status) {
        401 | 403 => Error::Unauthenticated,
        status => Error::api(status, kind, message),
    }
}

/// Deltas arrive interleaved by `index`; a block stays open until a different kind of delta arrives.
#[derive(Default)]
struct StreamState {
    open: Option<Open>,
    calls: BTreeMap<u64, String>,
    called_tools: bool,
    usage: Option<Usage>,
    finish: Option<StopReason>,
}

#[derive(PartialEq)]
enum Open {
    Text,
    Reasoning,
    Call(u64),
}

impl StreamState {
    fn chunks(&mut self, data: &str) -> Result<Vec<Chunk>, Error> {
        if data.trim() == "[DONE]" {
            return Ok(self.done());
        }
        let value: Value = serde_json::from_str(data).map_err(|e| Error::Malformed(e.to_string()))?;
        if value["error"].is_object() {
            return Err(api_error(super::STREAMED, data));
        }
        if let Some(usage) = value.get("usage").filter(|u| u.is_object()) {
            self.usage = Some(usage_from(usage));
        }
        let mut out = Vec::new();
        let Some(choice) = value["choices"].get(0) else { return Ok(out) };
        let delta = &choice["delta"];
        if let Some(text) = delta["reasoning_content"].as_str().or(delta["reasoning"].as_str()).filter(|t| !t.is_empty()) {
            out.extend(self.switch(Open::Reasoning, Chunk::ReasoningStart));
            out.push(Chunk::ReasoningDelta(text.into()));
        }
        if let Some(text) = delta["content"].as_str().filter(|t| !t.is_empty()) {
            out.extend(self.switch(Open::Text, Chunk::TextStart));
            out.push(Chunk::TextDelta(text.into()));
        }
        for call in delta["tool_calls"].as_array().into_iter().flatten() {
            out.extend(self.tool_delta(call));
        }
        if let Some(reason) = choice["finish_reason"].as_str() {
            self.finish = Some(match reason {
                "tool_calls" | "function_call" => StopReason::ToolUse,
                "length" => StopReason::MaxTokens,
                "stop" => StopReason::EndTurn,
                _ => StopReason::Other,
            });
        }
        Ok(out)
    }

    fn switch(&mut self, next: Open, start: Chunk) -> Vec<Chunk> {
        if self.open == Some(next) {
            return Vec::new();
        }
        let mut out = Vec::new();
        if self.open.take().is_some() {
            out.push(Chunk::BlockStop);
        }
        self.open = Some(match start {
            Chunk::TextStart => Open::Text,
            Chunk::ReasoningStart => Open::Reasoning,
            _ => Open::Text,
        });
        out.push(start);
        out
    }

    fn tool_delta(&mut self, call: &Value) -> Vec<Chunk> {
        let index = call["index"].as_u64().unwrap_or(0);
        let mut out = Vec::new();
        if self.open != Some(Open::Call(index)) {
            if self.open.take().is_some() {
                out.push(Chunk::BlockStop);
            }
            self.open = Some(Open::Call(index));
            let id = call["id"].as_str().map(str::to_string).unwrap_or_else(|| format!("call_{index}"));
            let id = self.calls.entry(index).or_insert(id).clone();
            let name = call["function"]["name"].as_str().unwrap_or_default().into();
            self.called_tools = true;
            out.push(Chunk::ToolUseStart { id, name });
        }
        if let Some(arguments) = call["function"]["arguments"].as_str().filter(|a| !a.is_empty()) {
            out.push(Chunk::ToolInputDelta(arguments.into()));
        }
        out
    }

    fn done(&mut self) -> Vec<Chunk> {
        let mut out = Vec::new();
        if self.open.take().is_some() {
            out.push(Chunk::BlockStop);
        }
        if let Some(usage) = self.usage.take() {
            out.push(Chunk::Usage(usage));
        }
        let stop = match self.finish.take() {
            Some(StopReason::EndTurn) | None if self.called_tools => StopReason::ToolUse,
            Some(reason) => reason,
            None => StopReason::EndTurn,
        };
        out.push(Chunk::Stop(stop));
        out
    }
}

fn usage_from(usage: &Value) -> Usage {
    let count = |key: &str| usage[key].as_u64().unwrap_or(0);
    let cache_read = usage["prompt_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0);
    Usage { input: count("prompt_tokens").saturating_sub(cache_read), output: count("completion_tokens"), cache_read, cache_write: 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolSpec;

    fn request() -> Request {
        Request {
            model: "grok-4.5".into(),
            system: "sys".into(),
            messages: vec![
                ChatMessage { role: Role::User, blocks: vec![Block::Text("hi".into())] },
                ChatMessage {
                    role: Role::Assistant,
                    blocks: vec![
                        Block::Reasoning { text: "hm".into(), signature: None, redacted: None },
                        Block::Text("ok".into()),
                        Block::ToolUse { id: "call_1".into(), name: "read".into(), input: json!({ "path": "a" }) },
                    ],
                },
                ChatMessage { role: Role::User, blocks: vec![Block::ToolResult { call_id: "call_1".into(), content: "1: x".into(), is_error: false }] },
            ],
            tools: vec![ToolSpec { name: "read".into(), description: "r".into(), input_schema: json!({ "type": "object" }) }],
            max_tokens: 500,
            thinking_budget: None,
            temperature: Some(0.2),
            cache_key: None,
        }
    }

    #[test]
    fn body_matches_chat_completions() {
        let built = body(&request());
        let messages = built["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["content"][0]["type"], "text");
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(messages[2]["content"], "ok");
        assert_eq!(messages[2]["reasoning_content"], "hm");
        assert_eq!(messages[2]["tool_calls"][0]["function"]["arguments"], r#"{"path":"a"}"#);
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["tool_call_id"], "call_1");
        assert_eq!(built["tools"][0]["function"]["name"], "read");
        assert_eq!(built["stream_options"]["include_usage"], true);
        assert_eq!(built["temperature"], 0.2);
    }

    #[test]
    fn stream_deltas_become_blocks() {
        let mut state = StreamState::default();
        let feed = |state: &mut StreamState, json: &str| state.chunks(json).unwrap();
        assert_eq!(feed(&mut state, r#"{"choices":[{"delta":{"role":"assistant","content":""}}]}"#), vec![]);
        assert_eq!(feed(&mut state, r#"{"choices":[{"delta":{"reasoning_content":"th"}}]}"#), vec![Chunk::ReasoningStart, Chunk::ReasoningDelta("th".into())]);
        assert_eq!(feed(&mut state, r#"{"choices":[{"delta":{"content":"Hi"}}]}"#), vec![Chunk::BlockStop, Chunk::TextStart, Chunk::TextDelta("Hi".into())]);
        assert_eq!(feed(&mut state, r#"{"choices":[{"delta":{"content":"!"}}]}"#), vec![Chunk::TextDelta("!".into())]);
        assert_eq!(
            feed(&mut state, r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"read","arguments":""}}]}}]}"#),
            vec![Chunk::BlockStop, Chunk::ToolUseStart { id: "call_a".into(), name: "read".into() }]
        );
        assert_eq!(feed(&mut state, r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"p"}}]}}]}"#), vec![Chunk::ToolInputDelta("{\"p".into())]);
        assert_eq!(feed(&mut state, r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#), vec![]);
        assert_eq!(feed(&mut state, r#"{"choices":[],"usage":{"prompt_tokens":50,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":20}}}"#), vec![]);
        assert_eq!(feed(&mut state, "[DONE]"), vec![Chunk::BlockStop, Chunk::Usage(Usage { input: 30, output: 7, cache_read: 20, cache_write: 0 }), Chunk::Stop(StopReason::ToolUse)]);
    }

    #[test]
    fn errors_and_length_stops_classify() {
        assert!(matches!(StreamState::default().chunks(r#"{"error":{"message":"nope","code":"bad"}}"#), Err(Error::Api { retryable: false, .. })));
        let gateway = StreamState::default().chunks(r#"{"error":{"message":"Provider returned error","code":502},"choices":[{"finish_reason":"error"}]}"#);
        assert!(matches!(gateway, Err(Error::Api { status: 502, retryable: true, .. })), "a gateway's streamed 502 retries: {gateway:?}");
        let overloaded = StreamState::default().chunks(r#"{"error":{"message":"busy","type":"overloaded_error"}}"#);
        assert!(matches!(overloaded, Err(Error::Api { retryable: true, .. })), "an upstream overload passed through retries");
        assert!(matches!(StreamState::default().chunks(r#"{"error":{"message":"key","code":401}}"#), Err(Error::Unauthenticated)));
        let mut state = StreamState::default();
        state.chunks(r#"{"choices":[{"delta":{"content":"x"},"finish_reason":"length"}]}"#).unwrap();
        assert!(state.done().contains(&Chunk::Stop(StopReason::MaxTokens)));
        assert!(matches!(api_error(429, "{}"), Error::Api { retryable: true, .. }));
    }
}
