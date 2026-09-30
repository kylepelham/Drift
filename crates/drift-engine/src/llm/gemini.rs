//! Gemini generateContent over SSE. Also serves Vertex once its token exchange lands.

use std::collections::HashMap;

use futures_util::StreamExt;
use serde_json::{json, Value};

use super::sse;
use super::{Block, ChatMessage, Chunk, ChunkStream, Credential, Error, Request, Role, StopReason};
use crate::session::types::Usage;

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

#[derive(Clone, Debug)]
pub struct Gemini {
    base_url: String,
    client: reqwest::Client,
    pub timeouts: super::http::Timeouts,
}

impl Default for Gemini {
    fn default() -> Self {
        Self::new(DEFAULT_BASE_URL)
    }
}

impl Gemini {
    pub fn new(base_url: &str) -> Self {
        Self { base_url: base_url.trim_end_matches('/').to_string(), client: super::http::client(), timeouts: super::http::Timeouts::default() }
    }

    pub async fn stream(&self, request: &Request, credential: &Credential) -> Result<ChunkStream, Error> {
        let url = format!("{}/models/{}:streamGenerateContent?alt=sse", self.base_url, request.model);
        let http = match credential {
            Credential::ApiKey { key } => self.client.post(url).header("x-goog-api-key", key),
            Credential::OAuth { access, .. } => self.client.post(url).bearer_auth(access),
        };
        let response = super::http::send(http.header("accept", "text/event-stream").json(&body(request)), &self.timeouts).await?;
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
    let mut names: HashMap<String, String> = HashMap::new();
    let mut body = json!({
        "contents": request.messages.iter().map(|m| content(m, &mut names)).collect::<Vec<_>>(),
        "generationConfig": { "maxOutputTokens": request.max_tokens },
    });
    if !request.system.is_empty() {
        body["systemInstruction"] = json!({ "parts": [{ "text": request.system }] });
    }
    if !request.tools.is_empty() {
        let declarations: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| json!({ "name": tool.name, "description": tool.description, "parameters": schema(&tool.input_schema) }))
            .collect();
        body["tools"] = json!([{ "functionDeclarations": declarations }]);
    }
    if let Some(budget) = request.thinking_budget {
        body["generationConfig"]["thinkingConfig"] = json!({ "thinkingBudget": budget, "includeThoughts": true });
    } else if let Some(temperature) = request.temperature {
        body["generationConfig"]["temperature"] = json!(temperature);
    }
    body
}

/// Gemini's schema dialect rejects a few JSON Schema keywords; drop them rather than fail the call.
fn schema(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| !matches!(key.as_str(), "$schema" | "additionalProperties" | "default" | "examples"))
                .map(|(key, value)| (key.clone(), schema(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(schema).collect()),
        other => other.clone(),
    }
}

/// Tool results need the function's name, which only the earlier call carries; `names` remembers it.
fn content(message: &ChatMessage, names: &mut HashMap<String, String>) -> Value {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "model",
    };
    let mut signature: Option<String> = None;
    let mut parts: Vec<Value> = Vec::new();
    for block in &message.blocks {
        match block {
            Block::Text(text) => parts.push(json!({ "text": text })),
            Block::Image { mime, base64 } => parts.push(json!({ "inlineData": { "mimeType": mime, "data": base64 } })),
            Block::Reasoning { signature: Some(sig), .. } => signature = Some(sig.clone()),
            Block::Reasoning { .. } => {}
            Block::ToolUse { id, name, input } => {
                names.insert(id.clone(), name.clone());
                parts.push(json!({ "functionCall": { "id": id, "name": name, "args": input } }));
            }
            Block::ToolResult { call_id, content, is_error } => {
                let name = names.get(call_id).cloned().unwrap_or_default();
                let key = if *is_error { "error" } else { "output" };
                parts.push(json!({ "functionResponse": { "id": call_id, "name": name, "response": { key: content } } }));
            }
        }
    }
    if let (Some(signature), Some(first)) = (signature, parts.first_mut()) {
        first["thoughtSignature"] = Value::String(signature);
    }
    json!({ "role": role, "parts": parts })
}

fn api_error(status: u16, text: &str) -> Error {
    let parsed: Value = serde_json::from_str(text).unwrap_or_default();
    let kind = parsed["error"]["status"].as_str().unwrap_or("api_error").to_string();
    let message = parsed["error"]["message"].as_str().unwrap_or(text).to_string();
    match status {
        401 | 403 => Error::Unauthenticated,
        _ => Error::api(status, kind, message),
    }
}

#[derive(Default)]
struct StreamState {
    open: Option<Open>,
    calls: u32,
    called_tools: bool,
    signature: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Open {
    Text,
    Thought,
}

impl StreamState {
    fn chunks(&mut self, data: &str) -> Result<Vec<Chunk>, Error> {
        let value: Value = serde_json::from_str(data).map_err(|e| Error::Malformed(e.to_string()))?;
        if value["error"].is_object() {
            return Err(api_error(super::STREAMED, data));
        }
        let mut out = Vec::new();
        let candidate = &value["candidates"][0];
        for part in candidate["content"]["parts"].as_array().into_iter().flatten() {
            out.extend(self.part(part));
        }
        if let Some(usage) = value.get("usageMetadata").filter(|u| u.is_object()) {
            let count = |key: &str| usage[key].as_u64().unwrap_or(0);
            let cache_read = count("cachedContentTokenCount");
            out.push(Chunk::Usage(Usage {
                input: count("promptTokenCount").saturating_sub(cache_read),
                output: count("candidatesTokenCount") + count("thoughtsTokenCount"),
                cache_read,
                cache_write: 0,
            }));
        }
        if let Some(reason) = candidate["finishReason"].as_str() {
            out.extend(self.close());
            out.push(Chunk::Stop(match reason {
                "MAX_TOKENS" => StopReason::MaxTokens,
                "STOP" if self.called_tools => StopReason::ToolUse,
                "STOP" => StopReason::EndTurn,
                _ => StopReason::Other,
            }));
        }
        Ok(out)
    }

    fn part(&mut self, part: &Value) -> Vec<Chunk> {
        let mut out = Vec::new();
        if let Some(signature) = part["thoughtSignature"].as_str() {
            self.signature = Some(signature.into());
        }
        if let Some(call) = part.get("functionCall") {
            out.extend(self.close());
            self.calls += 1;
            self.called_tools = true;
            let id = call["id"].as_str().map(str::to_string).unwrap_or_else(|| format!("call_{}", self.calls));
            out.push(Chunk::ToolUseStart { id, name: call["name"].as_str().unwrap_or_default().into() });
            out.push(Chunk::ToolInputDelta(call["args"].to_string()));
            out.push(Chunk::BlockStop);
            return out;
        }
        let Some(text) = part["text"].as_str() else { return out };
        let kind = if part["thought"].as_bool().unwrap_or(false) { Open::Thought } else { Open::Text };
        if self.open != Some(kind) {
            out.extend(self.close());
            out.push(if kind == Open::Thought { Chunk::ReasoningStart } else { Chunk::TextStart });
            self.open = Some(kind);
        }
        out.push(if self.open == Some(Open::Thought) { Chunk::ReasoningDelta(text.into()) } else { Chunk::TextDelta(text.into()) });
        out
    }

    /// The signature is attached to whatever thought block was open, so it can be replayed.
    fn close(&mut self) -> Vec<Chunk> {
        let mut out = Vec::new();
        match self.open.take() {
            Some(Open::Thought) => {
                if let Some(signature) = self.signature.take() {
                    out.push(Chunk::ReasoningSignature(signature));
                }
                out.push(Chunk::BlockStop);
            }
            Some(Open::Text) => out.push(Chunk::BlockStop),
            None => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolSpec;

    fn request() -> Request {
        Request {
            model: "gemini-2.5-pro".into(),
            system: "sys".into(),
            messages: vec![
                ChatMessage { role: Role::User, blocks: vec![Block::Text("hi".into())] },
                ChatMessage {
                    role: Role::Assistant,
                    blocks: vec![
                        Block::Reasoning { text: "hm".into(), signature: Some("sig".into()), redacted: None },
                        Block::ToolUse { id: "call_1".into(), name: "read".into(), input: json!({ "path": "a" }) },
                    ],
                },
                ChatMessage { role: Role::User, blocks: vec![Block::ToolResult { call_id: "call_1".into(), content: "1: x".into(), is_error: false }] },
            ],
            tools: vec![ToolSpec { name: "read".into(), description: "r".into(), input_schema: json!({ "type": "object", "additionalProperties": false, "properties": {} }) }],
            max_tokens: 500,
            thinking_budget: Some(2048),
            temperature: None,
            cache_key: None,
        }
    }

    #[test]
    fn body_matches_generate_content() {
        let built = body(&request());
        assert_eq!(built["systemInstruction"]["parts"][0]["text"], "sys");
        assert_eq!(built["generationConfig"]["thinkingConfig"]["thinkingBudget"], 2048);
        let contents = built["contents"].as_array().unwrap();
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"][0]["functionCall"]["name"], "read");
        assert_eq!(contents[1]["parts"][0]["thoughtSignature"], "sig");
        assert_eq!(contents[2]["parts"][0]["functionResponse"]["name"], "read");
        assert_eq!(contents[2]["parts"][0]["functionResponse"]["response"]["output"], "1: x");
        let declaration = &built["tools"][0]["functionDeclarations"][0];
        assert!(declaration["parameters"].get("additionalProperties").is_none());
    }

    #[test]
    fn stream_parts_become_blocks_with_signature_on_the_thought() {
        let mut state = StreamState::default();
        let feed = |state: &mut StreamState, json: &str| state.chunks(json).unwrap();
        assert_eq!(feed(&mut state, r#"{"candidates":[{"content":{"parts":[{"text":"th","thought":true}]}}]}"#), vec![Chunk::ReasoningStart, Chunk::ReasoningDelta("th".into())]);
        let call = feed(&mut state, r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"read","args":{"path":"a"}},"thoughtSignature":"sig"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":3,"thoughtsTokenCount":4}}"#);
        assert_eq!(
            call,
            vec![
                Chunk::ReasoningSignature("sig".into()),
                Chunk::BlockStop,
                Chunk::ToolUseStart { id: "call_1".into(), name: "read".into() },
                Chunk::ToolInputDelta(r#"{"path":"a"}"#.into()),
                Chunk::BlockStop,
                Chunk::Usage(Usage { input: 10, output: 7, cache_read: 0, cache_write: 0 }),
                Chunk::Stop(StopReason::ToolUse),
            ]
        );
        let mut plain = StreamState::default();
        assert_eq!(feed(&mut plain, r#"{"candidates":[{"content":{"parts":[{"text":"Hi"}]}}]}"#), vec![Chunk::TextStart, Chunk::TextDelta("Hi".into())]);
        assert_eq!(feed(&mut plain, r#"{"candidates":[{"content":{"parts":[]},"finishReason":"MAX_TOKENS"}]}"#), vec![Chunk::BlockStop, Chunk::Stop(StopReason::MaxTokens)]);
        assert!(matches!(StreamState::default().chunks(r#"{"error":{"status":"UNAVAILABLE","message":"x"}}"#), Err(Error::Api { retryable: true, .. })));
        assert!(matches!(StreamState::default().chunks(r#"{"error":{"status":"INVALID_ARGUMENT","message":"x"}}"#), Err(Error::Api { retryable: false, .. })));
    }
}
