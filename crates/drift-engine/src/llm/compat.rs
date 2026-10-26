//! OpenAI Chat Completions over SSE: the dialect xAI, Z.ai, OpenRouter, LM Studio, Moonshot, DeepSeek and Alibaba speak.

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

mod dialect;
#[cfg(test)]
mod dialect_tests;
#[cfg(test)]
mod gateway_tests;
mod request;

#[cfg(test)]
use dialect::reason;
use dialect::{Thinking, moonshot_schemas, reasoning_on_every_reply};
#[cfg(test)]
use request::message;
use request::{body, claude_call_ids, mark_breakpoints, takes_breakpoints};

#[derive(Clone, Debug)]
pub struct Compat {
    base_url: String,
    client: reqwest::Client,
    pub timeouts: super::http::Timeouts,
    /// Which models get Anthropic's per-block `cache_control`, and on how many of the latest turns.
    breakpoints: Breakpoints,
    thinking: Thinking,
    /// How the route keeps one conversation on the server that holds its cache.
    affinity: Affinity,
    /// The output cap goes as `max_completion_tokens` (Alibaba) rather than `max_tokens`.
    completion_tokens: bool,
    /// Every assistant message carries `reasoning_content` (DeepSeek, Kimi, Qwen).
    every_reply_reasons: bool,
    /// Tool schemas rewritten to what Moonshot's validator takes.
    moonshot_schemas: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Breakpoints {
    None,
    /// OpenRouter passes them through to Claude and Qwen, on the last two turns.
    Gateway,
    /// Alibaba caches Qwen at them, on the system prompt and the last turn, as opencode marks it.
    Qwen,
}

impl Breakpoints {
    fn turns(self, model: &str) -> Option<usize> {
        match self {
            Self::Gateway if takes_breakpoints(model) => Some(2),
            Self::Qwen if model.starts_with("qwen") => Some(1),
            _ => None,
        }
    }
}

/// Where a conversation's cache key goes so the provider routes it to its warm cache.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Affinity {
    None,
    /// xAI's `x-grok-conv-id` header.
    Header(&'static str),
    /// A body field: OpenRouter's `session_id`, which pins the upstream provider, or Moonshot's `prompt_cache_key`.
    Body(&'static str),
}

impl Compat {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: super::http::client(),
            timeouts: super::http::Timeouts::default(),
            breakpoints: Breakpoints::None,
            thinking: Thinking::Effort,
            affinity: Affinity::None,
            completion_tokens: false,
            every_reply_reasons: false,
            moonshot_schemas: false,
        }
    }

    /// OpenRouter: forwards `cache_control` on content blocks to Claude, takes reasoning as an object, and keeps a
    /// session on one upstream provider.
    pub fn openrouter(base_url: &str) -> Self {
        Self {
            breakpoints: Breakpoints::Gateway,
            thinking: Thinking::Object,
            affinity: Affinity::Body("session_id"),
            ..Self::new(base_url)
        }
    }

    /// Z.ai: thinking on, with the reasoning of the turn's earlier steps kept.
    pub fn zai(base_url: &str) -> Self {
        Self {
            thinking: Thinking::Kept,
            ..Self::new(base_url)
        }
    }

    /// xAI: caches per server, so a conversation names itself to stay on one.
    pub fn xai(base_url: &str) -> Self {
        Self {
            affinity: Affinity::Header("x-grok-conv-id"),
            ..Self::new(base_url)
        }
    }

    /// Moonshot's Kimi, on its platform and its coding plans: a cache key, and schemas its validator takes.
    pub fn moonshot(base_url: &str) -> Self {
        Self {
            thinking: Thinking::Moonshot,
            affinity: Affinity::Body("prompt_cache_key"),
            every_reply_reasons: true,
            moonshot_schemas: true,
            ..Self::new(base_url)
        }
    }

    pub fn deepseek(base_url: &str) -> Self {
        Self {
            thinking: Thinking::DeepSeek,
            every_reply_reasons: true,
            ..Self::new(base_url)
        }
    }

    /// Alibaba's DashScope and coding plans: Qwen caches at breakpoints.
    pub fn alibaba(base_url: &str) -> Self {
        Self {
            breakpoints: Breakpoints::Qwen,
            thinking: Thinking::Alibaba,
            completion_tokens: true,
            every_reply_reasons: true,
            ..Self::new(base_url)
        }
    }

    /// The request body as this route takes it.
    fn shaped(&self, request: &Request) -> Value {
        let mut body = body(request);
        dialect::reason(&mut body, request.reasoning.as_ref(), self.thinking);
        if let Some(turns) = self
            .breakpoints
            .turns(&request.model)
            .filter(|_| request.cache_conversation)
        {
            mark_breakpoints(&mut body, request.cache_hour, turns);
        }
        if request.model.contains("claude") {
            claude_call_ids(&mut body);
        }
        // Explicit mode with no breakpoints writes nothing to the cache, which a one-off request would pay for unread.
        if !request.cache_conversation && request.cache_writes_billed && self.breakpoints != Breakpoints::Qwen {
            body["prompt_cache_options"] = json!({ "mode": "explicit" });
        }
        if let (Affinity::Body(field), Some(key)) = (self.affinity, &request.cache_key) {
            body[field] = json!(key);
        }
        self.dialect(&mut body);

        super::apply_mode(&mut body, request);

        body
    }

    /// What the route's own validator needs of a body already built.
    fn dialect(&self, body: &mut Value) {
        if self.completion_tokens
            && let Some(fields) = body.as_object_mut()
            && let Some(cap) = fields.remove("max_tokens")
        {
            fields.insert("max_completion_tokens".into(), cap);
        }
        if self.every_reply_reasons {
            reasoning_on_every_reply(body);
        }
        if self.moonshot_schemas {
            moonshot_schemas(body);
        }
    }

    pub async fn stream(&self, request: &Request, credential: &Credential) -> Result<ChunkStream, Error> {
        let key = match credential {
            Credential::ApiKey { key } => key.clone(),
            Credential::OAuth { access, .. } => access.clone(),
            Credential::Ambient { .. } => return Err(Error::Unauthenticated(String::new())),
        };

        let body = self.shaped(request);
        let mut http = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(key)
            .header("accept", "text/event-stream");
        if let (Affinity::Header(name), Some(key)) = (self.affinity, &request.cache_key) {
            http = http.header(name, key);
        }
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
    let message = super::error_message(text);
    let named = error["code"]
        .as_u64()
        .and_then(|code| u16::try_from(code).ok())
        .filter(|_| status == super::STREAMED);
    // xAI refuses a bad key with 400 and a generic code, said only in words.
    let bad_key = message.contains("Incorrect API key provided");
    match named.unwrap_or(status) {
        401 | 403 => Error::Unauthenticated(message),
        400 if bad_key => Error::Unauthenticated(message),
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
        if value["error"].is_object() || value["error"].is_string() {
            return Err(api_error(super::STREAMED, data));
        }

        if let Some(usage) = usage_of(&value) {
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

/// Top-level `usage`, or the choice's where Moonshot puts it.
fn usage_of(chunk: &Value) -> Option<&Value> {
    [&chunk["usage"], &chunk["choices"][0]["usage"]]
        .into_iter()
        .find(|usage| usage.is_object())
}

/// Cache hits are `prompt_tokens_details.cached_tokens` (OpenAI), top-level `cached_tokens` (Z.ai) or
/// `prompt_cache_hit_tokens` (DeepSeek).
fn usage_from(usage: &Value) -> Usage {
    let count = |key: &str| usage[key].as_u64().unwrap_or(0);
    let details = &usage["prompt_tokens_details"];
    let cache_read = [
        &details["cached_tokens"],
        &usage["cached_tokens"],
        &usage["prompt_cache_hit_tokens"],
    ]
    .into_iter()
    .find_map(Value::as_u64)
    .unwrap_or(0);
    let cache_write = details["cache_write_tokens"].as_u64().unwrap_or(0);
    Usage {
        input: count("prompt_tokens").saturating_sub(cache_read + cache_write),
        output: count("completion_tokens"),
        cache_read,
        cache_write,
    }
}

#[cfg(test)]
mod tests;
