//! Anthropic Messages API over SSE. Also serves Bedrock and Vertex once their signing lands.

pub mod claude_code;
pub mod oauth;

use futures_util::StreamExt;
use serde_json::{json, Value};

use super::sse;
use super::{Block, ChatMessage, Chunk, ChunkStream, Credential, Error, Request, Role, StopReason};
use crate::session::types::Usage;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";

#[derive(Clone, Debug)]
pub struct Anthropic {
    pub base_url: String,
    client: reqwest::Client,
}

impl Default for Anthropic {
    fn default() -> Self {
        Self::new(DEFAULT_BASE_URL)
    }
}

impl Anthropic {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// Subscription tokens only work for requests shaped like Claude Code's; keys take the plain path.
    pub async fn stream(&self, request: &Request, credential: &Credential) -> Result<ChunkStream, Error> {
        let mut body = body(request);
        let subscription = matches!(credential, Credential::OAuth { .. });
        let url = format!("{}/v1/messages{}", self.base_url, if subscription { "?beta=true" } else { "" });
        let http = self.client.post(url).header("anthropic-version", API_VERSION).header("accept", "text/event-stream");
        let http = match credential {
            Credential::ApiKey { key } => http.header("x-api-key", key),
            Credential::OAuth { access, .. } => {
                claude_code::transform(&mut body);
                http.bearer_auth(access).header("anthropic-beta", claude_code::BETAS).header("user-agent", claude_code::user_agent())
            }
        };
        let response = http.json(&body).send().await?;
        let status = response.status();
        if !status.is_success() {
            return Err(api_error(status.as_u16(), &response.text().await.unwrap_or_default()));
        }
        let events = sse::events(response.bytes_stream());
        Ok(Box::pin(events.flat_map(move |event| {
            let items: Vec<Result<Chunk, Error>> = match event {
                Err(error) => vec![Err(Error::Transport(error.to_string()))],
                Ok(event) => match chunks(&event.event, &event.data) {
                    Ok(chunks) => chunks.into_iter().map(|c| Ok(unprefix(c, subscription))).collect(),
                    Err(error) => vec![Err(error)],
                },
            };
            futures_util::stream::iter(items)
        })))
    }
}

fn unprefix(chunk: Chunk, subscription: bool) -> Chunk {
    match chunk {
        Chunk::ToolUseStart { id, name } if subscription => Chunk::ToolUseStart { id, name: claude_code::original_name(&name) },
        other => other,
    }
}

/// Anthropic allows four breakpoints: tools, system, and these two in the conversation.
const CONVERSATION_BREAKPOINTS: usize = 2;

/// Shared by every Anthropic route (key, subscription, gateway base URLs), so all of them cache.
fn body(request: &Request) -> Value {
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
        let mut tools: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| json!({ "name": tool.name, "description": tool.description, "input_schema": tool.input_schema }))
            .collect();
        if let Some(last) = tools.last_mut() {
            last["cache_control"] = ephemeral();
        }
        body["tools"] = Value::Array(tools);
    }
    if let Some(budget) = request.thinking_budget {
        body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
    } else if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    body
}

fn ephemeral() -> Value {
    json!({ "type": "ephemeral" })
}

/// Rolling breakpoints on the last two user messages: the newest writes the whole prefix, and the one
/// before it sits exactly where the previous request wrote, so a step that adds more blocks than the
/// cache lookback still hits.
fn mark_conversation(messages: &mut [Value]) {
    for message in messages.iter_mut().rev().filter(|m| m["role"] == "user").take(CONVERSATION_BREAKPOINTS) {
        let last = message["content"].as_array_mut().and_then(|blocks| blocks.iter_mut().rev().find(|b| cacheable(b)));
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
    json!({ "role": role, "content": message.blocks.iter().map(block).collect::<Vec<_>>() })
}

fn block(block: &Block) -> Value {
    match block {
        Block::Text(text) => json!({ "type": "text", "text": text }),
        Block::Reasoning { redacted: Some(data), .. } => json!({ "type": "redacted_thinking", "data": data }),
        Block::Reasoning { text, signature, .. } => {
            json!({ "type": "thinking", "thinking": text, "signature": signature.clone().unwrap_or_default() })
        }
        Block::ToolUse { id, name, input } => json!({ "type": "tool_use", "id": id, "name": name, "input": input }),
        Block::ToolResult { call_id, content, is_error } => {
            json!({ "type": "tool_result", "tool_use_id": call_id, "content": content, "is_error": is_error })
        }
        Block::Image { mime, base64 } => {
            json!({ "type": "image", "source": { "type": "base64", "media_type": mime, "data": base64 } })
        }
    }
}

fn api_error(status: u16, text: &str) -> Error {
    let parsed: Value = serde_json::from_str(text).unwrap_or_default();
    let kind = parsed["error"]["type"].as_str().unwrap_or("api_error").to_string();
    let message = parsed["error"]["message"].as_str().unwrap_or(text).to_string();
    match status {
        401 | 403 => Error::Unauthenticated,
        _ => Error::Api { status, kind, message, retryable: matches!(status, 408 | 429 | 500 | 502 | 503 | 529) },
    }
}

/// Maps one SSE event to its chunks; most frames give one, message_delta carries usage and the stop.
fn chunks(event: &str, data: &str) -> Result<Vec<Chunk>, Error> {
    let value: Value = serde_json::from_str(data).map_err(|e| Error::Malformed(e.to_string()))?;
    let chunk = match event {
        "message_start" => Chunk::Usage(usage(&value["message"]["usage"])),
        "content_block_start" => block_start(&value["content_block"])?,
        "content_block_delta" => block_delta(&value["delta"])?,
        "content_block_stop" => Chunk::BlockStop,
        "message_delta" => return Ok(message_delta(&value)),
        "error" => return Err(api_error(200, data)),
        _ => return Ok(Vec::new()),
    };
    Ok(vec![chunk])
}

fn block_start(block: &Value) -> Result<Chunk, Error> {
    Ok(match block["type"].as_str().unwrap_or_default() {
        "text" => Chunk::TextStart,
        "thinking" => Chunk::ReasoningStart,
        "redacted_thinking" => Chunk::ReasoningRedacted(block["data"].as_str().unwrap_or_default().into()),
        "tool_use" => Chunk::ToolUseStart {
            id: block["id"].as_str().unwrap_or_default().into(),
            name: block["name"].as_str().unwrap_or_default().into(),
        },
        other => return Err(Error::Malformed(format!("unknown content block {other}"))),
    })
}

fn block_delta(delta: &Value) -> Result<Chunk, Error> {
    let text = |key: &str| delta[key].as_str().unwrap_or_default().to_string();
    Ok(match delta["type"].as_str().unwrap_or_default() {
        "text_delta" => Chunk::TextDelta(text("text")),
        "thinking_delta" => Chunk::ReasoningDelta(text("thinking")),
        "signature_delta" => Chunk::ReasoningSignature(text("signature")),
        "input_json_delta" => Chunk::ToolInputDelta(text("partial_json")),
        other => return Err(Error::Malformed(format!("unknown delta {other}"))),
    })
}

fn message_delta(value: &Value) -> Vec<Chunk> {
    let mut out = vec![Chunk::Usage(usage(&value["usage"]))];
    if let Some(reason) = value["delta"]["stop_reason"].as_str() {
        out.push(Chunk::Stop(match reason {
            "end_turn" | "stop_sequence" => StopReason::EndTurn,
            "tool_use" => StopReason::ToolUse,
            "max_tokens" => StopReason::MaxTokens,
            _ => StopReason::Other,
        }));
    }
    out
}

fn usage(value: &Value) -> Usage {
    let count = |key: &str| value[key].as_u64().unwrap_or(0);
    Usage {
        input: count("input_tokens"),
        output: count("output_tokens"),
        cache_read: count("cache_read_input_tokens"),
        cache_write: count("cache_creation_input_tokens"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolSpec;

    fn request() -> Request {
        Request {
            model: "claude-sonnet-4-5".into(),
            system: "You are Drift.".into(),
            messages: vec![
                ChatMessage { role: Role::User, blocks: vec![Block::Text("hi".into())] },
                ChatMessage {
                    role: Role::Assistant,
                    blocks: vec![
                        Block::Reasoning { text: "think".into(), signature: Some("sig".into()), redacted: None },
                        Block::ToolUse { id: "toolu_1".into(), name: "read".into(), input: json!({ "path": "a" }) },
                    ],
                },
                ChatMessage {
                    role: Role::User,
                    blocks: vec![Block::ToolResult { call_id: "toolu_1".into(), content: "ok".into(), is_error: false }],
                },
            ],
            tools: vec![ToolSpec { name: "read".into(), description: "Reads".into(), input_schema: json!({ "type": "object" }) }],
            max_tokens: 1000,
            thinking_budget: Some(2048),
            temperature: Some(0.5),
        }
    }

    #[test]
    fn body_matches_the_messages_api() {
        let body = body(&request());
        assert_eq!(body["stream"], true);
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(body["tools"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(body["thinking"]["budget_tokens"], 2048);
        assert!(body.get("temperature").is_none(), "temperature is dropped when thinking is on");
        assert_eq!(body["messages"][1]["content"][0]["type"], "thinking");
        assert_eq!(body["messages"][1]["content"][0]["signature"], "sig");
        assert_eq!(body["messages"][1]["content"][1]["id"], "toolu_1");
        assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], "toolu_1");
    }

    fn breakpoints(body: &Value) -> usize {
        body.to_string().matches("\"cache_control\"").count()
    }

    fn marked(body: &Value, index: usize) -> bool {
        body["messages"][index]["content"].as_array().unwrap().last().unwrap().get("cache_control").is_some()
    }

    #[test]
    fn the_last_two_user_messages_carry_the_conversation_breakpoints() {
        let mut first = request();
        first.messages.truncate(1);
        let first = body(&first);
        assert!(marked(&first, 0));
        assert_eq!(breakpoints(&first), 3, "system, tools and the only user message");

        let mut next = request();
        next.messages.push(ChatMessage { role: Role::Assistant, blocks: vec![Block::ToolUse { id: "toolu_2".into(), name: "read".into(), input: json!({}) }] });
        next.messages.push(ChatMessage { role: Role::User, blocks: vec![Block::ToolResult { call_id: "toolu_2".into(), content: "ok".into(), is_error: false }] });
        let previous = body(&request());
        let next = body(&next);
        assert_eq!(breakpoints(&next), 4, "never more than Anthropic allows");
        assert!(marked(&next, 4) && marked(&next, 2) && !marked(&next, 0) && !marked(&next, 1) && !marked(&next, 3));
        assert!(marked(&previous, 2), "the step before wrote at the block this one reads from");
        let unmarked = |body: &Value| body["messages"].to_string().replace(r#""cache_control":{"type":"ephemeral"},"#, "");
        assert!(unmarked(&next).starts_with(unmarked(&previous).trim_end_matches(']')), "and the content up to it is unchanged");
    }

    #[test]
    fn breakpoints_skip_blocks_that_cannot_carry_one_and_survive_the_subscription_shape() {
        let mut request = request();
        request.messages[0].blocks = vec![Block::Text("look".into()), Block::Text(String::new())];
        let mut body = body(&request);
        assert!(body["messages"][0]["content"][0].get("cache_control").is_some(), "empty text is skipped for the block before it");
        assert!(body["messages"][0]["content"][1].get("cache_control").is_none());
        claude_code::transform(&mut body);
        assert_eq!(breakpoints(&body), 4);
        assert!(marked(&body, 2));
    }

    #[test]
    fn temperature_applies_without_thinking() {
        let mut request = request();
        request.thinking_budget = None;
        assert_eq!(body(&request)["temperature"], 0.5);
    }

    #[test]
    fn stream_events_map_to_chunks() {
        let cases = [
            ("message_start", r#"{"message":{"usage":{"input_tokens":10,"cache_read_input_tokens":4}}}"#, Some(Chunk::Usage(Usage { input: 10, output: 0, cache_read: 4, cache_write: 0 }))),
            ("content_block_start", r#"{"content_block":{"type":"tool_use","id":"t1","name":"read"}}"#, Some(Chunk::ToolUseStart { id: "t1".into(), name: "read".into() })),
            ("content_block_delta", r#"{"delta":{"type":"input_json_delta","partial_json":"{\"pa"}}"#, Some(Chunk::ToolInputDelta("{\"pa".into()))),
            ("content_block_delta", r#"{"delta":{"type":"signature_delta","signature":"s"}}"#, Some(Chunk::ReasoningSignature("s".into()))),
            ("content_block_start", r#"{"content_block":{"type":"redacted_thinking","data":"xyz"}}"#, Some(Chunk::ReasoningRedacted("xyz".into()))),
            ("content_block_stop", r#"{}"#, Some(Chunk::BlockStop)),
            ("message_delta", r#"{"delta":{},"usage":{"output_tokens":7}}"#, Some(Chunk::Usage(Usage { output: 7, ..Usage::default() }))),
            ("ping", r#"{}"#, None),
            ("message_stop", r#"{}"#, None),
        ];
        for (event, data, expected) in cases {
            assert_eq!(chunks(event, data).unwrap().into_iter().next(), expected, "{event}");
        }
        let both = chunks("message_delta", r#"{"delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}}"#).unwrap();
        assert_eq!(both, vec![Chunk::Usage(Usage { output: 7, ..Usage::default() }), Chunk::Stop(StopReason::ToolUse)]);
    }

    #[test]
    fn error_frames_and_statuses_classify() {
        let error = chunks("error", r#"{"error":{"type":"overloaded_error","message":"busy"}}"#).unwrap_err();
        assert!(matches!(error, Error::Api { kind, .. } if kind == "overloaded_error"));
        assert!(matches!(api_error(429, "{}"), Error::Api { retryable: true, .. }));
        assert!(matches!(api_error(400, "{}"), Error::Api { retryable: false, .. }));
        assert!(matches!(api_error(401, "{}"), Error::Unauthenticated));
    }
}
