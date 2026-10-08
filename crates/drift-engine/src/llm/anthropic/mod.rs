//! Anthropic Messages API over SSE; its body and event mapping also serve Bedrock and Vertex.

pub mod claude_code;
pub mod oauth;
mod request;

use futures_util::StreamExt;
use serde_json::{Value, json};

use super::catalog::Reasoning;
use super::sse;
#[cfg(test)]
use super::{Block, ChatMessage, Role};
use super::{Chunk, ChunkStream, Credential, Error, Request, StopReason};
use crate::session::types::Usage;
#[cfg(test)]
use request::block;
use request::body;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";

#[derive(Clone, Debug)]
pub struct Anthropic {
    pub base_url: String,
    client: reqwest::Client,
    pub timeouts: super::http::Timeouts,
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
            client: super::http::client(),
            timeouts: super::http::Timeouts::default(),
        }
    }

    /// Subscription tokens only work for requests shaped like Claude Code's; keys take the plain path.
    pub async fn stream(&self, request: &Request, credential: &Credential) -> Result<ChunkStream, Error> {
        let mut body = body(request);
        let subscription = matches!(credential, Credential::OAuth { .. });
        let query = if subscription { "?beta=true" } else { "" };
        let url = format!("{}/v1/messages{query}", self.base_url);

        let http = self
            .client
            .post(url)
            .header("anthropic-version", API_VERSION)
            .header("accept", "text/event-stream");

        let (http, betas) = match credential {
            Credential::ApiKey { key } => (
                http.header("x-api-key", key),
                if interleaves(request) {
                    vec![INTERLEAVED_THINKING]
                } else {
                    Vec::new()
                },
            ),
            Credential::OAuth { access, .. } => {
                claude_code::transform(&mut body);
                (
                    http.bearer_auth(access).header("user-agent", claude_code::user_agent()),
                    claude_code::BETAS.split(',').collect(),
                )
            }
            Credential::Ambient { .. } => return Err(Error::Unauthenticated(String::new())),
        };

        stream_from(
            super::mode_headers(http, request, betas).json(&body),
            &self.timeouts,
            subscription,
        )
        .await
    }
}

/// Lets a model with a thinking budget think again between tool calls, not only before the first.
pub(super) const INTERLEAVED_THINKING: &str = "interleaved-thinking-2025-05-14";

/// Only a budget needs the beta: adaptive thinking interleaves already, and without tools there is nothing between.
pub(super) fn interleaves(request: &Request) -> bool {
    matches!(request.reasoning, Some(Reasoning::Budget { .. })) && !request.tools.is_empty()
}

/// Sends a Messages request already addressed, authorised and given its body (the API or Vertex) and reads its events.
pub(super) async fn stream_from(
    http: reqwest::RequestBuilder,
    timeouts: &super::http::Timeouts,
    subscription: bool,
) -> Result<ChunkStream, Error> {
    let response = super::http::send(http, timeouts).await?;
    let status = response.status();
    if !status.is_success() {
        let headers = response.headers().clone();
        let text = super::http::bounded_body(response, timeouts).await;

        return Err(api_error(status.as_u16(), &text).with_headers(&headers));
    }

    let events = sse::events(response.bytes_stream(), timeouts.idle);
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

/// The Messages body for a cloud route: the model goes in the URL and the API version in the body.
pub(super) fn cloud_body(request: &Request, version: &str, stream: bool) -> Value {
    let mut body = body(request);
    if let Some(fields) = body.as_object_mut() {
        fields.remove("model");
        if !stream {
            fields.remove("stream");
        }
        fields.insert("anthropic_version".into(), json!(version));
    }

    body
}

fn unprefix(chunk: Chunk, subscription: bool) -> Chunk {
    match chunk {
        Chunk::ToolUseStart { id, name } if subscription => Chunk::ToolUseStart {
            id,
            name: claude_code::original_name(&name),
        },
        other => other,
    }
}

pub(super) fn api_error(status: u16, text: &str) -> Error {
    let parsed: Value = serde_json::from_str(text).unwrap_or_default();
    let kind = parsed["error"]["type"].as_str().unwrap_or("api_error").to_string();
    let message = parsed["error"]["message"].as_str().unwrap_or(text).to_string();

    match status {
        401 | 403 => Error::Unauthenticated(message),
        _ => Error::api(status, kind, message),
    }
}

/// Maps one SSE event to its chunks; most frames give one, message_delta carries usage and the stop.
pub(super) fn chunks(event: &str, data: &str) -> Result<Vec<Chunk>, Error> {
    let value: Value = serde_json::from_str(data).map_err(|e| Error::Malformed(e.to_string()))?;

    let chunk = match event {
        "message_start" => Some(Chunk::Usage(usage(&value["message"]["usage"]))),
        "content_block_start" => block_start(&value["content_block"]),
        "content_block_delta" => block_delta(&value["delta"]),
        "content_block_stop" => Some(Chunk::BlockStop),
        "message_delta" => return Ok(message_delta(&value)),
        "error" => return Err(api_error(super::STREAMED, data)),
        _ => None,
    };

    Ok(chunk.into_iter().collect())
}

/// Skips unknown block kinds such as server tools and citations.
/// Without an open block, their deltas and stop frames produce no content.
fn block_start(block: &Value) -> Option<Chunk> {
    Some(match block["type"].as_str().unwrap_or_default() {
        "text" => Chunk::TextStart,
        "thinking" => Chunk::ReasoningStart,
        "redacted_thinking" => Chunk::ReasoningRedacted(block["data"].as_str().unwrap_or_default().into()),
        "tool_use" => Chunk::ToolUseStart {
            id: block["id"].as_str().unwrap_or_default().into(),
            name: block["name"].as_str().unwrap_or_default().into(),
        },
        _ => return None,
    })
}

fn block_delta(delta: &Value) -> Option<Chunk> {
    let text = |key: &str| delta[key].as_str().unwrap_or_default().to_string();
    Some(match delta["type"].as_str().unwrap_or_default() {
        "text_delta" => Chunk::TextDelta(text("text")),
        "thinking_delta" => Chunk::ReasoningDelta(text("thinking")),
        "signature_delta" => Chunk::ReasoningSignature(text("signature")),
        "input_json_delta" => Chunk::ToolInputDelta(text("partial_json")),
        _ => return None,
    })
}

fn message_delta(value: &Value) -> Vec<Chunk> {
    let mut out = vec![Chunk::Usage(usage(&value["usage"]))];
    if let Some(reason) = value["delta"]["stop_reason"].as_str() {
        out.push(Chunk::Stop(match reason {
            "end_turn" | "stop_sequence" => StopReason::EndTurn,
            "tool_use" => StopReason::ToolUse,
            "max_tokens" => StopReason::MaxTokens,
            "refusal" => StopReason::Refused,
            "model_context_window_exceeded" => StopReason::ContextFull,
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

    #[test]
    fn a_pdf_is_a_base64_document() {
        let sent = block(&Block::Pdf {
            base64: "JVBERi0=".into(),
        });
        assert_eq!(
            sent,
            json!({ "type": "document", "source": { "type": "base64", "media_type": "application/pdf", "data": "JVBERi0=" } })
        );
    }

    #[test]
    fn call_ids_from_other_providers_are_made_acceptable_on_both_sides() {
        let request = Request {
            messages: vec![
                ChatMessage {
                    role: Role::Assistant,
                    blocks: vec![Block::ToolUse {
                        id: "functions.read:0".into(),
                        name: "read".into(),
                        input: json!({}),
                    }],
                },
                ChatMessage {
                    role: Role::User,
                    blocks: vec![Block::ToolResult {
                        call_id: "functions.read:0".into(),
                        content: "ok".into(),
                        is_error: false,
                    }],
                },
            ],
            ..request()
        };
        let built = body(&request);
        let (used, answered) = (
            &built["messages"][0]["content"][0]["id"],
            &built["messages"][1]["content"][0]["tool_use_id"],
        );
        assert_eq!(
            (used.as_str(), answered.as_str()),
            (Some("functions_read_0"), Some("functions_read_0"))
        );
    }

    #[test]
    fn a_text_only_request_keeps_its_tools_but_forbids_calls() {
        assert!(body(&request()).get("tool_choice").is_none());
        let built = body(&Request {
            no_tool_calls: true,
            ..request()
        });
        assert_eq!(
            (
                built["tool_choice"].clone(),
                built["tools"].as_array().is_some_and(|tools| !tools.is_empty())
            ),
            (json!({ "type": "none" }), true)
        );
    }

    fn request() -> Request {
        Request {
            model: "claude-sonnet-4-5".into(),
            system: "You are Drift.".into(),
            messages: vec![
                ChatMessage {
                    role: Role::User,
                    blocks: vec![Block::Text("hi".into())],
                },
                ChatMessage {
                    role: Role::Assistant,
                    blocks: vec![
                        Block::Reasoning {
                            text: "think".into(),
                            signature: Some("sig".into()),
                            redacted: None,
                        },
                        Block::ToolUse {
                            id: "toolu_1".into(),
                            name: "read".into(),
                            input: json!({ "path": "a" }),
                        },
                    ],
                },
                ChatMessage {
                    role: Role::User,
                    blocks: vec![Block::ToolResult {
                        call_id: "toolu_1".into(),
                        content: "ok".into(),
                        is_error: false,
                    }],
                },
            ],
            tools: vec![ToolSpec {
                name: "read".into(),
                description: "Reads".into(),
                input_schema: json!({ "type": "object" }),
            }],
            max_tokens: 1000,
            reasoning: Some(Reasoning::Budget { tokens: 2048 }),
            temperature: Some(0.5),
            cache_key: None,
            no_tool_calls: false,
            verbosity: None,
            show_thinking: false,
            top_p: None,
            top_k: None,
            mode: None,
        }
    }

    #[test]
    fn body_matches_the_messages_api() {
        let body = body(&request());
        assert_eq!(body["stream"], true);
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(body["tools"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(body["thinking"]["budget_tokens"], 2048);
        assert!(
            body.get("temperature").is_none(),
            "temperature is dropped when thinking is on"
        );
        assert_eq!(body["messages"][1]["content"][0]["type"], "thinking");
        assert_eq!(body["messages"][1]["content"][0]["signature"], "sig");
        assert_eq!(body["messages"][1]["content"][1]["id"], "toolu_1");
        assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], "toolu_1");
    }

    #[test]
    fn unsigned_thinking_is_never_sent() {
        let mut request = request();
        request.messages[1].blocks[0] = Block::Reasoning {
            text: "think".into(),
            signature: None,
            redacted: None,
        };
        assert_eq!(body(&request)["messages"][1]["content"][0]["type"], "tool_use");
    }

    fn breakpoints(body: &Value) -> usize {
        body.to_string().matches("\"cache_control\"").count()
    }

    fn marked(body: &Value, index: usize) -> bool {
        body["messages"][index]["content"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .get("cache_control")
            .is_some()
    }

    #[test]
    fn the_last_two_user_messages_carry_the_conversation_breakpoints() {
        let mut first = request();
        first.messages.truncate(1);
        let first = body(&first);
        assert!(marked(&first, 0));
        assert_eq!(breakpoints(&first), 3, "system, tools and the only user message");

        let mut next = request();
        next.messages.push(ChatMessage {
            role: Role::Assistant,
            blocks: vec![Block::ToolUse {
                id: "toolu_2".into(),
                name: "read".into(),
                input: json!({}),
            }],
        });
        next.messages.push(ChatMessage {
            role: Role::User,
            blocks: vec![Block::ToolResult {
                call_id: "toolu_2".into(),
                content: "ok".into(),
                is_error: false,
            }],
        });
        let previous = body(&request());
        let next = body(&next);
        assert_eq!(breakpoints(&next), 4, "never more than Anthropic allows");
        assert!(marked(&next, 4) && marked(&next, 2) && !marked(&next, 0) && !marked(&next, 1) && !marked(&next, 3));
        assert!(
            marked(&previous, 2),
            "the step before wrote at the block this one reads from"
        );
        let unmarked = |body: &Value| {
            body["messages"]
                .to_string()
                .replace(r#""cache_control":{"type":"ephemeral"},"#, "")
        };
        assert!(
            unmarked(&next).starts_with(unmarked(&previous).trim_end_matches(']')),
            "and the content up to it is unchanged"
        );
    }

    #[test]
    fn breakpoints_skip_blocks_that_cannot_carry_one_and_survive_the_subscription_shape() {
        let mut request = request();
        request.messages[0].blocks = vec![Block::Text("look".into()), Block::Text(String::new())];
        let mut body = body(&request);
        assert!(
            body["messages"][0]["content"][0].get("cache_control").is_some(),
            "empty text is skipped for the block before it"
        );
        assert!(body["messages"][0]["content"][1].get("cache_control").is_none());
        claude_code::transform(&mut body);
        assert_eq!(breakpoints(&body), 4);
        assert!(marked(&body, 2));
    }

    #[test]
    fn temperature_applies_without_thinking() {
        let mut request = request();
        request.reasoning = None;
        assert_eq!(body(&request)["temperature"], 0.5);
    }

    #[test]
    fn an_effort_asks_for_adaptive_thinking_with_its_summary() {
        let mut request = request();
        request.reasoning = Some(Reasoning::Effort { level: "xhigh".into() });
        let body = body(&request);
        assert_eq!(body["thinking"], json!({ "type": "adaptive", "display": "summarized" }));
        assert_eq!(body["output_config"]["effort"], "xhigh");
        assert!(body["thinking"].get("budget_tokens").is_none() && body.get("temperature").is_none());
    }

    #[test]
    fn stream_events_map_to_chunks() {
        let cases = [
            (
                "message_start",
                r#"{"message":{"usage":{"input_tokens":10,"cache_read_input_tokens":4}}}"#,
                Some(Chunk::Usage(Usage {
                    input: 10,
                    output: 0,
                    cache_read: 4,
                    cache_write: 0,
                })),
            ),
            (
                "content_block_start",
                r#"{"content_block":{"type":"tool_use","id":"t1","name":"read"}}"#,
                Some(Chunk::ToolUseStart {
                    id: "t1".into(),
                    name: "read".into(),
                }),
            ),
            (
                "content_block_delta",
                r#"{"delta":{"type":"input_json_delta","partial_json":"{\"pa"}}"#,
                Some(Chunk::ToolInputDelta("{\"pa".into())),
            ),
            (
                "content_block_delta",
                r#"{"delta":{"type":"signature_delta","signature":"s"}}"#,
                Some(Chunk::ReasoningSignature("s".into())),
            ),
            (
                "content_block_start",
                r#"{"content_block":{"type":"redacted_thinking","data":"xyz"}}"#,
                Some(Chunk::ReasoningRedacted("xyz".into())),
            ),
            ("content_block_stop", r#"{}"#, Some(Chunk::BlockStop)),
            (
                "message_delta",
                r#"{"delta":{},"usage":{"output_tokens":7}}"#,
                Some(Chunk::Usage(Usage {
                    output: 7,
                    ..Usage::default()
                })),
            ),
            ("ping", r#"{}"#, None),
            ("message_stop", r#"{}"#, None),
            (
                "content_block_start",
                r#"{"content_block":{"type":"server_tool_use","id":"s1","name":"web_search"}}"#,
                None,
            ),
            (
                "content_block_delta",
                r#"{"delta":{"type":"citations_delta","citation":{}}}"#,
                None,
            ),
        ];
        for (event, data, expected) in cases {
            assert_eq!(chunks(event, data).unwrap().into_iter().next(), expected, "{event}");
        }
    }

    #[test]
    fn message_deltas_carry_usage_and_stop_reasons() {
        let both = chunks(
            "message_delta",
            r#"{"delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}}"#,
        )
        .unwrap();
        assert_eq!(
            both,
            vec![
                Chunk::Usage(Usage {
                    output: 7,
                    ..Usage::default()
                }),
                Chunk::Stop(StopReason::ToolUse)
            ]
        );
        let stop = |reason: &str| {
            chunks("message_delta", &format!(r#"{{"delta":{{"stop_reason":"{reason}"}}}}"#))
                .unwrap()
                .pop()
        };
        assert_eq!(stop("refusal"), Some(Chunk::Stop(StopReason::Refused)));
        assert_eq!(
            stop("model_context_window_exceeded"),
            Some(Chunk::Stop(StopReason::ContextFull))
        );
    }

    #[test]
    fn error_frames_and_statuses_classify() {
        let error = chunks(
            "error",
            r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#,
        )
        .unwrap_err();
        assert!(
            matches!(error, Error::Api { ref kind, retryable: true, .. } if kind == "overloaded_error"),
            "an overload mid-stream retries"
        );
        let invalid = chunks(
            "error",
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad"}}"#,
        )
        .unwrap_err();
        assert!(matches!(invalid, Error::Api { retryable: false, .. }));
        assert!(matches!(api_error(429, "{}"), Error::Api { retryable: true, .. }));
        assert!(matches!(api_error(400, "{}"), Error::Api { retryable: false, .. }));
        let expired = api_error(
            401,
            r#"{"type":"error","error":{"type":"authentication_error","message":"OAuth token has expired."}}"#,
        );
        assert!(
            matches!(expired, Error::Unauthenticated(ref m) if m == "OAuth token has expired."),
            "the provider's words are kept: {expired:?}"
        );
    }
}
