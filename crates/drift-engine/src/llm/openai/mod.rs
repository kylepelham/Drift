//! OpenAI Responses over reusable WebSockets, with SSE fallback, for API keys and ChatGPT subscriptions.

pub mod codex;
pub mod oauth;
mod request;
mod stream;
mod websocket;

use futures_util::StreamExt;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;

#[cfg(test)]
use super::catalog::Reasoning;
use super::sse;
#[cfg(test)]
use super::{Block, ChatMessage, Role, StopReason};
use super::{Chunk, ChunkStream, Credential, Error, Request};
#[cfg(test)]
use crate::session::types::Usage;
use request::body;
#[cfg(test)]
use request::items;
use stream::StreamState;

const API_BASE_URL: &str = "https://api.openai.com/v1";
pub(crate) const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
/// The client identity the Codex backend expects on subscription traffic.
const CODEX_ORIGINATOR: &str = "opencode";

#[derive(Clone, Debug)]
pub struct OpenAi {
    base_url: Option<String>,
    client: reqwest::Client,
    pub timeouts: super::http::Timeouts,
    websockets: std::sync::Arc<websocket::Pool>,
}

impl Default for OpenAi {
    fn default() -> Self {
        Self {
            base_url: None,
            client: super::http::client(),
            timeouts: super::http::Timeouts::default(),
            websockets: websocket::Pool::shared(),
        }
    }
}

impl OpenAi {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: Some(base_url.trim_end_matches('/').to_string()),
            ..Self::default()
        }
    }

    /// Streams over the conversation's WebSocket, or over SSE where the endpoint does not take one.
    pub async fn stream(&self, request: &Request, credential: &Credential) -> Result<ChunkStream, Error> {
        let subscription = matches!(credential, Credential::OAuth { .. });
        let http = self.http_request(request, credential)?;

        // The SSE request's URL and headers also authenticate the WebSocket upgrade.
        let handshake = http
            .try_clone()
            .ok_or_else(|| Error::Transport("could not prepare the WebSocket handshake".into()))?
            .build()?;
        let prepared = websocket::Prepared {
            handshake,
            body: body(request, subscription),
            session: request.cache_key.clone(),
            timeouts: self.timeouts,
            subscription,
        };

        let client = super::http::websocket_client();
        if let Some(stream) = self.websockets.stream(&client, &prepared).await? {
            return Ok(stream);
        }

        self.stream_http(http.json(&prepared.body)).await
    }

    /// The authenticated `POST /responses`, with the mode's headers, before its body.
    fn http_request(&self, request: &Request, credential: &Credential) -> Result<reqwest::RequestBuilder, Error> {
        let subscription = matches!(credential, Credential::OAuth { .. });
        let default_base = if subscription { CODEX_BASE_URL } else { API_BASE_URL };
        let base = self.base_url.as_deref().unwrap_or(default_base);

        let mut http = self
            .client
            .post(format!("{base}/responses"))
            .header("accept", "text/event-stream");

        http = match credential {
            Credential::ApiKey { key } => http.bearer_auth(key),
            Credential::OAuth { access, account, .. } => {
                let mut http = http.bearer_auth(access).header("originator", CODEX_ORIGINATOR);
                if let Some(account) = account {
                    http = http.header("chatgpt-account-id", account);
                }
                // Codex groups requests from the same conversation with this session header.
                if let Some(session) = &request.cache_key {
                    http = http.header("session-id", session);
                }
                // An account bound to a region must say so, or the backend refuses it.
                if let Some(residency) = residency(access) {
                    http = http.header("x-openai-internal-codex-residency", residency);
                }

                http
            }
            Credential::Ambient { .. } => return Err(Error::Unauthenticated(String::new())),
        };

        Ok(super::mode_headers(http, request, Vec::new()))
    }

    async fn stream_http(&self, http: reqwest::RequestBuilder) -> Result<ChunkStream, Error> {
        let response = super::http::send(http, &self.timeouts).await?;
        let status = response.status();
        if !status.is_success() {
            let headers = response.headers().clone();
            let text = super::http::bounded_body(response, &self.timeouts).await;

            return Err(api_error(status.as_u16(), &text).with_headers(&headers));
        }

        let limits = super::limits::Limits::from_headers(response.headers()).map(|limits| Ok(Chunk::Limits(limits)));
        let mut state = StreamState::default();
        let events = sse::events(response.bytes_stream(), self.timeouts.idle);
        let chunks = events.flat_map(move |event| {
            let items: Vec<Result<Chunk, Error>> = match event {
                Err(error) => vec![Err(Error::Transport(error.to_string()))],
                Ok(event) => match state.chunks(&event.data) {
                    Ok(chunks) => chunks.into_iter().map(Ok).collect(),
                    Err(error) => vec![Err(error)],
                },
            };
            futures_util::stream::iter(items)
        });

        Ok(Box::pin(futures_util::stream::iter(limits).chain(chunks)))
    }
}

/// The compute residency a ChatGPT sign-in's access token claims, unless it is unconstrained.
fn residency(access: &str) -> Option<String> {
    use base64::Engine as _;
    let payload = access.split('.').nth(1)?;
    let claims: Value = serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload.trim_end_matches('='))
            .ok()?,
    )
    .ok()?;
    let found = claims["https://api.openai.com/auth"]["chatgpt_compute_residency"]
        .as_str()
        .or(claims["chatgpt_compute_residency"].as_str())?;

    (!found.is_empty() && found != "no_constraint").then(|| found.to_string())
}

/// `code` names the fault; a streamed `error` event's `type` is just "error".
fn api_error(status: u16, text: &str) -> Error {
    let parsed: Value = serde_json::from_str(text).unwrap_or_default();
    let error = if parsed["error"].is_object() {
        &parsed["error"]
    } else {
        &parsed
    };
    let kind = error["code"]
        .as_str()
        .or(error["type"].as_str())
        .unwrap_or("api_error")
        .to_string();
    let message = error["message"].as_str().unwrap_or(text).to_string();

    match status {
        401 | 403 => Error::Unauthenticated(message),
        _ => Error::api(status, kind, message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolSpec;

    mod subscriptions {
        use super::requests::request;
        use super::*;

        #[test]
        fn a_sign_in_bound_to_a_region_names_it() {
            use base64::Engine as _;
            let token = |claims: Value| {
                format!(
                    "h.{}.s",
                    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string())
                )
            };
            assert_eq!(
                residency(&token(
                    json!({ "https://api.openai.com/auth": { "chatgpt_compute_residency": "eu" } })
                ))
                .as_deref(),
                Some("eu")
            );
            assert_eq!(
                residency(&token(json!({ "chatgpt_compute_residency": "no_constraint" }))),
                None
            );
            assert_eq!(residency("not-a-jwt"), None);
        }

        #[tokio::test]
        async fn a_codex_request_carries_its_session_and_residency() {
            use base64::Engine as _;

            let _ = rustls::crypto::ring::default_provider().install_default();
            let seen: std::sync::Arc<std::sync::Mutex<Option<axum::http::HeaderMap>>> = Default::default();
            let recorded = seen.clone();
            let app = axum::Router::new().fallback(axum::routing::post(move |headers: axum::http::HeaderMap| {
                *recorded.lock().unwrap() = Some(headers);
                async {
                    (
                        [("content-type", "text/event-stream")],
                        "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{}}}\n\n",
                    )
                }
            }));

            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

            let access = format!(
                "h.{}.s",
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(json!({ "chatgpt_compute_residency": "eu" }).to_string())
            );
            let credential = Credential::OAuth {
                access,
                refresh: String::new(),
                expires_at: 0,
                account: Some("acct".into()),
            };
            let request = Request {
                cache_key: Some("ses_1".into()),
                ..request()
            };

            let _ = OpenAi::new(&url)
                .stream(&request, &credential)
                .await
                .unwrap()
                .collect::<Vec<_>>()
                .await;

            let headers = seen.lock().unwrap().clone().unwrap();
            assert_eq!(
                (
                    headers["session-id"].to_str().unwrap(),
                    headers["x-openai-internal-codex-residency"].to_str().unwrap()
                ),
                ("ses_1", "eu")
            );
            assert_eq!(headers["chatgpt-account-id"], "acct");
        }
    }

    mod requests {
        use super::*;

        #[test]
        fn a_pdf_is_an_input_file() {
            let sent = items(&ChatMessage {
                role: Role::User,
                blocks: vec![Block::Pdf {
                    base64: "JVBERi0=".into(),
                }],
            });
            assert_eq!(
                sent[0]["content"][0],
                json!({
                    "type": "input_file",
                    "filename": "document.pdf",
                    "file_data": "data:application/pdf;base64,JVBERi0="
                })
            );
        }

        #[test]
        fn verbosity_rides_in_text() {
            assert!(body(&request(), false).get("text").is_none());
            assert_eq!(
                body(
                    &Request {
                        verbosity: Some("low"),
                        ..request()
                    },
                    true
                )["text"],
                json!({ "verbosity": "low" })
            );
        }

        #[test]
        fn a_text_only_request_keeps_its_tools_but_forbids_calls() {
            assert_eq!(body(&request(), false)["tool_choice"], "auto");
            let built = body(
                &Request {
                    no_tool_calls: true,
                    ..request()
                },
                true,
            );
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
                model: "gpt-5.4".into(),
                system: "You are Drift.".into(),
                messages: vec![
                    ChatMessage {
                        role: Role::User,
                        blocks: vec![
                            Block::Text("hi".into()),
                            Block::Image {
                                mime: "image/png".into(),
                                base64: "AAAA".into(),
                            },
                        ],
                    },
                    ChatMessage {
                        role: Role::Assistant,
                        blocks: vec![
                            Block::Reasoning {
                                text: "think".into(),
                                signature: Some("enc".into()),
                                redacted: None,
                            },
                            Block::Text("Let me look".into()),
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
                reasoning: Some(Reasoning::Effort { level: "medium".into() }),
                temperature: None,
                cache_key: Some("ses_1".into()),
                no_tool_calls: false,
                verbosity: None,
                show_thinking: false,
                top_p: None,
                top_k: None,
                mode: None,
            }
        }

        #[test]
        fn every_request_of_a_conversation_carries_its_cache_key_on_both_routes() {
            assert_eq!(body(&request(), false)["prompt_cache_key"], "ses_1");
            assert_eq!(
                body(&request(), true)["prompt_cache_key"],
                "ses_1",
                "the Codex route too"
            );
            let mut keyless = request();
            keyless.cache_key = None;
            assert!(body(&keyless, false).get("prompt_cache_key").is_none());
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
            assert!(body(&request(), true).get("max_output_tokens").is_none());
        }

        #[test]
        fn message_blocks_become_responses_input_items() {
            let built = body(&request(), false);
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
        }

        #[test]
        fn a_budget_is_not_an_openai_setting() {
            let mut request = request();
            request.reasoning = Some(Reasoning::Budget { tokens: 8000 });
            assert!(body(&request, false).get("reasoning").is_none());
        }

        #[test]
        fn a_daybreak_mode_selects_the_program_without_changing_the_wire_model() {
            let mut request = request();
            assert!(body(&request, true).get("access_programs").is_none());
            request.mode = Some(crate::llm::catalog::ModelMode {
                name: "daybreak".into(),
                base: request.model.clone(),
                body: json!({ "access_programs": { "cyber": "daybreak_blue" } })
                    .as_object()
                    .unwrap()
                    .clone(),
                headers: Default::default(),
            });
            let built = body(&request, true);
            assert_eq!(built["model"], request.model);
            assert_eq!(built["access_programs"]["cyber"], "daybreak_blue");
            assert_eq!(built["reasoning"]["effort"], "medium");
        }
    }

    mod streams {
        use super::*;

        #[test]
        fn stream_events_map_to_chunks() {
            let mut state = StreamState::default();
            let feed = |state: &mut StreamState, json: &str| state.chunks(json).unwrap();

            assert_eq!(feed(&mut state, r#"{"type":"response.created","response":{}}"#), vec![]);
            assert_eq!(
                feed(
                    &mut state,
                    r#"{"type":"response.output_item.added","item":{"type":"reasoning","id":"rs_1"}}"#
                ),
                vec![Chunk::ReasoningStart]
            );
            assert_eq!(
                feed(
                    &mut state,
                    r#"{"type":"response.reasoning_summary_text.delta","item_id":"rs_1","delta":"hm"}"#
                ),
                vec![Chunk::ReasoningDelta("hm".into())]
            );
            assert_eq!(
                feed(
                    &mut state,
                    r#"{"type":"response.reasoning_summary_part.added","item_id":"rs_1","summary_index":1}"#
                ),
                vec![Chunk::ReasoningDelta("\n\n".into())]
            );
            assert_eq!(
                feed(
                    &mut state,
                    &json!({
                        "type": "response.output_item.done",
                        "item": { "type": "reasoning", "id": "rs_1", "encrypted_content": "enc" }
                    })
                    .to_string()
                ),
                vec![Chunk::ReasoningSignature("enc".into()), Chunk::BlockStop]
            );

            assert_eq!(
                feed(
                    &mut state,
                    r#"{"type":"response.output_item.added","item":{"type":"message","id":"msg_1"}}"#
                ),
                vec![Chunk::TextStart]
            );
            assert_eq!(
                feed(
                    &mut state,
                    r#"{"type":"response.output_text.delta","item_id":"msg_1","delta":"Hi"}"#
                ),
                vec![Chunk::TextDelta("Hi".into())]
            );
            assert_eq!(
                feed(
                    &mut state,
                    r#"{"type":"response.output_item.done","item":{"type":"message","id":"msg_1"}}"#
                ),
                vec![Chunk::BlockStop]
            );
        }

        #[test]
        fn function_call_deltas_finish_with_usage_and_tool_stop() {
            let mut state = StreamState::default();
            let feed = |state: &mut StreamState, json: &str| state.chunks(json).unwrap();
            let call = json!({ "type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "read" });

            assert_eq!(
                feed(
                    &mut state,
                    &json!({ "type": "response.output_item.added", "item": with_arguments(&call, "") }).to_string()
                ),
                vec![Chunk::ToolUseStart {
                    id: "call_1".into(),
                    name: "read".into()
                }]
            );
            assert_eq!(
                feed(
                    &mut state,
                    r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"{\"pa"}"#
                ),
                vec![Chunk::ToolInputDelta("{\"pa".into())]
            );
            assert_eq!(
                feed(
                    &mut state,
                    &json!({ "type": "response.output_item.done", "item": with_arguments(&call, r#"{"path":"a"}"#) })
                        .to_string()
                ),
                vec![Chunk::BlockStop]
            );

            let done = feed(
                &mut state,
                &json!({
                    "type": "response.completed",
                    "response": {
                        "usage": {
                            "input_tokens": 100,
                            "input_tokens_details": { "cached_tokens": 40 },
                            "output_tokens": 9
                        }
                    }
                })
                .to_string(),
            );
            assert_eq!(
                done,
                vec![
                    Chunk::Usage(Usage {
                        input: 60,
                        output: 9,
                        cache_read: 40,
                        cache_write: 0
                    }),
                    Chunk::Stop(StopReason::ToolUse)
                ]
            );
        }

        /// `call` with its `arguments` set, as an output item event carries it.
        fn with_arguments(call: &Value, arguments: &str) -> Value {
            let mut item = call.clone();
            item["arguments"] = json!(arguments);
            item
        }

        #[test]
        fn a_call_without_deltas_takes_arguments_from_done() {
            let mut state = StreamState::default();
            let call = json!({ "type": "function_call", "id": "fc_1", "call_id": "c", "name": "read" });

            let added = json!({ "type": "response.output_item.added", "item": call });
            state.chunks(&added.to_string()).unwrap();

            let finished = json!({ "type": "response.output_item.done", "item": with_arguments(&call, "{}") });
            let done = state.chunks(&finished.to_string()).unwrap();
            assert_eq!(done, vec![Chunk::ToolInputDelta("{}".into()), Chunk::BlockStop]);
        }

        #[test]
        fn incomplete_and_errors_classify() {
            let state = StreamState::default();
            let out = state.finished(
                &json!({ "incomplete_details": { "reason": "max_output_tokens" }, "usage": {} }),
                true,
            );
            assert_eq!(out[1], Chunk::Stop(StopReason::MaxTokens));
            let streamed =
                StreamState::default().chunks(r#"{"type":"error","code":"server_error","message":"try again"}"#);
            assert!(
                matches!(streamed, Err(Error::Api { ref kind, retryable: true, .. }) if kind == "server_error"),
                "{streamed:?}"
            );
            let failed = StreamState::default().chunks(
                r#"{"type":"response.failed","response":{"error":{"code":"rate_limit_exceeded","message":"slow"}}}"#,
            );
            assert!(matches!(failed, Err(Error::Api { retryable: true, .. })), "{failed:?}");
            let refused = StreamState::default().chunks(r#"{"type":"error","code":"invalid_prompt","message":"no"}"#);
            assert!(matches!(refused, Err(Error::Api { retryable: false, .. })));
            assert!(matches!(api_error(429, "{}"), Error::Api { retryable: true, .. }));
            let expired = api_error(401, r#"{"error":{"message":"token expired"}}"#);
            assert!(matches!(expired, Error::Unauthenticated(ref m) if m == "token expired"));
        }
    }
}
