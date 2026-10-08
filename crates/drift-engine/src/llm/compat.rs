//! OpenAI Chat Completions over SSE: the dialect xAI, Z.ai, OpenRouter, LM Studio and Ollama all speak.

use std::collections::BTreeMap;

use futures_util::StreamExt;
use serde_json::{Value, json};

#[cfg(test)]
use super::catalog::Reasoning;
use super::sse;
#[cfg(test)]
use super::{Block, ChatMessage, Role};
use super::{Chunk, ChunkStream, Credential, Error, Request, StopReason};
use crate::session::types::Usage;

#[cfg(test)]
mod gateway_tests;
mod request;

#[cfg(test)]
use request::message;
use request::{body, is_claude, mark_breakpoints, reason};

#[derive(Clone, Debug)]
pub struct Compat {
    base_url: String,
    client: reqwest::Client,
    pub timeouts: super::http::Timeouts,
    /// The gateway passes Anthropic's per-block `cache_control` through to Claude (OpenRouter does).
    claude_breakpoints: bool,
    /// Reasoning goes as OpenRouter's `reasoning` object, which takes a budget too, rather than `reasoning_effort`.
    reasoning_object: bool,
    /// Z.ai drops earlier reasoning unless told `clear_thinking: false`, as opencode sends.
    keep_thinking: bool,
}

impl Compat {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: super::http::client(),
            timeouts: super::http::Timeouts::default(),
            claude_breakpoints: false,
            reasoning_object: false,
            keep_thinking: false,
        }
    }

    /// OpenRouter: forwards `cache_control` on content blocks to Claude, and takes reasoning as an object.
    pub fn openrouter(base_url: &str) -> Self {
        Self {
            claude_breakpoints: true,
            reasoning_object: true,
            ..Self::new(base_url)
        }
    }

    /// Z.ai: thinking on, with the reasoning of the turn's earlier steps kept.
    pub fn zai(base_url: &str) -> Self {
        Self {
            keep_thinking: true,
            ..Self::new(base_url)
        }
    }

    /// The request body as this route takes it.
    fn shaped(&self, request: &Request) -> Value {
        let mut body = body(request);
        reason(&mut body, request.reasoning.as_ref(), self.reasoning_object);
        if self.keep_thinking {
            body["thinking"] = json!({ "type": "enabled", "clear_thinking": false });
        }
        if self.claude_breakpoints && is_claude(&request.model) {
            mark_breakpoints(&mut body);
        }

        super::apply_mode(&mut body, request);

        body
    }

    pub async fn stream(&self, request: &Request, credential: &Credential) -> Result<ChunkStream, Error> {
        let key = match credential {
            Credential::ApiKey { key } => key.clone(),
            Credential::OAuth { access, .. } => access.clone(),
            Credential::Ambient { .. } => return Err(Error::Unauthenticated(String::new())),
        };

        let body = self.shaped(request);
        let http = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(key)
            .header("accept", "text/event-stream");
        let sending = super::mode_headers(http, request, Vec::new()).json(&body);

        let response = super::http::send(sending, &self.timeouts).await?;
        let status = response.status();
        if !status.is_success() {
            let headers = response.headers().clone();
            let text = super::http::bounded_body(response, &self.timeouts).await;

            return Err(api_error(status.as_u16(), &text).with_headers(&headers));
        }

        let mut state = StreamState::default();
        let events = sse::events(response.bytes_stream(), self.timeouts.idle);
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

/// Gateways put an HTTP code inside a streamed error object; it classifies like the status it names.
fn api_error(status: u16, text: &str) -> Error {
    let parsed: Value = serde_json::from_str(text).unwrap_or_default();
    let error = if parsed["error"].is_object() {
        &parsed["error"]
    } else {
        &parsed
    };
    let kind = error["type"]
        .as_str()
        .or(error["code"].as_str())
        .unwrap_or("api_error")
        .to_string();
    let message = error["message"].as_str().unwrap_or(text).to_string();
    let named = error["code"]
        .as_u64()
        .and_then(|code| u16::try_from(code).ok())
        .filter(|_| status == super::STREAMED);
    match named.unwrap_or(status) {
        401 | 403 => Error::Unauthenticated(message),
        status => Error::api(status, kind, message),
    }
}

/// Text and reasoning stream as they come; tool calls interleave by `index`, so each is gathered
/// whole and handed on at the end, in index order.
#[derive(Default)]
struct StreamState {
    open: Option<Open>,
    calls: BTreeMap<u64, Call>,
    usage: Option<Usage>,
    finish: Option<StopReason>,
}

#[derive(Default)]
struct Call {
    id: String,
    name: String,
    arguments: String,
}

#[derive(PartialEq)]
enum Open {
    Text,
    Reasoning,
}

impl StreamState {
    fn chunks(&mut self, data: &str) -> Result<Vec<Chunk>, Error> {
        if data.trim() == "[DONE]" {
            return self.done();
        }

        let value: Value = serde_json::from_str(data).map_err(|e| Error::Malformed(e.to_string()))?;
        if value["error"].is_object() {
            return Err(api_error(super::STREAMED, data));
        }

        if let Some(usage) = value.get("usage").filter(|u| u.is_object()) {
            self.usage = Some(usage_from(usage));
        }
        let mut out = Vec::new();
        let Some(choice) = value["choices"].get(0) else {
            return Ok(out);
        };

        let delta = &choice["delta"];
        if let Some(text) = delta["reasoning_content"]
            .as_str()
            .or(delta["reasoning"].as_str())
            .filter(|t| !t.is_empty())
        {
            out.extend(self.switch(Open::Reasoning, Chunk::ReasoningStart));
            out.push(Chunk::ReasoningDelta(text.into()));
        }

        if let Some(text) = delta["content"].as_str().filter(|t| !t.is_empty()) {
            out.extend(self.switch(Open::Text, Chunk::TextStart));
            out.push(Chunk::TextDelta(text.into()));
        }

        for call in delta["tool_calls"].as_array().into_iter().flatten() {
            self.tool_delta(call);
        }

        if let Some(reason) = choice["finish_reason"].as_str() {
            self.finish = Some(match reason {
                "tool_calls" | "function_call" => StopReason::ToolUse,
                "length" => StopReason::MaxTokens,
                "stop" => StopReason::EndTurn,
                "content_filter" => StopReason::Refused,
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

    /// Adds to the call at this index: its id and name from whichever delta carries them, its arguments in order.
    fn tool_delta(&mut self, delta: &Value) {
        let index = delta["index"].as_u64().unwrap_or(0);
        let call = self.calls.entry(index).or_default();
        if let Some(id) = delta["id"].as_str().filter(|id| !id.is_empty()) {
            call.id = id.to_string();
        }
        if let Some(name) = delta["function"]["name"].as_str() {
            call.name.push_str(name);
        }
        if let Some(arguments) = delta["function"]["arguments"].as_str() {
            call.arguments.push_str(arguments);
        }
    }

    /// `[DONE]` closes the reply; one that never said why it finished is broken, and its calls are not trusted.
    fn done(&mut self) -> Result<Vec<Chunk>, Error> {
        let Some(finish) = self.finish.take() else {
            return Err(Error::Transport("the stream ended without a finish reason".into()));
        };

        let mut out = Vec::new();
        if self.open.take().is_some() {
            out.push(Chunk::BlockStop);
        }

        let calls = std::mem::take(&mut self.calls);
        let called = !calls.is_empty();
        for (_, call) in calls {
            let id = if call.id.is_empty() {
                crate::id::new("call")
            } else {
                call.id
            };
            out.push(Chunk::ToolUseStart { id, name: call.name });
            if !call.arguments.is_empty() {
                out.push(Chunk::ToolInputDelta(call.arguments));
            }
            out.push(Chunk::BlockStop);
        }

        if let Some(usage) = self.usage.take() {
            out.push(Chunk::Usage(usage));
        }
        let stop = match finish {
            StopReason::EndTurn if called => StopReason::ToolUse,
            reason => reason,
        };
        out.push(Chunk::Stop(stop));

        Ok(out)
    }
}

fn usage_from(usage: &Value) -> Usage {
    let count = |key: &str| usage[key].as_u64().unwrap_or(0);
    let details = &usage["prompt_tokens_details"];
    let cache_read = details["cached_tokens"].as_u64().unwrap_or(0);
    let cache_write = details["cache_write_tokens"].as_u64().unwrap_or(0);
    Usage {
        input: count("prompt_tokens").saturating_sub(cache_read + cache_write),
        output: count("completion_tokens"),
        cache_read,
        cache_write,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolSpec;

    #[test]
    fn a_pdf_is_a_file_part() {
        let sent = message(&ChatMessage {
            role: Role::User,
            blocks: vec![Block::Pdf {
                base64: "JVBERi0=".into(),
            }],
        });
        assert_eq!(
            sent[0]["content"][0]["file"]["file_data"],
            "data:application/pdf;base64,JVBERi0="
        );
    }

    #[test]
    fn zai_keeps_thinking_and_tuned_sampling_is_sent() {
        let tuned = Request {
            temperature: Some(1.0),
            top_p: Some(0.95),
            top_k: Some(40),
            ..request()
        };
        let zai = Compat::zai("https://z.example").shaped(&tuned);
        assert_eq!(zai["thinking"], json!({ "type": "enabled", "clear_thinking": false }));
        assert_eq!(
            (zai["temperature"].as_f64(), zai["top_p"].as_f64()),
            (Some(1.0), Some(0.95))
        );
        assert!(zai.get("top_k").is_none(), "not a Chat Completions field");
        assert!(
            Compat::new("https://other.example")
                .shaped(&tuned)
                .get("thinking")
                .is_none()
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
            (json!("none"), true)
        );
    }

    pub(super) fn request() -> Request {
        Request {
            model: "grok-4.5".into(),
            system: "sys".into(),
            messages: vec![
                ChatMessage {
                    role: Role::User,
                    blocks: vec![Block::Text("hi".into())],
                },
                ChatMessage {
                    role: Role::Assistant,
                    blocks: vec![
                        Block::Reasoning {
                            text: "hm".into(),
                            signature: None,
                            redacted: None,
                        },
                        Block::Text("ok".into()),
                        Block::ToolUse {
                            id: "call_1".into(),
                            name: "read".into(),
                            input: json!({ "path": "a" }),
                        },
                    ],
                },
                ChatMessage {
                    role: Role::User,
                    blocks: vec![Block::ToolResult {
                        call_id: "call_1".into(),
                        content: "1: x".into(),
                        is_error: false,
                    }],
                },
            ],
            tools: vec![ToolSpec {
                name: "read".into(),
                description: "r".into(),
                input_schema: json!({ "type": "object" }),
            }],
            max_tokens: 500,
            reasoning: None,
            temperature: Some(0.2),
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
    fn reasoning_goes_as_each_preset_takes_it() {
        let reasoned = |reasoning, object| {
            let mut body = json!({});
            reason(&mut body, Some(&reasoning), object);
            body
        };
        let effort = || Reasoning::Effort { level: "high".into() };
        assert_eq!(reasoned(effort(), false), json!({ "reasoning_effort": "high" }));
        assert_eq!(
            reasoned(effort(), true),
            json!({ "reasoning": { "effort": "high" } }),
            "OpenRouter"
        );
        assert_eq!(
            reasoned(Reasoning::Budget { tokens: 8000 }, true),
            json!({ "reasoning": { "max_tokens": 8000 } })
        );
        assert_eq!(
            reasoned(Reasoning::Budget { tokens: 8000 }, false),
            json!({}),
            "no budget field to carry it"
        );
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
        assert_eq!(
            feed(
                &mut state,
                r#"{"choices":[{"delta":{"role":"assistant","content":""}}]}"#
            ),
            vec![]
        );
        assert_eq!(
            feed(&mut state, r#"{"choices":[{"delta":{"reasoning_content":"th"}}]}"#),
            vec![Chunk::ReasoningStart, Chunk::ReasoningDelta("th".into())]
        );
        assert_eq!(
            feed(&mut state, r#"{"choices":[{"delta":{"content":"Hi"}}]}"#),
            vec![Chunk::BlockStop, Chunk::TextStart, Chunk::TextDelta("Hi".into())]
        );
        assert_eq!(
            feed(&mut state, r#"{"choices":[{"delta":{"content":"!"}}]}"#),
            vec![Chunk::TextDelta("!".into())]
        );
        assert_eq!(
            feed(
                &mut state,
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"read","arguments":""}}]}}]}"#
            ),
            vec![]
        );
        assert_eq!(
            feed(
                &mut state,
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"p\":1}"}}]}}]}"#
            ),
            vec![]
        );
        assert_eq!(
            feed(&mut state, r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#),
            vec![]
        );
        assert_eq!(
            feed(
                &mut state,
                r#"{"choices":[],"usage":{"prompt_tokens":50,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":20}}}"#
            ),
            vec![]
        );
        assert_eq!(
            feed(&mut state, "[DONE]"),
            vec![
                Chunk::BlockStop,
                Chunk::ToolUseStart {
                    id: "call_a".into(),
                    name: "read".into()
                },
                Chunk::ToolInputDelta("{\"p\":1}".into()),
                Chunk::BlockStop,
                Chunk::Usage(Usage {
                    input: 30,
                    output: 7,
                    cache_read: 20,
                    cache_write: 0
                }),
                Chunk::Stop(StopReason::ToolUse)
            ]
        );
    }

    #[test]
    fn interleaved_calls_are_put_together_by_index() {
        let mut state = StreamState::default();
        for delta in [
            r#"{"index":0,"id":"call_a","function":{"name":"read","arguments":"{\"path\":"}}"#,
            r#"{"index":1,"id":"call_b","function":{"name":"grep","arguments":"{\"pattern\":"}}"#,
            r#"{"index":0,"function":{"arguments":"\"a.txt\"}"}}"#,
            r#"{"index":1,"function":{"arguments":"\"x\"}"}}"#,
        ] {
            assert!(
                state
                    .chunks(&format!(r#"{{"choices":[{{"delta":{{"tool_calls":[{delta}]}}}}]}}"#))
                    .unwrap()
                    .is_empty()
            );
        }
        state
            .chunks(r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#)
            .unwrap();
        let out = state.chunks("[DONE]").unwrap();
        let starts: Vec<&Chunk> = out.iter().filter(|c| matches!(c, Chunk::ToolUseStart { .. })).collect();
        assert_eq!(starts.len(), 2, "one start per call: {out:?}");
        assert!(
            out.contains(&Chunk::ToolInputDelta("{\"path\":\"a.txt\"}".into()))
                && out.contains(&Chunk::ToolInputDelta("{\"pattern\":\"x\"}".into()))
        );
    }

    #[test]
    fn a_stream_that_never_says_why_it_finished_is_refused_with_its_calls() {
        let mut state = StreamState::default();
        state.chunks(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"bash","arguments":"{\"command\":\"rm -rf"}}]}}]}"#).unwrap();
        let error = state.chunks("[DONE]").unwrap_err();
        assert!(error.to_string().contains("without a finish reason"), "{error}");
    }

    #[test]
    fn errors_and_length_stops_classify() {
        assert!(matches!(
            StreamState::default().chunks(r#"{"error":{"message":"nope","code":"bad"}}"#),
            Err(Error::Api { retryable: false, .. })
        ));
        let gateway = StreamState::default().chunks(
            r#"{"error":{"message":"Provider returned error","code":502},"choices":[{"finish_reason":"error"}]}"#,
        );
        assert!(
            matches!(
                gateway,
                Err(Error::Api {
                    status: 502,
                    retryable: true,
                    ..
                })
            ),
            "a gateway's streamed 502 retries: {gateway:?}"
        );
        let overloaded = StreamState::default().chunks(r#"{"error":{"message":"busy","type":"overloaded_error"}}"#);
        assert!(
            matches!(overloaded, Err(Error::Api { retryable: true, .. })),
            "an upstream overload passed through retries"
        );
        assert!(
            matches!(StreamState::default().chunks(r#"{"error":{"message":"key","code":401}}"#), Err(Error::Unauthenticated(ref m)) if m == "key")
        );
        let mut state = StreamState::default();
        state
            .chunks(r#"{"choices":[{"delta":{"content":"x"},"finish_reason":"length"}]}"#)
            .unwrap();
        assert!(state.done().unwrap().contains(&Chunk::Stop(StopReason::MaxTokens)));
        assert!(matches!(api_error(429, "{}"), Error::Api { retryable: true, .. }));
    }
}
