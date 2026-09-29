//! Talking to models. One neutral request shape, one adapter per wire protocol, streamed chunks out.

pub mod anthropic;
pub mod catalog;
pub mod credentials;
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
    OAuth { access: String, refresh: String, expires_at: i64 },
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
    pub temperature: Option<f32>,
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
    Scripted(scripted::Scripted),
}

impl Provider {
    pub async fn stream(&self, request: &Request, credential: &Credential) -> Result<ChunkStream, Error> {
        match self {
            Self::Anthropic(provider) => provider.stream(request, credential).await,
            Self::Scripted(provider) => provider.stream(request),
        }
    }
}

/// Replays canned responses in order and records every request; tests and the conformance harness use it.
pub mod scripted {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use super::{Chunk, ChunkStream, Error, Request};

    type Responses = Arc<Mutex<VecDeque<Result<Vec<Chunk>, Error>>>>;

    #[derive(Clone, Debug, Default)]
    pub struct Scripted {
        responses: Responses,
        pub requests: Arc<Mutex<Vec<Request>>>,
    }

    impl Scripted {
        pub fn push(&self, chunks: Vec<Chunk>) -> &Self {
            self.responses.lock().unwrap().push_back(Ok(chunks));
            self
        }

        pub fn push_error(&self, error: Error) -> &Self {
            self.responses.lock().unwrap().push_back(Err(error));
            self
        }

        pub fn stream(&self, request: &Request) -> Result<ChunkStream, Error> {
            self.requests.lock().unwrap().push(request.clone());
            let next = self.responses.lock().unwrap().pop_front();
            let chunks = next.unwrap_or_else(|| Err(Error::Transport("scripted provider has no more responses".into())))?;
            Ok(Box::pin(futures_util::stream::iter(chunks.into_iter().map(Ok))))
        }
    }
}
