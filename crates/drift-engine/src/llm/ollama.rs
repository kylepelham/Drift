//! Ollama's own chat API, `/api/chat`, streamed as one JSON object per line, for a local server and Ollama
//! Cloud alike. Unlike Ollama's OpenAI-compatible `/v1`, it takes the window to run a model with (`num_ctx`),
//! so a long conversation is not cut at the smaller window the server would otherwise load.

use futures_util::StreamExt;
use serde_json::Value;

use super::{Chunk, ChunkStream, Credential, Error, Request, StopReason};
use crate::session::types::Usage;

mod request;
#[cfg(test)]
mod tests;

/// Where a local Ollama listens unless drift.json says otherwise.
pub const LOCAL_ROOT: &str = "http://127.0.0.1:11434";
/// Ollama Cloud's own API root.
pub const CLOUD_ROOT: &str = "https://ollama.com";

#[derive(Clone, Debug)]
pub struct Ollama {
    root: String,
    client: reqwest::Client,
    pub timeouts: super::http::Timeouts,
}

impl Ollama {
    /// `base` is the server's root or its OpenAI-compatible `/v1`, as models.dev and older drift.json files name it.
    pub fn new(base: &str) -> Self {
        Self {
            root: root(base),
            client: super::http::client(),
            timeouts: super::http::Timeouts::default(),
        }
    }

    pub async fn stream(&self, request: &Request, credential: &Credential) -> Result<ChunkStream, Error> {
        let key = match credential {
            Credential::ApiKey { key } => key.clone(),
            Credential::OAuth { access, .. } => access.clone(),
            Credential::Ambient { .. } => return Err(Error::Unauthenticated(String::new())),
        };

        // A local server takes no key; the engine hands it the placeholder `none`.
        let mut http = self
            .client
            .post(format!("{}/api/chat", self.root))
            .header("accept", "application/x-ndjson");
        if !key.is_empty() && key != "none" {
            http = http.bearer_auth(key);
        }
        let sending = super::mode_headers(http, request, Vec::new()).json(&request::body(request));

        let response = super::http::send(sending, &self.timeouts).await?;
        let status = response.status();
        if !status.is_success() {
            let headers = response.headers().clone();
            let text = super::http::bounded_body(response, &self.timeouts).await;

            return Err(api_error(status.as_u16(), &text).with_headers(&headers));
        }

        let mut state = StreamState::default();
        let lines = super::sse::lines(response.bytes_stream(), self.timeouts.idle);
        Ok(Box::pin(lines.flat_map(move |line| {
            let items: Vec<Result<Chunk, Error>> = match line {
                Err(error) => vec![Err(Error::Transport(error.to_string()))],
                Ok(line) => match state.chunks(&line) {
                    Ok(chunks) => chunks.into_iter().map(Ok).collect(),
                    Err(error) => vec![Err(error)],
                },
            };
            futures_util::stream::iter(items)
        })))
    }
}

/// The server's root from either form of its address.
pub fn root(base: &str) -> String {
    let base = base.trim_end_matches('/');

    base.strip_suffix("/v1").unwrap_or(base).to_string()
}

/// Ollama answers `{"error": "..."}`; a missing key or model is said in words only.
fn api_error(status: u16, text: &str) -> Error {
    let message = super::error_message(text);

    match status {
        401 | 403 => Error::Unauthenticated(message),
        status => Error::api(status, "api_error", message),
    }
}

/// Text and thinking stream as they come. Ollama sends each tool call whole, so calls are held and handed on
/// at the end, after the reply's text.
#[derive(Default)]
struct StreamState {
    open: Option<Open>,
    calls: Vec<Call>,
}

struct Call {
    id: String,
    name: String,
    arguments: Value,
}

#[derive(Clone, Copy, PartialEq)]
enum Open {
    Text,
    Reasoning,
}

impl StreamState {
    fn chunks(&mut self, line: &str) -> Result<Vec<Chunk>, Error> {
        let value: Value = serde_json::from_str(line).map_err(|error| Error::Malformed(error.to_string()))?;
        if let Some(error) = value["error"].as_str() {
            return Err(Error::api(super::STREAMED, "api_error", error));
        }

        let mut out = Vec::new();
        let message = &value["message"];

        // Thinking, then the answer, each as a block of its own.
        if let Some(thought) = message["thinking"].as_str().filter(|thought| !thought.is_empty()) {
            out.extend(self.switch(Open::Reasoning));
            out.push(Chunk::ReasoningDelta(thought.into()));
        }
        if let Some(said) = message["content"].as_str().filter(|said| !said.is_empty()) {
            out.extend(self.switch(Open::Text));
            out.push(Chunk::TextDelta(said.into()));
        }
        for call in message["tool_calls"].as_array().into_iter().flatten() {
            self.calls.push(Call {
                id: call["id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .map_or_else(|| crate::id::new("call"), str::to_string),
                name: call["function"]["name"].as_str().unwrap_or_default().to_string(),
                arguments: call["function"]["arguments"].clone(),
            });
        }

        if value["done"] == true {
            out.extend(self.done(&value));
        }

        Ok(out)
    }

    fn switch(&mut self, next: Open) -> Vec<Chunk> {
        if self.open == Some(next) {
            return Vec::new();
        }

        let mut out = Vec::new();
        if self.open.take().is_some() {
            out.push(Chunk::BlockStop);
        }
        self.open = Some(next);
        out.push(match next {
            Open::Text => Chunk::TextStart,
            Open::Reasoning => Chunk::ReasoningStart,
        });

        out
    }

    /// The last line: the held calls, what the request cost and why it stopped.
    fn done(&mut self, value: &Value) -> Vec<Chunk> {
        let mut out = Vec::new();
        if self.open.take().is_some() {
            out.push(Chunk::BlockStop);
        }

        let calls = std::mem::take(&mut self.calls);
        let called = !calls.is_empty();
        for call in calls {
            out.push(Chunk::ToolUseStart {
                id: call.id,
                name: call.name,
            });
            if !call.arguments.is_null() {
                out.push(Chunk::ToolInputDelta(call.arguments.to_string()));
            }
            out.push(Chunk::BlockStop);
        }

        let count = |key: &str| value[key].as_u64().unwrap_or(0);
        out.push(Chunk::Usage(Usage {
            input: count("prompt_eval_count"),
            output: count("eval_count"),
            cache_read: 0,
            cache_write: 0,
        }));

        let stop = match value["done_reason"].as_str().unwrap_or("stop") {
            "length" => StopReason::MaxTokens,
            "stop" if called => StopReason::ToolUse,
            "stop" => StopReason::EndTurn,
            _ => StopReason::Other,
        };
        out.push(Chunk::Stop(stop));

        out
    }
}
