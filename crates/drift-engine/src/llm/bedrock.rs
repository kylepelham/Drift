//! Claude on Amazon Bedrock: the Anthropic Messages body, signed with SigV4 (or a Bedrock API key), streamed as AWS event-stream frames.

use base64::Engine as _;
use futures_util::StreamExt;
use serde_json::Value;

use super::aws::{self, Auth};
use super::eventstream::Decoder;
use super::{Chunk, ChunkStream, Credential, Error, Request, anthropic};

const VERSION: &str = "bedrock-2023-05-31";

#[derive(Clone, Debug)]
pub struct Bedrock {
    /// `DRIFT_AMAZON_BEDROCK_BASE_URL` for recorded runs; otherwise the region's runtime endpoint.
    base_url: Option<String>,
    client: reqwest::Client,
    pub timeouts: super::http::Timeouts,
}

impl Bedrock {
    pub fn new(base_url: Option<String>) -> Self {
        Self {
            base_url,
            client: super::http::client(),
            timeouts: super::http::Timeouts::default(),
        }
    }

    pub async fn stream(&self, request: &Request, credential: &Credential) -> Result<ChunkStream, Error> {
        // A key saved in Settings is a Bedrock API key; otherwise the environment's, found now.
        let auth = match credential {
            Credential::ApiKey { key } => Auth::Bearer(key.clone()),
            _ => aws::auth().ok_or(Error::Unauthenticated(String::new()))?,
        };
        self.send(request, &auth, &aws::region()).await
    }

    async fn send(&self, request: &Request, auth: &Auth, region: &str) -> Result<ChunkStream, Error> {
        let region = region.to_string();
        let base = self
            .base_url
            .clone()
            .unwrap_or_else(|| format!("https://bedrock-runtime.{region}.amazonaws.com"));
        let path = format!("/model/{}/invoke-with-response-stream", aws::encode(&request.model));
        let mut body = anthropic::cloud_body(request, VERSION, false);
        // Bedrock takes Anthropic betas in the body.
        if anthropic::interleaves(request) {
            body["anthropic_beta"] = serde_json::json!([anthropic::INTERLEAVED_THINKING]);
        }
        let body = serde_json::to_vec(&body).map_err(|e| Error::Malformed(e.to_string()))?;
        let mut http = self
            .client
            .post(format!("{base}{path}"))
            .header("content-type", "application/json")
            .header("accept", "application/vnd.amazon.eventstream");
        http = match auth {
            Auth::Bearer(token) => http.bearer_auth(token),
            Auth::Signed(keys) => {
                let host = base.split("://").nth(1).unwrap_or(&base).trim_end_matches('/');
                let date = aws::amz_date();
                let signing = aws::Signing {
                    method: "POST",
                    host,
                    path: &path,
                    body: &body,
                    region: &region,
                    service: "bedrock",
                    amz_date: &date,
                };
                aws::sign(&signing, keys)
                    .into_iter()
                    .fold(http, |http, (name, value)| http.header(name, value))
            }
        };
        let response = super::http::send(http.body(body), &self.timeouts).await?;
        let status = response.status();
        if !status.is_success() {
            let headers = response.headers().clone();
            // `x-amzn-ErrorType` names the fault as `ThrottlingException:<doc url>`.
            let kind = headers
                .get("x-amzn-errortype")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(':').next())
                .unwrap_or_default()
                .to_string();
            return Err(api_error(
                status.as_u16(),
                &kind,
                &super::http::bounded_body(response, &self.timeouts).await,
            )
            .with_headers(&headers));
        }
        let mut decoder = Decoder::default();
        let frames = super::sse::watched(response.bytes_stream(), self.timeouts.idle);
        Ok(Box::pin(frames.flat_map(move |bytes| {
            let items = match bytes {
                Ok(bytes) => decoder
                    .feed(&bytes)
                    .map_err(Error::Malformed)
                    .map(|messages| messages.into_iter().flat_map(message_chunks).collect())
                    .unwrap_or_else(|e| vec![Err(e)]),
                Err(error) => vec![Err(Error::Transport(error))],
            };
            futures_util::stream::iter(items)
        })))
    }
}

/// A `chunk` wraps an Anthropic event; exception and error frames end the stream with their reason; unknown kinds are a broken stream.
fn message_chunks(message: super::eventstream::Message) -> Vec<Result<Chunk, Error>> {
    let payload: Value = serde_json::from_slice(&message.payload).unwrap_or_default();
    let header = |name: &str| message.headers.get(name).map(String::as_str);
    match (header(":message-type"), header(":event-type")) {
        (Some("event"), Some("chunk")) => chunk_events(&payload),
        // Other event kinds carry no content for this route.
        (Some("event"), _) => Vec::new(),
        (Some("exception"), _) => vec![Err(stream_exception(
            header(":exception-type").unwrap_or("exception"),
            &payload,
        ))],
        (Some("error"), _) => {
            let message = header(":error-message")
                .map(str::to_string)
                .unwrap_or_else(|| String::from_utf8_lossy(&message.payload).into_owned());
            vec![Err(classify(
                super::STREAMED,
                header(":error-code").unwrap_or("error"),
                &message,
            ))]
        }
        (other, _) => vec![Err(Error::Malformed(format!(
            "Bedrock sent an event-stream message of kind {other:?}"
        )))],
    }
}

fn chunk_events(payload: &Value) -> Vec<Result<Chunk, Error>> {
    let decoded = payload["bytes"]
        .as_str()
        .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok());
    let Some(decoded) = decoded else {
        return vec![Err(Error::Malformed("a Bedrock chunk had no bytes".into()))];
    };
    let text = String::from_utf8_lossy(&decoded);
    let event: Value = serde_json::from_str(&text).unwrap_or_default();
    match anthropic::chunks(event["type"].as_str().unwrap_or_default(), &text) {
        Ok(chunks) => chunks.into_iter().map(Ok).collect(),
        Err(error) => vec![Err(error)],
    }
}

/// A model stream error carries the model's own status and message when it has them.
fn stream_exception(kind: &str, payload: &Value) -> Error {
    let status = payload["originalStatusCode"]
        .as_u64()
        .and_then(|s| u16::try_from(s).ok())
        .unwrap_or(super::STREAMED);
    let message = payload["originalMessage"]
        .as_str()
        .or(payload["message"].as_str())
        .unwrap_or(kind);
    classify(status, kind, message)
}

/// Bedrock answers errors as `{"message": ...}`, naming the fault in a header; without one the status says.
fn api_error(status: u16, kind: &str, text: &str) -> Error {
    let message = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| text.to_string());
    classify(status, kind, &message)
}

/// Each Bedrock fault as the retry rules read it: the status it stands for when the response gave none.
fn classify(status: u16, kind: &str, message: &str) -> Error {
    let name = kind.to_ascii_lowercase();
    if matches!(status, 401 | 403)
        || ["accessdenied", "unrecognizedclient", "expiredtoken", "invalidsignature"]
            .iter()
            .any(|k| name.starts_with(k))
    {
        return Error::Unauthenticated(message.to_string());
    }
    let implied = match name.trim_end_matches("exception") {
        "throttling" => 429,
        "serviceunavailable" | "modelnotready" => 503,
        "internalserver" | "modelstreamerror" => 500,
        "modeltimeout" => 504,
        "validation" | "modelerror" => 400,
        "resourcenotfound" => 404,
        _ => status,
    };
    let status = if status == super::STREAMED || status == 0 {
        implied
    } else {
        status
    };
    Error::api(status, kind, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::eventstream::frame;

    fn chunk(event: &str) -> Vec<u8> {
        let wrapped =
            serde_json::json!({ "bytes": base64::engine::general_purpose::STANDARD.encode(event) }).to_string();
        frame(
            &[
                (":message-type", "event"),
                (":event-type", "chunk"),
                (":content-type", "application/json"),
            ],
            wrapped.as_bytes(),
        )
    }

    #[test]
    fn chunk_events_are_anthropic_events_inside() {
        let mut decoder = Decoder::default();
        let stream = [
            chunk(r#"{"type":"message_start","message":{"usage":{"input_tokens":12,"output_tokens":1}}}"#),
            chunk(r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#),
            chunk(r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#),
            chunk(r#"{"type":"content_block_stop","index":0}"#),
            chunk(r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}"#),
        ]
        .concat();
        let chunks: Vec<Chunk> = decoder
            .feed(&stream)
            .unwrap()
            .into_iter()
            .flat_map(message_chunks)
            .map(Result::unwrap)
            .collect();
        assert!(matches!(chunks[0], Chunk::Usage(ref u) if u.input == 12));
        assert_eq!(chunks[1], Chunk::TextStart);
        assert_eq!(chunks[2], Chunk::TextDelta("Hi".into()));
        assert!(chunks.contains(&Chunk::Stop(crate::llm::StopReason::EndTurn)));
    }

    fn first_error(bytes: &[u8]) -> Error {
        Decoder::default()
            .feed(bytes)
            .unwrap()
            .into_iter()
            .flat_map(message_chunks)
            .next()
            .expect("a frame must not vanish")
            .unwrap_err()
    }

    fn retryable(error: &Error) -> Option<(u16, bool)> {
        match error {
            Error::Api { status, retryable, .. } => Some((*status, *retryable)),
            _ => None,
        }
    }

    #[test]
    fn exceptions_and_error_frames_end_the_stream_with_a_reason_retries_understand() {
        let throttled = first_error(&frame(
            &[
                (":message-type", "exception"),
                (":exception-type", "throttlingException"),
            ],
            br#"{"message":"Too many requests"}"#,
        ));
        assert!(throttled.to_string().contains("Too many requests"));
        assert_eq!(retryable(&throttled), Some((429, true)));
        let internal = first_error(&frame(
            &[
                (":message-type", "exception"),
                (":exception-type", "internalServerException"),
            ],
            br#"{"message":"oops"}"#,
        ));
        assert_eq!(retryable(&internal), Some((500, true)));
        let model = br#"{"message":"stream failed","originalStatusCode":529,"originalMessage":"Overloaded"}"#;
        let overloaded = first_error(&frame(
            &[
                (":message-type", "exception"),
                (":exception-type", "modelStreamErrorException"),
            ],
            model,
        ));
        assert_eq!(
            retryable(&overloaded),
            Some((529, true)),
            "the model's own status is kept"
        );
        assert!(overloaded.to_string().contains("Overloaded"));
        let invalid = first_error(&frame(
            &[
                (":message-type", "exception"),
                (":exception-type", "validationException"),
            ],
            br#"{"message":"bad input"}"#,
        ));
        assert_eq!(retryable(&invalid), Some((400, false)));
        let errored = first_error(&frame(
            &[
                (":message-type", "error"),
                (":error-code", "ServiceUnavailableException"),
                (":error-message", "try later"),
            ],
            b"",
        ));
        assert!(errored.to_string().contains("try later"));
        assert_eq!(retryable(&errored), Some((503, true)));
        let unknown = first_error(&frame(&[(":message-type", "surprise")], b"{}"));
        assert!(matches!(unknown, Error::Malformed(_)));
        assert!(
            matches!(api_error(403, "", r#"{"message":"no access"}"#), Error::Unauthenticated(ref m) if m == "no access")
        );
        assert_eq!(
            retryable(&api_error(400, "ThrottlingException", r#"{"message":"slow"}"#)),
            Some((400, false)),
            "a status given is the status kept"
        );
        assert!(
            api_error(429, "ThrottlingException", r#"{"message":"slow"}"#)
                .to_string()
                .contains("slow")
        );
    }

    /// Path, authorization and body of each request the fake saw.
    type Seen = std::sync::Arc<std::sync::Mutex<Vec<(String, String, String)>>>;

    /// A local stand-in for the runtime endpoint: records what arrived and answers with event-stream frames.
    async fn fake(reply: Vec<u8>) -> (String, Seen) {
        use axum::extract::{OriginalUri, State};
        let seen: Seen = Default::default();
        let handler = |State((seen, reply)): State<(Seen, Vec<u8>)>,
                       OriginalUri(uri): OriginalUri,
                       headers: axum::http::HeaderMap,
                       body: String| async move {
            let auth = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            seen.lock().unwrap().push((uri.path().to_string(), auth, body));
            ([("content-type", "application/vnd.amazon.eventstream")], reply)
        };
        let app = axum::Router::new()
            .fallback(axum::routing::post(handler))
            .with_state((seen.clone(), reply));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, seen)
    }

    #[tokio::test]
    async fn a_signed_request_reaches_the_model_path_and_its_reply_streams_back() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let reply = [
            chunk(r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#),
            chunk(r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"from bedrock"}}"#),
            chunk(r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}"#),
        ]
        .concat();
        let (url, seen) = fake(reply).await;
        let mut request = crate::llm::tests::request();
        request.model = "us.anthropic.claude-sonnet-4-5-v1:0".into();
        let keys = aws::Keys {
            access_key: "AKIDEXAMPLE".into(),
            secret_key: "secret".into(),
            session_token: None,
        };
        let stream = Bedrock::new(Some(url))
            .send(&request, &Auth::Signed(keys), "eu-west-1")
            .await
            .unwrap();
        let chunks: Vec<Chunk> = stream.map(Result::unwrap).collect().await;
        assert!(chunks.contains(&Chunk::TextDelta("from bedrock".into())));
        let (path, auth, body) = seen.lock().unwrap()[0].clone();
        assert_eq!(
            path,
            "/model/us.anthropic.claude-sonnet-4-5-v1%3A0/invoke-with-response-stream"
        );
        assert!(
            auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/")
                && auth.contains("/eu-west-1/bedrock/aws4_request"),
            "{auth}"
        );
        let sent: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(sent["anthropic_version"], VERSION);
        assert!(sent.get("anthropic_beta").is_none(), "no budget, no beta");
        let (budget_url, budget_seen) = fake(Vec::new()).await;
        let mut budgeted = request.clone();
        budgeted.tools = vec![crate::llm::ToolSpec {
            name: "read".into(),
            description: "r".into(),
            input_schema: serde_json::json!({}),
        }];
        budgeted.reasoning = Some(crate::llm::catalog::Reasoning::Budget { tokens: 4096 });
        let _ = Bedrock::new(Some(budget_url))
            .stream(&budgeted, &Credential::ApiKey { key: "k".into() })
            .await;
        let sent: Value = serde_json::from_str(&budget_seen.lock().unwrap()[0].2).unwrap();
        assert_eq!(
            sent["anthropic_beta"],
            serde_json::json!([anthropic::INTERLEAVED_THINKING]),
            "Bedrock takes the beta in the body"
        );

        let (url, seen) = fake(Vec::new()).await;
        let _ = Bedrock::new(Some(url))
            .stream(
                &request,
                &Credential::ApiKey {
                    key: "bedrock-api-key".into(),
                },
            )
            .await;
        assert_eq!(
            seen.lock().unwrap()[0].1,
            "Bearer bedrock-api-key",
            "a Bedrock API key is a bearer token"
        );
    }

    #[test]
    fn the_body_has_no_model_or_stream_and_names_bedrocks_version() {
        let request = crate::llm::tests::request();
        let body = anthropic::cloud_body(&request, VERSION, false);
        assert_eq!(body["anthropic_version"], VERSION);
        assert!(body.get("model").is_none() && body.get("stream").is_none());
        assert!(body["messages"].is_array());
    }
}
