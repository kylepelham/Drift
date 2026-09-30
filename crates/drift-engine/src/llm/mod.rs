//! Talking to models. One neutral request shape, one adapter per wire protocol, streamed chunks out.

pub mod anthropic;
pub mod catalog;
pub mod compat;
pub mod credentials;
pub mod gemini;
pub mod openai;
mod sse;
#[cfg(test)]
mod tests;

use std::pin::Pin;

use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::session::types::Usage;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Credential {
    ApiKey { key: String },
    OAuth {
        access: String,
        refresh: String,
        expires_at: i64,
        /// ChatGPT account id for Codex; absent for Anthropic.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<String>,
    },
}

impl Credential {
    pub fn is_expired(&self) -> bool {
        matches!(self, Self::OAuth { expires_at, .. } if *expires_at < crate::id::now_ms())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Role {
    User,
    Assistant,
}

/// One content block as the model sees it. Provider-specific fields ride along untouched.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Text(String),
    /// signature is whatever the provider needs to accept the block back: Anthropic's signature, OpenAI's encrypted content.
    Reasoning { text: String, signature: Option<String>, redacted: Option<String> },
    ToolUse { id: String, name: String, input: Value },
    ToolResult { call_id: String, content: String, is_error: bool },
    Image { mime: String, base64: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatMessage {
    pub role: Role,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub model: String,
    pub system: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: u32,
    pub thinking_budget: Option<u32>,
    pub temperature: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Other,
}

/// Streamed pieces of one assistant message, in arrival order.
#[derive(Clone, Debug, PartialEq)]
pub enum Chunk {
    TextStart,
    TextDelta(String),
    ReasoningStart,
    ReasoningDelta(String),
    ReasoningSignature(String),
    ReasoningRedacted(String),
    ToolUseStart { id: String, name: String },
    ToolInputDelta(String),
    BlockStop,
    Usage(Usage),
    Stop(StopReason),
}

#[derive(Debug)]
pub enum Error {
    /// The provider answered with an error. `retryable` covers rate limits, overload and server
    /// faults; `retry_after` is the wait the provider asked for, when it named one.
    Api { status: u16, kind: String, message: String, retryable: bool, retry_after: Option<std::time::Duration> },
    Transport(String),
    Malformed(String),
    Unauthenticated,
}

/// The status adapters give an error that arrives inside a stream which began with 200 OK.
pub const STREAMED: u16 = 200;
/// Timeouts, rate limits, overload and server faults: worth another try.
const RETRY_STATUSES: [u16; 7] = [408, 429, 500, 502, 503, 504, 529];
/// The same faults as providers name them inside a stream, lowercased.
const RETRY_KINDS: [&str; 10] = [
    "overloaded_error",
    "rate_limit_error",
    "api_error",
    "server_error",
    "rate_limit_exceeded",
    "server_is_overloaded",
    "resource_exhausted",
    "unavailable",
    "internal",
    "deadline_exceeded",
];

/// Faults no wait will fix, whatever status they came with: OpenAI answers a spent balance with 429.
const PERMANENT_KINDS: [&str; 4] = ["insufficient_quota", "billing_hard_limit_reached", "billing_not_active", "access_terminated"];

fn permanent(kind: &str) -> bool {
    PERMANENT_KINDS.contains(&kind.to_ascii_lowercase().as_str())
}

impl Error {
    /// A provider error, retryable by its status or, inside a stream, by what the provider calls it.
    /// A permanent fault is never retryable.
    pub fn api(status: u16, kind: impl Into<String>, message: impl Into<String>) -> Self {
        let kind = kind.into();
        let transient = RETRY_STATUSES.contains(&status) || (status == STREAMED && RETRY_KINDS.contains(&kind.to_ascii_lowercase().as_str()));
        let retryable = transient && !permanent(&kind);
        Self::Api { status, kind, message: message.into(), retryable, retry_after: None }
    }

    /// Takes what the response headers say about retrying: the wait the provider asks for, and its
    /// explicit `x-should-retry` verdict, which cannot make a permanent fault retryable.
    pub fn with_headers(mut self, headers: &http::HeaderMap) -> Self {
        if let Self::Api { kind, retryable, retry_after, .. } = &mut self {
            *retry_after = requested_wait(headers);
            match headers.get("x-should-retry").and_then(|v| v.to_str().ok()) {
                Some("true") => *retryable = !permanent(kind),
                Some("false") => *retryable = false,
                _ => {}
            }
        }
        self
    }
}

/// `retry-after-ms`, else `retry-after` as seconds or as an HTTP date. A wait too long to represent
/// saturates, so the caller sees it as longer than it will wait rather than failing to parse it.
fn requested_wait(headers: &http::HeaderMap) -> Option<std::time::Duration> {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim);
    let seconds = |text: &str| text.parse::<f64>().ok().filter(|n| !n.is_nan() && *n >= 0.0);
    let span = |secs: f64| std::time::Duration::try_from_secs_f64(secs).unwrap_or(std::time::Duration::MAX);
    if let Some(ms) = header("retry-after-ms").and_then(seconds) {
        return Some(span(ms / 1000.0));
    }
    let value = header("retry-after")?;
    if let Some(secs) = seconds(value) {
        return Some(span(secs));
    }
    let at = httpdate::parse_http_date(value).ok()?;
    Some(at.duration_since(std::time::SystemTime::now()).unwrap_or_default())
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn headers(pairs: &[(&'static str, String)]) -> http::HeaderMap {
        pairs.iter().map(|(name, value)| (http::HeaderName::from_static(name), value.parse().unwrap())).collect()
    }

    fn wait(error: &Error) -> Option<Duration> {
        let Error::Api { retry_after, .. } = error else { panic!() };
        *retry_after
    }

    #[test]
    fn faults_inside_a_stream_retry_by_name_and_request_errors_do_not() {
        for kind in ["overloaded_error", "rate_limit_error", "api_error", "server_error", "UNAVAILABLE", "RESOURCE_EXHAUSTED"] {
            assert!(matches!(Error::api(STREAMED, kind, "x"), Error::Api { retryable: true, .. }), "{kind}");
        }
        for kind in ["invalid_request_error", "authentication_error", "insufficient_quota", "INVALID_ARGUMENT"] {
            assert!(matches!(Error::api(STREAMED, kind, "x"), Error::Api { retryable: false, .. }), "{kind}");
        }
        assert!(matches!(Error::api(400, "api_error", "x"), Error::Api { retryable: false, .. }), "a status decides when there is one");
        assert!(matches!(Error::api(529, "anything", "x"), Error::Api { retryable: true, .. }));
    }

    #[test]
    fn a_spent_quota_never_retries_whatever_its_status_or_headers_say() {
        let spent = Error::api(429, "insufficient_quota", "You exceeded your current quota");
        assert!(matches!(spent, Error::Api { retryable: false, .. }));
        let told = spent.with_headers(&headers(&[("x-should-retry", "true".into()), ("retry-after", "1".into())]));
        assert!(matches!(told, Error::Api { retryable: false, .. }));
        assert!(matches!(Error::api(STREAMED, "billing_hard_limit_reached", "x"), Error::Api { retryable: false, .. }));
        assert!(matches!(Error::api(429, "rate_limit_exceeded", "x"), Error::Api { retryable: true, .. }), "an ordinary rate limit still retries");
    }

    #[test]
    fn the_providers_wait_is_read_in_every_form() {
        let busy = || Error::api(429, "rate_limit_error", "slow down");
        assert_eq!(wait(&busy().with_headers(&headers(&[("retry-after-ms", "1500".into()), ("retry-after", "9".into())]))), Some(Duration::from_millis(1500)), "ms wins");
        assert_eq!(wait(&busy().with_headers(&headers(&[("retry-after", "7".into())]))), Some(Duration::from_secs(7)));
        let later = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(120));
        let dated = wait(&busy().with_headers(&headers(&[("retry-after", later)]))).unwrap();
        assert!(dated > Duration::from_secs(110) && dated <= Duration::from_secs(120), "{dated:?}");
        let past = httpdate::fmt_http_date(SystemTime::now() - Duration::from_secs(60));
        assert_eq!(wait(&busy().with_headers(&headers(&[("retry-after", past)]))), Some(Duration::ZERO));
        assert_eq!(wait(&busy().with_headers(&headers(&[("retry-after", "soon".into())]))), None);
        for huge in ["1e300", "18446744073709551616", "inf"] {
            assert_eq!(wait(&busy().with_headers(&headers(&[("retry-after", huge.into())]))), Some(Duration::MAX), "{huge} saturates");
        }
        for huge in ["1e300", "inf"] {
            assert_eq!(wait(&busy().with_headers(&headers(&[("retry-after-ms", huge.into())]))), Some(Duration::MAX), "{huge} ms saturates");
        }
        assert!(matches!(busy().with_headers(&headers(&[("x-should-retry", "false".into())])), Error::Api { retryable: false, .. }));
        assert!(matches!(Error::api(400, "x", "y").with_headers(&headers(&[("x-should-retry", "true".into())])), Error::Api { retryable: true, .. }));
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api { status, kind, message, .. } => write!(f, "{kind} ({status}): {message}"),
            Self::Transport(message) => write!(f, "transport: {message}"),
            Self::Malformed(message) => write!(f, "malformed response: {message}"),
            Self::Unauthenticated => write!(f, "no credentials for this provider"),
        }
    }
}

impl std::error::Error for Error {}

/// How each provider says the request no longer fits the model's context window.
const OVERFLOW_PHRASES: [&str; 7] = [
    "prompt is too long",
    "context_length_exceeded",
    "maximum context length",
    "exceeds the context window",
    "input is too long",
    "too many input tokens",
    "input token count",
];

impl Error {
    /// The request did not fit the model's context; compacting can recover it where retrying cannot.
    pub fn is_context_overflow(&self) -> bool {
        matches!(self, Self::Api { status: 400 | 413, .. }) && mentions_context_overflow(&self.to_string())
    }
}

/// Also used on errors already flattened to text, such as a failed summary request.
pub fn mentions_context_overflow(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    OVERFLOW_PHRASES.iter().any(|phrase| text.contains(phrase))
}

#[cfg(test)]
mod overflow_tests {
    use super::Error;

    fn api(status: u16, message: &str) -> Error {
        Error::api(status, "invalid_request_error", message)
    }

    #[test]
    fn each_providers_too_long_error_is_recognised_and_nothing_else_is() {
        for message in [
            "prompt is too long: 210432 tokens > 200000 maximum",
            "Your input exceeds the context window of this model. Please adjust your input and try again.",
            "This model's maximum context length is 128000 tokens. However, your messages resulted in 130211 tokens.",
            "context_length_exceeded",
            "The input token count (1048577) exceeds the maximum number of tokens allowed (1048576).",
        ] {
            assert!(api(400, message).is_context_overflow(), "{message}");
        }
        assert!(api(413, "Input is too long for requested model.").is_context_overflow());
        assert!(!api(400, "messages: text content blocks must be non-empty").is_context_overflow());
        assert!(!api(429, "prompt is too long").is_context_overflow(), "a rate limit is not an overflow");
        assert!(!Error::Transport("prompt is too long".into()).is_context_overflow());
    }
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Self::Transport(error.to_string())
    }
}

pub type ChunkStream = Pin<Box<dyn Stream<Item = Result<Chunk, Error>> + Send>>;

/// A fixed set of wire adapters; enum dispatch keeps `async fn` simple.
#[derive(Clone, Debug)]
pub enum Provider {
    Anthropic(anthropic::Anthropic),
    OpenAi(openai::OpenAi),
    Compat(compat::Compat),
    Gemini(gemini::Gemini),
    Scripted(scripted::Scripted),
}

impl Provider {
    pub async fn stream(&self, request: &Request, credential: &Credential) -> Result<ChunkStream, Error> {
        match self {
            Self::Anthropic(provider) => provider.stream(request, credential).await,
            Self::OpenAi(provider) => provider.stream(request, credential).await,
            Self::Compat(provider) => provider.stream(request, credential).await,
            Self::Gemini(provider) => provider.stream(request, credential).await,
            Self::Scripted(provider) => provider.stream(request),
        }
    }
}

/// Builds the adapter for a catalog provider. `DRIFT_<ID>_BASE_URL` overrides the endpoint for recorded runs.
pub fn provider_for(id: &str, catalog_api: Option<&str>) -> Option<Provider> {
    let env_name = format!("DRIFT_{}_BASE_URL", id.to_uppercase().replace('-', "_"));
    let override_url = std::env::var(env_name).ok();
    let base = |default: &str| override_url.clone().or_else(|| catalog_api.map(str::to_string)).unwrap_or_else(|| default.to_string());
    Some(match id {
        "anthropic" => Provider::Anthropic(override_url.as_deref().map_or_else(anthropic::Anthropic::default, anthropic::Anthropic::new)),
        "openai" => Provider::OpenAi(override_url.as_deref().map_or_else(openai::OpenAi::default, openai::OpenAi::new)),
        "google" => Provider::Gemini(override_url.as_deref().map_or_else(gemini::Gemini::default, gemini::Gemini::new)),
        "xai" => Provider::Compat(compat::Compat::new(&base("https://api.x.ai/v1"))),
        "zai" => Provider::Compat(compat::Compat::new(&base("https://api.z.ai/api/paas/v4"))),
        "openrouter" => Provider::Compat(compat::Compat::new(&base("https://openrouter.ai/api/v1"))),
        "lmstudio" => Provider::Compat(compat::Compat::new(&base("http://127.0.0.1:1234/v1"))),
        "ollama" => Provider::Compat(compat::Compat::new(&base("http://127.0.0.1:11434/v1"))),
        _ => return None,
    })
}

/// Replays canned responses in order and records every request; tests and the conformance harness use it.
pub mod scripted {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use super::{Chunk, ChunkStream, Error, Request};

    #[derive(Debug)]
    enum Response {
        Chunks(Vec<Chunk>),
        Fail(Error),
        /// Streams the chunks, then the error, as an in-stream error frame does.
        FailMidway(Vec<Chunk>, Error),
        /// A response that never finishes, for exercising Stop.
        Stall,
    }

    type Responses = Arc<Mutex<VecDeque<Response>>>;

    #[derive(Clone, Debug, Default)]
    pub struct Scripted {
        responses: Responses,
        pub requests: Arc<Mutex<Vec<Request>>>,
    }

    impl Scripted {
        pub fn push(&self, chunks: Vec<Chunk>) -> &Self {
            self.responses.lock().unwrap().push_back(Response::Chunks(chunks));
            self
        }

        pub fn push_error(&self, error: Error) -> &Self {
            self.responses.lock().unwrap().push_back(Response::Fail(error));
            self
        }

        pub fn push_fail_midway(&self, chunks: Vec<Chunk>, error: Error) -> &Self {
            self.responses.lock().unwrap().push_back(Response::FailMidway(chunks, error));
            self
        }

        pub fn push_stall(&self) -> &Self {
            self.responses.lock().unwrap().push_back(Response::Stall);
            self
        }

        pub fn responses_left(&self) -> usize {
            self.responses.lock().unwrap().len()
        }

        pub fn stream(&self, request: &Request) -> Result<ChunkStream, Error> {
            self.requests.lock().unwrap().push(request.clone());
            match self.responses.lock().unwrap().pop_front() {
                Some(Response::Chunks(chunks)) => Ok(Box::pin(futures_util::stream::iter(chunks.into_iter().map(Ok)))),
                Some(Response::Fail(error)) => Err(error),
                Some(Response::FailMidway(chunks, error)) => Ok(Box::pin(futures_util::stream::iter(chunks.into_iter().map(Ok).chain([Err(error)])))),
                Some(Response::Stall) => Ok(Box::pin(futures_util::stream::pending())),
                None => Err(Error::Transport("scripted provider has no more responses".into())),
            }
        }
    }
}
