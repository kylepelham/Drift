//! OpenAI Chat Completions over SSE: the dialect xAI, Z.ai, OpenRouter, LM Studio and Ollama all speak.

use std::collections::BTreeMap;

use futures_util::StreamExt;
use serde_json::{json, Value};

use super::catalog::Reasoning;
use super::sse;
use super::{Block, ChatMessage, Chunk, ChunkStream, Credential, Error, Request, Role, StopReason};
use crate::session::types::Usage;

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
        Self { claude_breakpoints: true, reasoning_object: true, ..Self::new(base_url) }
    }

    /// Z.ai: thinking on, with the reasoning of the turn's earlier steps kept.
    pub fn zai(base_url: &str) -> Self {
        Self { keep_thinking: true, ..Self::new(base_url) }
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
        body
    }

    pub async fn stream(&self, request: &Request, credential: &Credential) -> Result<ChunkStream, Error> {
        let key = match credential {
            Credential::ApiKey { key } => key.clone(),
            Credential::OAuth { access, .. } => access.clone(),
            Credential::Ambient { .. } => return Err(Error::Unauthenticated(String::new())),
        };
        let body = self.shaped(request);
        let sending = self.client.post(format!("{}/chat/completions", self.base_url)).bearer_auth(key).header("accept", "text/event-stream").json(&body);
        let response = super::http::send(sending, &self.timeouts).await?;
        let status = response.status();
        if !status.is_success() {
            let headers = response.headers().clone();
            return Err(api_error(status.as_u16(), &super::http::bounded_body(response, &self.timeouts).await).with_headers(&headers));
        }
        let mut state = StreamState::default();
        let events = sse::events(response.bytes_stream(), self.timeouts.idle);
        Ok(Box::pin(events.flat_map(move |event| {
            let items: Vec<Result<Chunk, Error>> = match event {
                Err(error) => vec![Err(Error::Transport(error))],
                Ok(event) => match state.chunks(&event.data) {
                    Ok(chunks) => chunks.into_iter().map(Ok).collect(),
                    Err(error) => vec![Err(error)],
                },
            };
            futures_util::stream::iter(items)
        })))
    }
}

fn body(request: &Request) -> Value {
    let mut messages = Vec::new();
    if !request.system.is_empty() {
        messages.push(json!({ "role": "system", "content": request.system }));
    }
    // Reasoning from turns already over was left out by the session, which knows where its turn began.
    messages.extend(request.messages.iter().flat_map(message));
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
        "max_tokens": request.max_tokens,
    });
    if !request.tools.is_empty() {
        body["tools"] = request
            .tools
            .iter()
            .map(|tool| json!({ "type": "function", "function": { "name": tool.name, "description": tool.description, "parameters": tool.input_schema } }))
            .collect();
        if request.no_tool_calls {
            body["tool_choice"] = json!("none");
        }
    }
    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(top_p) = request.top_p {
        body["top_p"] = json!(top_p);
    }
    body
}

fn reason(body: &mut Value, reasoning: Option<&Reasoning>, object: bool) {
    match (reasoning, object) {
        (Some(Reasoning::Effort { level }), true) => body["reasoning"] = json!({ "effort": level }),
        (Some(Reasoning::Budget { tokens }), true) => body["reasoning"] = json!({ "max_tokens": tokens }),
        (Some(Reasoning::Effort { level }), false) => body["reasoning_effort"] = json!(level),
        _ => {}
    }
}

/// OpenRouter names Claude `anthropic/...`, or `~anthropic/...` for its moving aliases.
fn is_claude(model: &str) -> bool {
    model.trim_start_matches('~').starts_with("anthropic/")
}

/// Claude's three explicit breakpoints, as the Anthropic adapter places them: the system prompt and the
/// last two user turns. A turn here is the run of `tool` and `user` messages between assistant replies,
/// so a tool loop keeps caching past the prompt that started it.
fn mark_breakpoints(body: &mut Value) {
    let Some(messages) = body["messages"].as_array_mut() else { return };
    if let Some(system) = messages.iter_mut().find(|m| m["role"] == "system") {
        let text = system["content"].as_str().unwrap_or_default().to_string();
        system["content"] = json!([{ "type": "text", "text": text, "cache_control": ephemeral() }]);
    }
    let mut marked = 0;
    let mut in_turn = false;
    for message in messages.iter_mut().rev() {
        let turn = message["role"] == "user" || message["role"] == "tool";
        if !turn {
            in_turn = false;
            continue;
        }
        if in_turn || marked == 2 {
            continue;
        }
        if mark_last_text(message) {
            in_turn = true;
            marked += 1;
        }
    }
}

fn ephemeral() -> Value {
    json!({ "type": "ephemeral" })
}

/// Marks a message's last non-empty text part; a `tool` message's plain content becomes one part to carry it.
fn mark_last_text(message: &mut Value) -> bool {
    if let Some(text) = message["content"].as_str().filter(|text| !text.is_empty()).map(str::to_string) {
        message["content"] = json!([{ "type": "text", "text": text, "cache_control": ephemeral() }]);
        return true;
    }
    let last = message["content"].as_array_mut().and_then(|parts| parts.iter_mut().rev().find(|p| p["type"] == "text" && p["text"] != ""));
    match last {
        Some(part) => {
            part["cache_control"] = ephemeral();
            true
        }
        None => false,
    }
}

/// Tool results are their own `tool` messages; everything else folds into one message per role.
fn message(message: &ChatMessage) -> Vec<Value> {
    let mut out = Vec::new();
    let mut content: Vec<Value> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut reasoning: Option<String> = None;
    for block in &message.blocks {
        match block.unsigned() {
            Block::Signed { .. } => unreachable!("unsigned blocks cannot be signed"),
            Block::Text(text) => content.push(json!({ "type": "text", "text": text })),
            Block::Image { mime, base64 } => content.push(json!({ "type": "image_url", "image_url": { "url": format!("data:{mime};base64,{base64}") } })),
            Block::Reasoning { text, .. } => reasoning = Some(text.clone()),
            Block::ToolUse { id, name, input } => tool_calls.push(json!({ "id": id, "type": "function", "function": { "name": name, "arguments": input.to_string() } })),
            Block::ToolResult { call_id, content, .. } => out.push(json!({ "role": "tool", "tool_call_id": call_id, "content": content })),
            Block::Pdf { base64 } => content.push(json!({ "type": "file", "file": { "filename": "document.pdf", "file_data": format!("data:application/pdf;base64,{base64}") } })),
            Block::Stored { .. } => {}
        }
    }
    if content.is_empty() && tool_calls.is_empty() {
        return out;
    }
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let mut item = json!({ "role": role });
    item["content"] = if message.role == Role::Assistant {
        Value::String(content.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join(""))
    } else {
        Value::Array(content)
    };
    if !tool_calls.is_empty() {
        item["tool_calls"] = Value::Array(tool_calls);
    }
    if let Some(reasoning) = reasoning.filter(|_| message.role == Role::Assistant) {
        item["reasoning_content"] = Value::String(reasoning);
    }
    // Tool messages must follow the assistant's calls directly; text in the same turn comes after them.
    out.push(item);
    out
}


/// Gateways put an HTTP code inside a streamed error object; it classifies like the status it names.
fn api_error(status: u16, text: &str) -> Error {
    let parsed: Value = serde_json::from_str(text).unwrap_or_default();
    let error = if parsed["error"].is_object() { &parsed["error"] } else { &parsed };
    let kind = error["type"].as_str().or(error["code"].as_str()).unwrap_or("api_error").to_string();
    let message = error["message"].as_str().unwrap_or(text).to_string();
    let named = error["code"].as_u64().and_then(|code| u16::try_from(code).ok()).filter(|_| status == super::STREAMED);
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
        let Some(choice) = value["choices"].get(0) else { return Ok(out) };
        let delta = &choice["delta"];
        if let Some(text) = delta["reasoning_content"].as_str().or(delta["reasoning"].as_str()).filter(|t| !t.is_empty()) {
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
            let id = if call.id.is_empty() { crate::id::new("call") } else { call.id };
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
    Usage { input: count("prompt_tokens").saturating_sub(cache_read + cache_write), output: count("completion_tokens"), cache_read, cache_write }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolSpec;

    #[test]
    fn a_pdf_is_a_file_part() {
        let sent = message(&ChatMessage { role: Role::User, blocks: vec![Block::Pdf { base64: "JVBERi0=".into() }] });
        assert_eq!(sent[0]["content"][0]["file"]["file_data"], "data:application/pdf;base64,JVBERi0=");
    }

    #[test]
    fn zai_keeps_thinking_and_tuned_sampling_is_sent() {
        let tuned = Request { temperature: Some(1.0), top_p: Some(0.95), top_k: Some(40), ..request() };
        let zai = Compat::zai("https://z.example").shaped(&tuned);
        assert_eq!(zai["thinking"], json!({ "type": "enabled", "clear_thinking": false }));
        assert_eq!((zai["temperature"].as_f64(), zai["top_p"].as_f64()), (Some(1.0), Some(0.95)));
        assert!(zai.get("top_k").is_none(), "not a Chat Completions field");
        assert!(Compat::new("https://other.example").shaped(&tuned).get("thinking").is_none());
    }

    #[test]
    fn a_text_only_request_keeps_its_tools_but_forbids_calls() {
        assert!(body(&request()).get("tool_choice").is_none());
        let built = body(&Request { no_tool_calls: true, ..request() });
        assert_eq!((built["tool_choice"].clone(), built["tools"].as_array().map(Vec::len).unwrap_or(0) > 0), (json!("none"), true));
    }

    fn request() -> Request {
        Request {
            model: "grok-4.5".into(),
            system: "sys".into(),
            messages: vec![
                ChatMessage { role: Role::User, blocks: vec![Block::Text("hi".into())] },
                ChatMessage {
                    role: Role::Assistant,
                    blocks: vec![
                        Block::Reasoning { text: "hm".into(), signature: None, redacted: None },
                        Block::Text("ok".into()),
                        Block::ToolUse { id: "call_1".into(), name: "read".into(), input: json!({ "path": "a" }) },
                    ],
                },
                ChatMessage { role: Role::User, blocks: vec![Block::ToolResult { call_id: "call_1".into(), content: "1: x".into(), is_error: false }] },
            ],
            tools: vec![ToolSpec { name: "read".into(), description: "r".into(), input_schema: json!({ "type": "object" }) }],
            max_tokens: 500,
            reasoning: None,
            temperature: Some(0.2),
            cache_key: None,
            no_tool_calls: false,
            verbosity: None,
            show_thinking: false,
            top_p: None,
            top_k: None,
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
        assert_eq!(reasoned(effort(), true), json!({ "reasoning": { "effort": "high" } }), "OpenRouter");
        assert_eq!(reasoned(Reasoning::Budget { tokens: 8000 }, true), json!({ "reasoning": { "max_tokens": 8000 } }));
        assert_eq!(reasoned(Reasoning::Budget { tokens: 8000 }, false), json!({}), "no budget field to carry it");
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
        assert_eq!(feed(&mut state, r#"{"choices":[{"delta":{"role":"assistant","content":""}}]}"#), vec![]);
        assert_eq!(feed(&mut state, r#"{"choices":[{"delta":{"reasoning_content":"th"}}]}"#), vec![Chunk::ReasoningStart, Chunk::ReasoningDelta("th".into())]);
        assert_eq!(feed(&mut state, r#"{"choices":[{"delta":{"content":"Hi"}}]}"#), vec![Chunk::BlockStop, Chunk::TextStart, Chunk::TextDelta("Hi".into())]);
        assert_eq!(feed(&mut state, r#"{"choices":[{"delta":{"content":"!"}}]}"#), vec![Chunk::TextDelta("!".into())]);
        assert_eq!(feed(&mut state, r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"read","arguments":""}}]}}]}"#), vec![]);
        assert_eq!(feed(&mut state, r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"p\":1}"}}]}}]}"#), vec![]);
        assert_eq!(feed(&mut state, r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#), vec![]);
        assert_eq!(feed(&mut state, r#"{"choices":[],"usage":{"prompt_tokens":50,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":20}}}"#), vec![]);
        assert_eq!(
            feed(&mut state, "[DONE]"),
            vec![
                Chunk::BlockStop,
                Chunk::ToolUseStart { id: "call_a".into(), name: "read".into() },
                Chunk::ToolInputDelta("{\"p\":1}".into()),
                Chunk::BlockStop,
                Chunk::Usage(Usage { input: 30, output: 7, cache_read: 20, cache_write: 0 }),
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
            assert!(state.chunks(&format!(r#"{{"choices":[{{"delta":{{"tool_calls":[{delta}]}}}}]}}"#)).unwrap().is_empty());
        }
        state.chunks(r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#).unwrap();
        let out = state.chunks("[DONE]").unwrap();
        let starts: Vec<&Chunk> = out.iter().filter(|c| matches!(c, Chunk::ToolUseStart { .. })).collect();
        assert_eq!(starts.len(), 2, "one start per call: {out:?}");
        assert!(out.contains(&Chunk::ToolInputDelta("{\"path\":\"a.txt\"}".into())) && out.contains(&Chunk::ToolInputDelta("{\"pattern\":\"x\"}".into())));
    }

    #[test]
    fn a_stream_that_never_says_why_it_finished_is_refused_with_its_calls() {
        let mut state = StreamState::default();
        state.chunks(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"bash","arguments":"{\"command\":\"rm -rf"}}]}}]}"#).unwrap();
        let error = state.chunks("[DONE]").unwrap_err();
        assert!(error.to_string().contains("without a finish reason"), "{error}");
    }

    fn conversation(model: &str) -> Request {
        let user = |text: &str| ChatMessage { role: Role::User, blocks: vec![Block::Text(text.into())] };
        let assistant = ChatMessage { role: Role::Assistant, blocks: vec![Block::Text("ok".into())] };
        Request { model: model.into(), messages: vec![user("one"), assistant.clone(), user("two"), assistant, user("three")], ..request() }
    }

    fn marked(body: &Value) -> Vec<String> {
        let mut out = Vec::new();
        for message in body["messages"].as_array().unwrap() {
            for part in message["content"].as_array().into_iter().flatten().filter(|p| p.get("cache_control").is_some()) {
                out.push(format!("{}:{}", message["role"].as_str().unwrap(), part["text"].as_str().unwrap()));
            }
        }
        out
    }

    #[test]
    fn claude_through_a_caching_gateway_gets_the_anthropic_breakpoints_and_nothing_else_does() {
        let mut body = body(&conversation("anthropic/claude-sonnet-4.5"));
        mark_breakpoints(&mut body);
        assert_eq!(marked(&body), ["system:sys", "user:two", "user:three"]);
        assert!(is_claude("~anthropic/claude-sonnet-latest") && !is_claude("openai/gpt-6"));
        let usage = usage_from(&json!({ "prompt_tokens": 10_339, "completion_tokens": 60, "prompt_tokens_details": { "cached_tokens": 10_000, "cache_write_tokens": 300 } }));
        assert_eq!(usage, Usage { input: 39, output: 60, cache_read: 10_000, cache_write: 300 });
    }

    #[test]
    fn tool_messages_follow_the_calls_and_text_in_that_turn_comes_after_them() {
        let results = ChatMessage {
            role: Role::User,
            blocks: vec![
                Block::ToolResult { call_id: "call_1".into(), content: "1: x".into(), is_error: false },
                Block::Text("The read call (call_1) returned this:".into()),
                Block::Text("and also look at b".into()),
            ],
        };
        let mut request = request();
        request.messages[2] = results;
        let built = body(&request);
        let roles: Vec<&str> = built["messages"].as_array().unwrap().iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["system", "user", "assistant", "tool", "user"]);
    }

    #[test]
    fn reasoning_given_goes_back_on_its_own_assistant_message() {
        let thinking = |text: &str| ChatMessage {
            role: Role::Assistant,
            blocks: vec![Block::Reasoning { text: text.into(), signature: None, redacted: None }, Block::ToolUse { id: format!("c_{text}"), name: "read".into(), input: json!({}) }],
        };
        let result = |text: &str| ChatMessage { role: Role::User, blocks: vec![Block::ToolResult { call_id: format!("c_{text}"), content: "r".into(), is_error: false }] };
        let prompt = |text: &str| ChatMessage { role: Role::User, blocks: vec![Block::Text(text.into())] };
        let steered = ChatMessage {
            role: Role::User,
            blocks: vec![Block::ToolResult { call_id: "c_next".into(), content: "r".into(), is_error: false }, Block::Text("also check b".into())],
        };
        let messages = vec![prompt("one"), thinking("old"), result("old"), ChatMessage { role: Role::Assistant, blocks: vec![Block::Text("done".into())] }, prompt("two"), thinking("new"), steered, thinking("next"), result("next")];
        let built = body(&Request { messages, ..request() });
        let kept: Vec<&str> = built["messages"].as_array().unwrap().iter().filter_map(|m| m["reasoning_content"].as_str()).collect();
        assert_eq!(kept, ["old", "new", "next"], "which turns' reasoning to send is the session's choice; the wire keeps what it is given");
    }

    #[test]
    fn a_tool_loop_caches_past_the_prompt_that_started_it() {
        let call = |id: &str| ChatMessage { role: Role::Assistant, blocks: vec![Block::ToolUse { id: id.into(), name: "read".into(), input: json!({}) }] };
        let result = |id: &str| ChatMessage {
            role: Role::User,
            blocks: vec![Block::ToolResult { call_id: format!("{id}a"), content: format!("{id} first"), is_error: false }, Block::ToolResult { call_id: format!("{id}b"), content: format!("{id} second"), is_error: false }],
        };
        let prompt = ChatMessage { role: Role::User, blocks: vec![Block::Text("go".into())] };
        let mut body = body(&Request { model: "anthropic/claude-sonnet-4.5".into(), messages: vec![prompt, call("x"), result("x"), call("y"), result("y")], ..request() });
        mark_breakpoints(&mut body);
        assert_eq!(marked(&body), ["system:sys", "tool:x second", "tool:y second"], "the last result of each of the last two turns");
    }

    /// A local OpenRouter: records the body it got and replies with cache-hit usage.
    #[tokio::test]
    async fn an_openrouter_exchange_sends_breakpoints_for_claude_only_and_reads_cache_usage() {
        use axum::extract::State;
        let _ = rustls::crypto::ring::default_provider().install_default();
        let seen: std::sync::Arc<std::sync::Mutex<Vec<Value>>> = Default::default();
        let handler = |State(seen): State<std::sync::Arc<std::sync::Mutex<Vec<Value>>>>, axum::Json(body): axum::Json<Value>| async move {
            seen.lock().unwrap().push(body);
            let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":900,\"completion_tokens\":2,\"prompt_tokens_details\":{\"cached_tokens\":800,\"cache_write_tokens\":0}}}\n\ndata: [DONE]\n\n";
            ([("content-type", "text/event-stream")], sse)
        };
        let app = axum::Router::new().route("/chat/completions", axum::routing::post(handler)).with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let gateway = Compat::openrouter(&url);
        let key = Credential::ApiKey { key: "or-key".into() };
        let chunks: Vec<Chunk> = gateway.stream(&conversation("anthropic/claude-sonnet-4.5"), &key).await.unwrap().map(Result::unwrap).collect().await;
        assert!(chunks.contains(&Chunk::Usage(Usage { input: 100, output: 2, cache_read: 800, cache_write: 0 })));
        gateway.stream(&conversation("openai/gpt-6"), &key).await.unwrap().map(Result::unwrap).collect::<Vec<_>>().await;
        let seen = seen.lock().unwrap();
        assert_eq!(marked(&seen[0]), ["system:sys", "user:two", "user:three"]);
        assert!(marked(&seen[1]).is_empty(), "other vendors' models are sent as they were");
    }

    #[test]
    fn errors_and_length_stops_classify() {
        assert!(matches!(StreamState::default().chunks(r#"{"error":{"message":"nope","code":"bad"}}"#), Err(Error::Api { retryable: false, .. })));
        let gateway = StreamState::default().chunks(r#"{"error":{"message":"Provider returned error","code":502},"choices":[{"finish_reason":"error"}]}"#);
        assert!(matches!(gateway, Err(Error::Api { status: 502, retryable: true, .. })), "a gateway's streamed 502 retries: {gateway:?}");
        let overloaded = StreamState::default().chunks(r#"{"error":{"message":"busy","type":"overloaded_error"}}"#);
        assert!(matches!(overloaded, Err(Error::Api { retryable: true, .. })), "an upstream overload passed through retries");
        assert!(matches!(StreamState::default().chunks(r#"{"error":{"message":"key","code":401}}"#), Err(Error::Unauthenticated(ref m)) if m == "key"));
        let mut state = StreamState::default();
        state.chunks(r#"{"choices":[{"delta":{"content":"x"},"finish_reason":"length"}]}"#).unwrap();
        assert!(state.done().unwrap().contains(&Chunk::Stop(StopReason::MaxTokens)));
        assert!(matches!(api_error(429, "{}"), Error::Api { retryable: true, .. }));
    }
}
