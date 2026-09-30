//! OpenAI Responses API over SSE, for API keys and for ChatGPT subscriptions through the Codex backend.

pub mod oauth;

use std::collections::HashSet;

use futures_util::StreamExt;
use serde_json::{json, Value};

use super::sse;
use super::{Block, ChatMessage, Chunk, ChunkStream, Credential, Error, Request, Role, StopReason};
use crate::session::types::Usage;

const API_BASE_URL: &str = "https://api.openai.com/v1";
const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
/// The client identity the Codex backend expects on subscription traffic.
const CODEX_ORIGINATOR: &str = "opencode";

#[derive(Clone, Debug)]
pub struct OpenAi {
    base_url: Option<String>,
    client: reqwest::Client,
}

impl Default for OpenAi {
    fn default() -> Self {
        Self { base_url: None, client: reqwest::Client::new() }
    }
}

impl OpenAi {
    pub fn new(base_url: &str) -> Self {
        Self { base_url: Some(base_url.trim_end_matches('/').to_string()), client: reqwest::Client::new() }
    }

    pub async fn stream(&self, request: &Request, credential: &Credential) -> Result<ChunkStream, Error> {
        let subscription = matches!(credential, Credential::OAuth { .. });
        let default_base = if subscription { CODEX_BASE_URL } else { API_BASE_URL };
        let base = self.base_url.as_deref().unwrap_or(default_base);
        let mut http = self.client.post(format!("{base}/responses")).header("accept", "text/event-stream");
        http = match credential {
            Credential::ApiKey { key } => http.bearer_auth(key),
            Credential::OAuth { access, account, .. } => {
                let http = http.bearer_auth(access).header("originator", CODEX_ORIGINATOR);
                match account {
                    Some(account) => http.header("chatgpt-account-id", account),
                    None => http,
                }
            }
        };
        let response = http.json(&body(request, subscription)).send().await?;
        let status = response.status();
        if !status.is_success() {
            let headers = response.headers().clone();
            return Err(api_error(status.as_u16(), &response.text().await.unwrap_or_default()).with_headers(&headers));
        }
        let mut state = StreamState::default();
        let events = sse::events(response.bytes_stream());
        Ok(Box::pin(events.flat_map(move |event| {
            let items: Vec<Result<Chunk, Error>> = match event {
                Err(error) => vec![Err(Error::Transport(error.to_string()))],
                Ok(event) => match state.chunks(&event.data) {
                    Ok(chunks) => chunks.into_iter().map(Ok).collect(),
                    Err(error) => vec![Err(error)],
                },
            };
            futures_util::stream::iter(items)
        })))
    }
}

fn body(request: &Request, subscription: bool) -> Value {
    let mut body = json!({
        "model": request.model,
        "instructions": request.system,
        "input": request.messages.iter().flat_map(items).collect::<Vec<_>>(),
        "stream": true,
        "store": false,
        "include": ["reasoning.encrypted_content"],
    });
    if !request.tools.is_empty() {
        body["tools"] = request
            .tools
            .iter()
            .map(|tool| json!({ "type": "function", "name": tool.name, "description": tool.description, "parameters": tool.input_schema, "strict": false }))
            .collect();
        body["tool_choice"] = json!("auto");
    }
    if let Some(effort) = request.thinking_budget.map(effort) {
        body["reasoning"] = json!({ "effort": effort, "summary": "auto" });
    }
    if !subscription {
        body["max_output_tokens"] = json!(request.max_tokens);
        if let Some(temperature) = request.temperature {
            body["temperature"] = json!(temperature);
        }
    }
    body
}

/// Thinking budgets are Anthropic's unit; OpenAI takes an effort level, so bucket them.
fn effort(budget: u32) -> &'static str {
    match budget {
        0..=4_000 => "low",
        4_001..=12_000 => "medium",
        _ => "high",
    }
}

fn items(message: &ChatMessage) -> Vec<Value> {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let mut out = Vec::new();
    let mut content: Vec<Value> = Vec::new();
    let flush = |content: &mut Vec<Value>, out: &mut Vec<Value>| {
        if !content.is_empty() {
            out.push(json!({ "role": role, "content": std::mem::take(content) }));
        }
    };
    for block in &message.blocks {
        match block {
            Block::Text(text) if message.role == Role::User => content.push(json!({ "type": "input_text", "text": text })),
            Block::Text(text) => content.push(json!({ "type": "output_text", "text": text })),
            Block::Image { mime, base64 } => content.push(json!({ "type": "input_image", "image_url": format!("data:{mime};base64,{base64}") })),
            Block::Reasoning { text, signature: Some(encrypted), .. } => {
                flush(&mut content, &mut out);
                out.push(json!({ "type": "reasoning", "summary": [{ "type": "summary_text", "text": text }], "encrypted_content": encrypted }));
            }
            Block::Reasoning { .. } => {}
            Block::ToolUse { id, name, input } => {
                flush(&mut content, &mut out);
                out.push(json!({ "type": "function_call", "call_id": id, "name": name, "arguments": input.to_string() }));
            }
            Block::ToolResult { call_id, content: output, .. } => {
                flush(&mut content, &mut out);
                out.push(json!({ "type": "function_call_output", "call_id": call_id, "output": output }));
            }
        }
    }
    flush(&mut content, &mut out);
    out
}

/// `code` names the fault; a streamed `error` event's `type` is just "error".
fn api_error(status: u16, text: &str) -> Error {
    let parsed: Value = serde_json::from_str(text).unwrap_or_default();
    let error = if parsed["error"].is_object() { &parsed["error"] } else { &parsed };
    let kind = error["code"].as_str().or(error["type"].as_str()).unwrap_or("api_error").to_string();
    let message = error["message"].as_str().unwrap_or(text).to_string();
    match status {
        401 | 403 => Error::Unauthenticated,
        _ => Error::api(status, kind, message),
    }
}

/// Which streamed items are open, so item-level events map to the right block kind.
#[derive(Default)]
struct StreamState {
    reasoning: HashSet<String>,
    messages: HashSet<String>,
    calls_with_deltas: HashSet<String>,
    called_tools: bool,
}

impl StreamState {
    fn chunks(&mut self, data: &str) -> Result<Vec<Chunk>, Error> {
        let value: Value = serde_json::from_str(data).map_err(|e| Error::Malformed(e.to_string()))?;
        let kind = value["type"].as_str().unwrap_or_default();
        let item_id = value["item_id"].as_str().unwrap_or_default();
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_string();
        Ok(match kind {
            "response.output_item.added" => self.item_added(&value["item"]),
            "response.output_text.delta" => vec![Chunk::TextDelta(text("delta"))],
            "response.reasoning_summary_part.added" if value["summary_index"].as_u64().unwrap_or(0) > 0 => vec![Chunk::ReasoningDelta("\n\n".into())],
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => vec![Chunk::ReasoningDelta(text("delta"))],
            "response.function_call_arguments.delta" => {
                self.calls_with_deltas.insert(item_id.into());
                vec![Chunk::ToolInputDelta(text("delta"))]
            }
            "response.output_item.done" => self.item_done(&value["item"]),
            "response.completed" | "response.incomplete" => self.finished(&value["response"], kind == "response.incomplete"),
            "response.failed" => return Err(api_error(super::STREAMED, &value["response"].to_string())),
            "error" => return Err(api_error(super::STREAMED, data)),
            _ => Vec::new(),
        })
    }

    fn item_added(&mut self, item: &Value) -> Vec<Chunk> {
        let id = item["id"].as_str().unwrap_or_default().to_string();
        match item["type"].as_str().unwrap_or_default() {
            "message" => {
                self.messages.insert(id);
                vec![Chunk::TextStart]
            }
            "reasoning" => {
                self.reasoning.insert(id);
                vec![Chunk::ReasoningStart]
            }
            "function_call" => {
                self.called_tools = true;
                vec![Chunk::ToolUseStart { id: item["call_id"].as_str().unwrap_or_default().into(), name: item["name"].as_str().unwrap_or_default().into() }]
            }
            _ => Vec::new(),
        }
    }

    fn item_done(&mut self, item: &Value) -> Vec<Chunk> {
        let id = item["id"].as_str().unwrap_or_default();
        match item["type"].as_str().unwrap_or_default() {
            "message" => {
                self.messages.remove(id);
                vec![Chunk::BlockStop]
            }
            "reasoning" => {
                self.reasoning.remove(id);
                let mut out = Vec::new();
                if let Some(encrypted) = item["encrypted_content"].as_str() {
                    out.push(Chunk::ReasoningSignature(encrypted.into()));
                }
                out.push(Chunk::BlockStop);
                out
            }
            "function_call" => {
                let mut out = Vec::new();
                if !self.calls_with_deltas.remove(id) {
                    if let Some(arguments) = item["arguments"].as_str().filter(|a| !a.is_empty()) {
                        out.push(Chunk::ToolInputDelta(arguments.into()));
                    }
                }
                out.push(Chunk::BlockStop);
                out
            }
            _ => Vec::new(),
        }
    }

    fn finished(&self, response: &Value, incomplete: bool) -> Vec<Chunk> {
        let usage = &response["usage"];
        let count = |path: &[&str]| path.iter().fold(usage, |v, key| &v[*key]).as_u64().unwrap_or(0);
        let cache_read = count(&["input_tokens_details", "cached_tokens"]);
        let stop = if incomplete && response["incomplete_details"]["reason"] == "max_output_tokens" {
            StopReason::MaxTokens
        } else if self.called_tools {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        };
        vec![
            Chunk::Usage(Usage { input: count(&["input_tokens"]).saturating_sub(cache_read), output: count(&["output_tokens"]), cache_read, cache_write: 0 }),
            Chunk::Stop(stop),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolSpec;

    fn request() -> Request {
        Request {
            model: "gpt-5.4".into(),
            system: "You are Drift.".into(),
            messages: vec![
                ChatMessage { role: Role::User, blocks: vec![Block::Text("hi".into()), Block::Image { mime: "image/png".into(), base64: "AAAA".into() }] },
                ChatMessage {
                    role: Role::Assistant,
                    blocks: vec![
                        Block::Reasoning { text: "think".into(), signature: Some("enc".into()), redacted: None },
                        Block::Text("Let me look".into()),
                        Block::ToolUse { id: "call_1".into(), name: "read".into(), input: json!({ "path": "a" }) },
                    ],
                },
                ChatMessage { role: Role::User, blocks: vec![Block::ToolResult { call_id: "call_1".into(), content: "ok".into(), is_error: false }] },
            ],
            tools: vec![ToolSpec { name: "read".into(), description: "Reads".into(), input_schema: json!({ "type": "object" }) }],
            max_tokens: 1000,
            thinking_budget: Some(10_000),
            temperature: None,
        }
    }

    #[test]
    fn body_matches_the_responses_api() {
        let built = body(&request(), false);
        assert_eq!(built["instructions"], "You are Drift.");
        assert_eq!(built["store"], false);
        assert_eq!(built["include"][0], "reasoning.encrypted_content");
        assert_eq!(built["reasoning"]["effort"], "medium");
        assert_eq!(built["max_output_tokens"], 1000);
        assert_eq!(built["tools"][0]["type"], "function");
        let input = built["input"].as_array().unwrap();
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        assert_eq!(input[0]["content"][1]["type"], "input_image");
        assert_eq!(input[1]["type"], "reasoning");
        assert_eq!(input[1]["encrypted_content"], "enc");
        assert_eq!(input[2]["role"], "assistant");
        assert_eq!(input[2]["content"][0]["type"], "output_text");
        assert_eq!(input[3]["type"], "function_call");
        assert_eq!(input[3]["call_id"], "call_1");
        assert_eq!(input[3]["arguments"], r#"{"path":"a"}"#);
        assert_eq!(input[4]["type"], "function_call_output");
        assert!(body(&request(), true).get("max_output_tokens").is_none());
    }

    #[test]
    fn stream_events_map_to_chunks() {
        let mut state = StreamState::default();
        let feed = |state: &mut StreamState, json: &str| state.chunks(json).unwrap();
        assert_eq!(feed(&mut state, r#"{"type":"response.created","response":{}}"#), vec![]);
        assert_eq!(feed(&mut state, r#"{"type":"response.output_item.added","item":{"type":"reasoning","id":"rs_1"}}"#), vec![Chunk::ReasoningStart]);
        assert_eq!(feed(&mut state, r#"{"type":"response.reasoning_summary_text.delta","item_id":"rs_1","delta":"hm"}"#), vec![Chunk::ReasoningDelta("hm".into())]);
        assert_eq!(feed(&mut state, r#"{"type":"response.reasoning_summary_part.added","item_id":"rs_1","summary_index":1}"#), vec![Chunk::ReasoningDelta("\n\n".into())]);
        assert_eq!(
            feed(&mut state, r#"{"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_1","encrypted_content":"enc"}}"#),
            vec![Chunk::ReasoningSignature("enc".into()), Chunk::BlockStop]
        );
        assert_eq!(feed(&mut state, r#"{"type":"response.output_item.added","item":{"type":"message","id":"msg_1"}}"#), vec![Chunk::TextStart]);
        assert_eq!(feed(&mut state, r#"{"type":"response.output_text.delta","item_id":"msg_1","delta":"Hi"}"#), vec![Chunk::TextDelta("Hi".into())]);
        assert_eq!(feed(&mut state, r#"{"type":"response.output_item.done","item":{"type":"message","id":"msg_1"}}"#), vec![Chunk::BlockStop]);
        assert_eq!(
            feed(&mut state, r#"{"type":"response.output_item.added","item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read","arguments":""}}"#),
            vec![Chunk::ToolUseStart { id: "call_1".into(), name: "read".into() }]
        );
        assert_eq!(feed(&mut state, r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"{\"pa"}"#), vec![Chunk::ToolInputDelta("{\"pa".into())]);
        assert_eq!(
            feed(&mut state, r#"{"type":"response.output_item.done","item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read","arguments":"{\"path\":\"a\"}"}}"#),
            vec![Chunk::BlockStop]
        );
        let done = feed(&mut state, r#"{"type":"response.completed","response":{"usage":{"input_tokens":100,"input_tokens_details":{"cached_tokens":40},"output_tokens":9}}}"#);
        assert_eq!(done, vec![Chunk::Usage(Usage { input: 60, output: 9, cache_read: 40, cache_write: 0 }), Chunk::Stop(StopReason::ToolUse)]);
    }

    #[test]
    fn a_call_without_deltas_takes_arguments_from_done() {
        let mut state = StreamState::default();
        state.chunks(r#"{"type":"response.output_item.added","item":{"type":"function_call","id":"fc_1","call_id":"c","name":"read"}}"#).unwrap();
        let done = state.chunks(r#"{"type":"response.output_item.done","item":{"type":"function_call","id":"fc_1","call_id":"c","name":"read","arguments":"{}"}}"#).unwrap();
        assert_eq!(done, vec![Chunk::ToolInputDelta("{}".into()), Chunk::BlockStop]);
    }

    #[test]
    fn incomplete_and_errors_classify() {
        let state = StreamState::default();
        let out = state.finished(&json!({ "incomplete_details": { "reason": "max_output_tokens" }, "usage": {} }), true);
        assert_eq!(out[1], Chunk::Stop(StopReason::MaxTokens));
        let streamed = StreamState::default().chunks(r#"{"type":"error","code":"server_error","message":"try again"}"#);
        assert!(matches!(streamed, Err(Error::Api { ref kind, retryable: true, .. }) if kind == "server_error"), "{streamed:?}");
        let failed = StreamState::default().chunks(r#"{"type":"response.failed","response":{"error":{"code":"rate_limit_exceeded","message":"slow"}}}"#);
        assert!(matches!(failed, Err(Error::Api { retryable: true, .. })), "{failed:?}");
        let refused = StreamState::default().chunks(r#"{"type":"error","code":"invalid_prompt","message":"no"}"#);
        assert!(matches!(refused, Err(Error::Api { retryable: false, .. })));
        assert!(matches!(api_error(429, "{}"), Error::Api { retryable: true, .. }));
        assert!(matches!(api_error(401, "{}"), Error::Unauthenticated));
        assert_eq!(effort(2_000), "low");
        assert_eq!(effort(20_000), "high");
    }
}
