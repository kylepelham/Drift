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
    /// The provider answered with an error. `retryable` covers rate limits and overload.
    Api { status: u16, kind: String, message: String, retryable: bool },
    Transport(String),
    Malformed(String),
    Unauthenticated,
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
        Error::Api { status, kind: "invalid_request_error".into(), message: message.into(), retryable: false }
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
                Some(Response::Stall) => Ok(Box::pin(futures_util::stream::pending())),
                None => Err(Error::Transport("scripted provider has no more responses".into())),
            }
        }
    }
}
