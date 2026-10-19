//! Talking to models. One neutral request shape, one adapter per wire protocol, streamed chunks out.

pub mod anthropic;
pub mod aws;
pub mod bedrock;
pub mod catalog;
pub mod compat;
mod credential_file;
pub mod credentials;
mod eventstream;
mod files;
pub use files::prepare_files;
pub mod gemini;
pub mod google;
pub mod http;
pub mod limits;
pub mod local;
pub mod mask;
pub(crate) mod multipart;
mod oauth_error;
pub mod ollama;
pub mod openai;
pub use oauth_error::OAuthError;
#[cfg(test)]
mod retry_tests;
/// Replays canned responses in order and records every request; tests and the conformance harness use it.
pub mod scripted;
pub(crate) mod sse;
#[cfg(test)]
pub(crate) mod tests;
pub mod vertex;
pub mod xai;

use std::pin::Pin;

use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::session::types::{Refusal, Usage};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Credential {
    ApiKey {
        key: String,
    },
    OAuth {
        access: String,
        refresh: String,
        expires_at: i64,
        /// ChatGPT account id for Codex; absent for Anthropic.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<String>,
    },
    /// A cloud route's own credentials (AWS keys or profile, Google service account or ADC),
    /// read per request; never stored.
    Ambient {
        source: String,
    },
}

/// What the environment offers a cloud route, named for the provider list; `None` for other routes.
pub fn ambient(provider: &str) -> Option<Option<String>> {
    match provider {
        "amazon-bedrock" => Some(aws::detect()),
        "google-vertex" | "google-vertex-anthropic" => Some(google::detect()),
        _ => None,
    }
}

impl Credential {
    pub fn is_expired(&self) -> bool {
        matches!(self, Self::OAuth { expires_at, .. } if *expires_at < crate::id::now_ms())
    }
}

/// Images a request carries at most, newest first. Providers cap the count (Anthropic at 100, and
/// shrink limits past 20), and a long screenshot session must never grow past what they accept.
pub const MAX_IMAGES_SENT: usize = 10;

/// Image data a request carries at most, as base64: providers cap the whole request (Anthropic at
/// 32 MB), and ten images of 5 MB each would pass the count yet fail the size.
pub const MAX_IMAGE_DATA_SENT: usize = 20 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl ToolSpec {
    /// The input schema as every wire sends it: with an object root, which providers require of a tool's arguments
    /// while an MCP server may declare `{}` or a bare `properties` map.
    pub(crate) fn parameters(&self) -> Value {
        let Some(fields) = self.input_schema.as_object() else {
            return serde_json::json!({ "type": "object" });
        };
        if fields.contains_key("type") {
            return self.input_schema.clone();
        }

        // An empty struct's schema is an object-or-array union; every object matches its bare object branch.
        let bare_object = |branch: &Value| {
            branch
                .as_object()
                .is_some_and(|branch| branch.len() == 1 && branch.get("type") == Some(&Value::from("object")))
        };
        let mut root = fields.clone();
        if root
            .get("anyOf")
            .and_then(Value::as_array)
            .is_some_and(|branches| branches.iter().any(bare_object))
        {
            root.remove("anyOf");
        }
        root.insert("type".into(), Value::from("object"));

        Value::Object(root)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Role {
    User,
    Assistant,
}

/// One content block as the model sees it. Provider-specific fields ride along untouched.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Signed {
        part: Box<Block>,
        signature: String,
    },
    Text(String),
    /// signature is whatever the provider needs to accept the block back:
    /// Anthropic's signature, OpenAI's encrypted content.
    Reasoning {
        text: String,
        signature: Option<String>,
        redacted: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        call_id: String,
        content: String,
        is_error: bool,
    },
    Image {
        mime: String,
        base64: String,
    },
    /// A PDF sent whole, for a model that reads them.
    Pdf {
        base64: String,
    },
    /// An image or PDF a stored call returned, named by its blob; [`prepare_files`] loads it or
    /// replaces it with a line before any adapter sees the request.
    Stored {
        mime: String,
        hash: String,
    },
    /// An engine note wrapped in `<system-reminder>`, sent as that text, or as a `role: "system"` message on a
    /// Claude that takes them (`ClaudeFeatures::system_messages`). Never made from what a user or file says.
    Reminder(String),
    /// A reasoning effort change at this point of the conversation; request building lifts it into
    /// `Request::effort_marks`, so no adapter ever sees one.
    Effort(String),
}

impl Block {
    pub fn unsigned(&self) -> &Self {
        match self {
            Self::Signed { part, .. } => part.unsigned(),
            other => other,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatMessage {
    pub role: Role,
    pub blocks: Vec<Block>,
}

/// From after `messages[after]` on, the model reasons at `effort`.
#[derive(Clone, Debug, PartialEq)]
pub struct EffortMark {
    pub after: usize,
    pub effort: String,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub model: String,
    pub system: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: u32,
    pub reasoning: Option<catalog::Reasoning>,
    pub temperature: Option<f64>,
    /// The same for every request of one conversation tree, so providers that route by it (OpenAI's
    /// `prompt_cache_key`, the Codex `session-id` header) keep a conversation, its forks, spawned
    /// threads and workers on one cache.
    pub cache_key: Option<String>,
    /// The session itself, which keys a provider's live connection and its continuation; two
    /// sessions never share one, whatever cache they share.
    pub connection: Option<String>,
    /// Reasoning effort changes inside the conversation, each in effect after the message it follows,
    /// so the top-level effort and the cached prefix stay as they were (`Model::effort_updates`).
    pub effort_marks: Vec<EffortMark>,
    /// The newer Claude features the model takes; only the Anthropic wire reads them.
    pub claude: catalog::ClaudeFeatures,
    /// Ask for an hour-long prompt cache where the route takes one (Anthropic with an API key).
    pub cache_hour: bool,
    /// Tools stay defined so history with calls is valid, but the model may not call one.
    pub no_tool_calls: bool,
    /// OpenAI `text.verbosity`, from `catalog::verbosity`.
    pub verbosity: Option<&'static str>,
    /// Ask to see the model's thinking even with no level set (`catalog::shows_thinking`).
    pub show_thinking: bool,
    /// Sampling the model is tuned for (`catalog::sampling`), beside `temperature`.
    pub top_p: Option<f64>,
    pub top_k: Option<u32>,
    /// The catalog mode the chosen entry runs `model` in (fast, ultrafast, flex, pro).
    pub mode: Option<catalog::ModelMode>,
    /// The model's window in tokens, for servers that size it per request (Ollama's `num_ctx`); `None` when unknown.
    pub context_window: Option<u64>,
}

/// Lays the mode's body fields over the adapter's body, objects merged key by key
/// (`reasoning.mode` beside `reasoning.effort`).
pub(crate) fn apply_mode(body: &mut serde_json::Value, request: &Request) {
    fn merge(into: &mut serde_json::Value, value: &serde_json::Value) {
        match (into, value) {
            (serde_json::Value::Object(into), serde_json::Value::Object(value)) => {
                for (key, value) in value {
                    merge(into.entry(key.clone()).or_insert(serde_json::Value::Null), value);
                }
            }
            (into, value) => *into = value.clone(),
        }
    }

    for (key, value) in request.mode.iter().flat_map(|mode| &mode.body) {
        merge(&mut body[key.as_str()], value);
    }
}

/// The route's `anthropic-beta` list with any the mode adds, as one header, and the mode's other headers as given.
pub(crate) fn mode_headers<'a>(
    mut http: reqwest::RequestBuilder,
    request: &'a Request,
    mut betas: Vec<&'a str>,
) -> reqwest::RequestBuilder {
    for (name, value) in request.mode.iter().flat_map(|mode| &mode.headers) {
        if name.eq_ignore_ascii_case("anthropic-beta") {
            for beta in value.split(',').map(str::trim).filter(|beta| !beta.is_empty()) {
                if !betas.contains(&beta) {
                    betas.push(beta);
                }
            }
        } else {
            http = http.header(name, value);
        }
    }

    if !betas.is_empty() {
        http = http.header("anthropic-beta", betas.join(","));
    }

    http
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    /// The provider's safety filter ended the reply (Anthropic `refusal`, `content_filter`, Gemini `SAFETY`).
    Refused,
    /// The reply ran into the end of the context window (Anthropic `model_context_window_exceeded`).
    ContextFull,
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
    PartSignature(String),
    ReasoningRedacted(String),
    ToolUseStart {
        id: String,
        name: String,
    },
    ToolInputDelta(String),
    BlockStop,
    Usage(Usage),
    /// What the account that sent the request has spent of its subscription, from the response's headers or events.
    Limits(limits::Limits),
    /// What the provider said about a safety refusal, ahead of its `Stop(Refused)`.
    Refusal(Refusal),
    Stop(StopReason),
}

#[derive(Debug)]
pub enum Error {
    /// The provider answered with an error. `retryable` covers rate limits, overload and server
    /// faults; `retry_after` is the wait the provider asked for, when it named one.
    Api {
        status: u16,
        kind: String,
        message: String,
        retryable: bool,
        retry_after: Option<std::time::Duration>,
    },
    Transport(String),
    Malformed(String),
    /// The provider refused the credentials, in its own words; empty when there were none to send.
    Unauthenticated(String),
}

/// The status adapters give an error that arrives inside a stream which began with 200 OK.
pub const STREAMED: u16 = 200;
/// Timeouts, conflicts, rate limits, overload and server faults: worth another try.
const RETRY_STATUSES: [u16; 8] = [408, 409, 429, 500, 502, 503, 504, 529];
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

/// How a streamed fault says it is passing when its kind names nothing known (gateways, proxies), lowercased.
const RETRY_WORDS: [&str; 7] = [
    "try again",
    "overloaded",
    "temporarily unavailable",
    "service unavailable",
    "server is busy",
    "provider returned error",
    "upstream connect",
];

/// Kinds that say the request itself is wrong, whatever their words suggest.
const REQUEST_KINDS: [&str; 4] = [
    "invalid_request_error",
    "invalid_request",
    "invalid_argument",
    "invalid_prompt",
];

fn retry_words(kind: &str, message: &str) -> bool {
    let message = message.to_ascii_lowercase();

    !REQUEST_KINDS.contains(&kind.to_ascii_lowercase().as_str()) && RETRY_WORDS.iter().any(|w| message.contains(w))
}

/// Faults no wait will fix, whatever status they came with: OpenAI answers a spent balance with 429.
const PERMANENT_KINDS: [&str; 4] = [
    "insufficient_quota",
    "billing_hard_limit_reached",
    "billing_not_active",
    "access_terminated",
];

/// The same said in words, lowercased: Z.ai answers a spent balance or an expired plan with 429.
const SPENT_WORDS: [&str; 3] = [
    "insufficient balance",
    "package has expired",
    "plan does not yet include",
];

/// No wait will fix it: a permanent kind, a spent balance, or a plan without the model (another account may have it).
fn hopeless(kind: &str, message: &str) -> bool {
    let words = message.to_ascii_lowercase();

    PERMANENT_KINDS.contains(&kind.to_ascii_lowercase().as_str())
        || SPENT_WORDS.iter().any(|phrase| words.contains(phrase))
        || plan_lacks_model(message)
}

/// What providers say, lowercased, when the signed-in account's plan does not include the model, whatever
/// status and kind come with it: Anthropic answers 429 `rate_limit_error`, Codex 400.
const PLAN_LACKS_MODEL: [&str; 3] = [
    "usage credits are required",
    "not supported when using codex with a chatgpt account",
    "not available on your plan",
];

fn plan_lacks_model(message: &str) -> bool {
    let message = message.to_ascii_lowercase();

    PLAN_LACKS_MODEL.iter().any(|phrase| message.contains(phrase))
}

impl Error {
    /// A provider error, retryable by its status or, inside a stream, by what the provider calls it or says.
    /// A fault no wait will fix is never retryable.
    pub fn api(status: u16, kind: impl Into<String>, message: impl Into<String>) -> Self {
        let kind = kind.into();
        let message = message.into();
        let streamed_fault = RETRY_KINDS.contains(&kind.to_ascii_lowercase().as_str()) || retry_words(&kind, &message);
        let transient = RETRY_STATUSES.contains(&status) || (status == STREAMED && streamed_fault);
        let retryable = transient && !hopeless(&kind, &message);

        Self::Api {
            status,
            kind,
            message,
            retryable,
            retry_after: None,
        }
    }

    /// Takes what the response headers say about retrying: the wait the provider asks for, and its
    /// explicit `x-should-retry` verdict, which cannot make a permanent fault retryable.
    /// A refusal for spent subscription usage becomes `LIMIT_REACHED`, waiting until the usage frees.
    pub fn with_headers(mut self, headers: &::http::HeaderMap) -> Self {
        if let Self::Api {
            status,
            kind,
            message,
            retryable,
            retry_after,
        } = &mut self
        {
            *retry_after = requested_wait(headers).or(*retry_after);
            match headers.get("x-should-retry").and_then(|v| v.to_str().ok()) {
                Some("true") => *retryable = !hopeless(kind, message),
                Some("false") => *retryable = false,
                _ => {}
            }

            let spent = limits::Limits::from_headers(headers).and_then(|limits| limits.spent_until);
            if *status == 429 && (spent.is_some() || kind == limits::LIMIT_REACHED) {
                *kind = limits::LIMIT_REACHED.into();
                *retry_after = spent.map(wait_until).or(*retry_after);
            }
        }

        self
    }

    /// The headers a WebSocket error event carries in its body, taken as `with_headers` takes a response's.
    pub fn with_header_object(self, headers: &Value) -> Self {
        let Some(object) = headers.as_object() else {
            return self;
        };

        let map = object
            .iter()
            .filter_map(|(name, value)| {
                let name = ::http::HeaderName::from_bytes(name.as_bytes()).ok()?;
                let value =
                    ::http::HeaderValue::from_str(&value.as_str().map_or_else(|| value.to_string(), str::to_string))
                        .ok()?;
                Some((name, value))
            })
            .collect();

        self.with_headers(&map)
    }

    /// The account's usage is spent: switching accounts, not waiting, is the remedy.
    pub fn limit_reached(&self) -> bool {
        matches!(self, Self::Api { kind, .. } if kind == limits::LIMIT_REACHED)
    }

    /// The account's plan does not include the model; another account's may.
    pub fn plan_lacks_model(&self) -> bool {
        matches!(self, Self::Api { message, .. } if plan_lacks_model(message))
    }
}

/// The wait from now until `at`, in ms since the epoch.
fn wait_until(at: i64) -> std::time::Duration {
    std::time::Duration::from_millis(u64::try_from(at - crate::id::now_ms()).unwrap_or(0))
}

/// `retry-after-ms`, else `retry-after` as seconds or as an HTTP date. A wait too long to represent
/// saturates, so the caller sees it as longer than it will wait rather than failing to parse it.
fn requested_wait(headers: &::http::HeaderMap) -> Option<std::time::Duration> {
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

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api {
                status, kind, message, ..
            } => write!(f, "{kind} ({status}): {message}"),
            Self::Transport(message) => write!(f, "transport: {message}"),
            Self::Malformed(message) => write!(f, "malformed response: {message}"),
            Self::Unauthenticated(words) if words.is_empty() => write!(f, "no credentials for this provider"),
            Self::Unauthenticated(words) => write!(f, "the provider refused the credentials: {words}"),
        }
    }
}

impl std::error::Error for Error {}

/// How each provider says the request no longer fits the model's context window.
const OVERFLOW_PHRASES: [&str; 18] = [
    "prompt is too long",
    "context_length_exceeded",
    "maximum context length",
    "exceeds the context window",
    "input is too long",
    "too many input tokens",
    "input token count",
    // Anthropic's 413 for a request over its byte limit.
    "request_too_large",
    // Z.ai, in English or Chinese.
    "prompt too long",
    "prompt 超长",
    "exceeds maximum input length",
    "longer than the model's context length",
    "exceeded this model context window limit",
    "range of input length should be",
    "`inputs` tokens + `max_new_tokens`",
    // llama.cpp's server, under LM Studio and other local servers.
    "exceeds the available context size",
    "reduce the length of the messages",
    "context length exceeded",
];

impl Error {
    /// The request did not fit the model's context; compacting can recover it where retrying cannot.
    /// Codex reports it inside the stream, as a `response.failed`.
    pub fn is_context_overflow(&self) -> bool {
        let refused = matches!(
            self,
            Self::Api {
                status: 400 | 413 | STREAMED,
                retryable: false,
                ..
            }
        );

        refused && mentions_context_overflow(&self.to_string())
    }

    /// The provider could not read an image the request carried; dropping it can recover where retrying cannot.
    pub fn is_unreadable_image(&self) -> bool {
        let refused = matches!(self, Self::Api { status: 400, .. });
        let text = self.to_string().to_ascii_lowercase();

        refused && UNREADABLE_IMAGE_PHRASES.iter().any(|phrase| text.contains(phrase))
    }
}

/// How providers say they could not read an image: Anthropic's wording, seen on a corrupt JPEG on 2026-10-09.
const UNREADABLE_IMAGE_PHRASES: [&str; 1] = ["could not process image"];

/// Also used on errors already flattened to text, such as a failed summary request.
pub fn mentions_context_overflow(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    OVERFLOW_PHRASES.iter().any(|phrase| text.contains(phrase))
}

/// The readable message in an error body, from the layouts providers and gateways use (AWS's `Message`, RFC 9457's
/// `detail`); the body itself when none holds one.
pub(crate) fn error_message(text: &str) -> String {
    let body: Value = serde_json::from_str(text).unwrap_or_default();
    let layouts = [
        &body["error"]["message"],
        &body["error"],
        &body["message"],
        &body["Message"],
        &body["detail"],
        &body["errors"][0]["message"],
    ];

    layouts
        .into_iter()
        .filter_map(Value::as_str)
        .find(|message| !message.trim().is_empty())
        .unwrap_or(text)
        .to_string()
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
        assert!(api(400, "Could not process image").is_unreadable_image());
        assert!(!api(400, "Could not process image").is_context_overflow());
        assert!(
            !api(500, "Could not process image").is_unreadable_image(),
            "a server fault retries instead"
        );
        assert!(!api(400, "prompt is too long").is_unreadable_image());
        assert!(
            !api(429, "prompt is too long").is_context_overflow(),
            "a rate limit is not an overflow"
        );
        assert!(!Error::Transport("prompt is too long".into()).is_context_overflow());
    }

    #[test]
    fn an_error_bodys_message_is_found_in_each_common_layout() {
        for body in [
            r#"{"error":{"message":"bad key"}}"#,
            r#"{"error":"bad key"}"#,
            r#"{"message":"bad key"}"#,
            r#"{"Message":"bad key"}"#,
            r#"{"detail":"bad key"}"#,
            r#"{"errors":[{"message":"bad key"}]}"#,
            r#"{"error":{"message":" "},"message":"bad key"}"#,
        ] {
            assert_eq!(super::error_message(body), "bad key", "{body}");
        }
        for body in ["upstream timed out", r#"{"error":{"code":500}}"#, ""] {
            assert_eq!(
                super::error_message(body),
                body,
                "the body itself when nothing says more"
            );
        }
    }

    #[test]
    fn more_servers_wordings_and_overflows_reported_inside_a_stream_are_recognised() {
        for message in [
            "Prompt too long",
            "Prompt 超长",
            "Requested input length 140000 exceeds maximum input length 131072",
            "Input (210000 tokens) is longer than the model's context length (131072 tokens).",
            "AiError: Exceeded this model context window limit",
            "Range of input length should be [1, 129024]",
            "`inputs` tokens + `max_new_tokens` must be <= 32769. Given: 40000 `inputs` tokens",
            "the request exceeds the available context size, try increasing it",
            "Please reduce the length of the messages.",
            "context length exceeded",
        ] {
            assert!(api(400, message).is_context_overflow(), "{message}");
        }
        let too_large = Error::api(
            413,
            "request_too_large",
            "Request exceeds the maximum allowed number of bytes.",
        );
        assert!(too_large.is_context_overflow(), "Anthropic's 413");

        let streamed = Error::api(
            crate::llm::STREAMED,
            "context_length_exceeded",
            "Your input exceeds the context window of this model.",
        );
        assert!(streamed.is_context_overflow(), "Codex reports it in response.failed");
        let throttled = Error::api(
            crate::llm::STREAMED,
            "rate_limit_exceeded",
            "too many input tokens per minute",
        );
        assert!(!throttled.is_context_overflow(), "a retryable fault is not an overflow");
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
    Bedrock(bedrock::Bedrock),
    Vertex(vertex::Vertex),
    Ollama(ollama::Ollama),
    Scripted(scripted::Scripted),
}

impl Provider {
    /// The same route with other time limits.
    pub fn with_timeouts(mut self, timeouts: http::Timeouts) -> Self {
        if let Some(slot) = self.timeouts_mut() {
            *slot = timeouts;
        }

        self
    }

    pub fn timeouts(&self) -> Option<http::Timeouts> {
        match self {
            Self::Anthropic(provider) => Some(provider.timeouts),
            Self::OpenAi(provider) => Some(provider.timeouts),
            Self::Compat(provider) => Some(provider.timeouts),
            Self::Gemini(provider) => Some(provider.timeouts),
            Self::Bedrock(provider) => Some(provider.timeouts),
            Self::Vertex(provider) => Some(provider.timeouts),
            Self::Ollama(provider) => Some(provider.timeouts),
            Self::Scripted(_) => None,
        }
    }

    fn timeouts_mut(&mut self) -> Option<&mut http::Timeouts> {
        match self {
            Self::Anthropic(provider) => Some(&mut provider.timeouts),
            Self::OpenAi(provider) => Some(&mut provider.timeouts),
            Self::Compat(provider) => Some(&mut provider.timeouts),
            Self::Gemini(provider) => Some(&mut provider.timeouts),
            Self::Bedrock(provider) => Some(&mut provider.timeouts),
            Self::Vertex(provider) => Some(&mut provider.timeouts),
            Self::Ollama(provider) => Some(&mut provider.timeouts),
            Self::Scripted(_) => None,
        }
    }

    pub async fn stream(&self, request: &Request, credential: &Credential) -> Result<ChunkStream, Error> {
        match self {
            Self::Anthropic(provider) => provider.stream(request, credential).await,
            Self::OpenAi(provider) => provider.stream(request, credential).await,
            Self::Compat(provider) => provider.stream(request, credential).await,
            Self::Gemini(provider) => provider.stream(request, credential).await,
            Self::Bedrock(provider) => provider.stream(request, credential).await,
            Self::Vertex(provider) => provider.stream(request, credential).await,
            Self::Ollama(provider) => provider.stream(request, credential).await,
            Self::Scripted(provider) => provider.stream(request, credential),
        }
    }
}

/// Builds the adapter for a catalog provider. `DRIFT_<ID>_BASE_URL` overrides the endpoint for recorded
/// runs; otherwise the catalog's (for the native routes, only ever the user's drift.json). An id the
/// adapters do not know is an OpenAI-compatible server when it has an endpoint.
pub fn provider_for(id: &str, catalog_api: Option<&str>) -> Option<Provider> {
    let env_name = format!("DRIFT_{}_BASE_URL", id.to_uppercase().replace('-', "_"));
    let override_url = std::env::var(env_name).ok().or_else(|| catalog_api.map(str::to_string));
    let base = |default: &str| override_url.clone().unwrap_or_else(|| default.to_string());

    let provider = match id {
        "anthropic" => Provider::Anthropic(
            override_url
                .as_deref()
                .map_or_else(anthropic::Anthropic::default, anthropic::Anthropic::new),
        ),
        "openai" => Provider::OpenAi(
            override_url
                .as_deref()
                .map_or_else(openai::OpenAi::default, openai::OpenAi::new),
        ),
        "google" => Provider::Gemini(
            override_url
                .as_deref()
                .map_or_else(gemini::Gemini::default, gemini::Gemini::new),
        ),
        "xai" => Provider::Compat(compat::Compat::new(&base("https://api.x.ai/v1"))),
        "zai" => Provider::Compat(compat::Compat::zai(&base("https://api.z.ai/api/paas/v4"))),
        "openrouter" => Provider::Compat(compat::Compat::openrouter(&base("https://openrouter.ai/api/v1"))),
        "lmstudio" => Provider::Compat(compat::Compat::new(&base("http://127.0.0.1:1234/v1"))),
        "ollama" => Provider::Ollama(ollama::Ollama::new(&base(ollama::LOCAL_ROOT))),
        "ollama-cloud" => Provider::Ollama(ollama::Ollama::new(&base(ollama::CLOUD_ROOT))),
        "amazon-bedrock" => Provider::Bedrock(bedrock::Bedrock::new(override_url)),
        "google-vertex" | "google-vertex-anthropic" => Provider::Vertex(vertex::Vertex::new(override_url)),
        _ => Provider::Compat(compat::Compat::new(override_url.as_deref()?)),
    };

    Some(provider.with_timeouts(http::Timeouts::for_route(id)))
}
