//! Gemini generateContent over SSE, for the Gemini API and, through `stream_from`, Vertex.

use std::collections::HashMap;

use futures_util::StreamExt;
use serde_json::{json, Value};

use super::catalog::Reasoning;
use super::sse;
use super::{Block, ChatMessage, Chunk, ChunkStream, Credential, Error, Request, Role, StopReason};
use crate::session::types::Usage;

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

#[derive(Clone, Debug)]
pub struct Gemini {
    base_url: String,
    client: reqwest::Client,
    pub timeouts: super::http::Timeouts,
}

impl Default for Gemini {
    fn default() -> Self {
        Self::new(DEFAULT_BASE_URL)
    }
}

impl Gemini {
    pub fn new(base_url: &str) -> Self {
        Self { base_url: base_url.trim_end_matches('/').to_string(), client: super::http::client(), timeouts: super::http::Timeouts::default() }
    }

    pub async fn stream(&self, request: &Request, credential: &Credential) -> Result<ChunkStream, Error> {
        let url = format!("{}/models/{}:streamGenerateContent?alt=sse", self.base_url, request.model);
        let http = match credential {
            Credential::ApiKey { key } => self.client.post(url).header("x-goog-api-key", key),
            Credential::OAuth { access, .. } => self.client.post(url).bearer_auth(access),
            Credential::Ambient { .. } => return Err(Error::Unauthenticated(String::new())),
        };
        stream_from(http, request, &self.timeouts).await
    }
}

/// Sends a generateContent request already addressed and authorised (the Gemini API or Vertex) and reads its events.
pub(super) async fn stream_from(http: reqwest::RequestBuilder, request: &Request, timeouts: &super::http::Timeouts) -> Result<ChunkStream, Error> {
    let response = super::http::send(http.header("accept", "text/event-stream").json(&body(request)), timeouts).await?;
    let status = response.status();
    if !status.is_success() {
        let headers = response.headers().clone();
        return Err(api_error(status.as_u16(), &super::http::bounded_body(response, timeouts).await).with_headers(&headers));
    }
    let mut state = StreamState::default();
    let events = sse::events(response.bytes_stream(), timeouts.idle);
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

fn body(request: &Request) -> Value {
    let mut names: HashMap<String, String> = HashMap::new();
    let mut body = json!({
        "contents": request.messages.iter().map(|m| content(m, &mut names)).collect::<Vec<_>>(),
        "generationConfig": { "maxOutputTokens": request.max_tokens },
    });
    if !request.system.is_empty() {
        body["systemInstruction"] = json!({ "parts": [{ "text": request.system }] });
    }
    if !request.tools.is_empty() {
        let declarations: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| json!({ "name": tool.name, "description": tool.description, "parametersJsonSchema": tool.input_schema }))
            .collect();
        body["tools"] = json!([{ "functionDeclarations": declarations }]);
        if request.no_tool_calls {
            body["toolConfig"] = json!({ "functionCallingConfig": { "mode": "NONE" } });
        }
    }
    let config = &mut body["generationConfig"];
    match &request.reasoning {
        Some(Reasoning::Budget { tokens }) => config["thinkingConfig"] = json!({ "thinkingBudget": tokens, "includeThoughts": true }),
        Some(Reasoning::Effort { level }) => config["thinkingConfig"] = json!({ "thinkingLevel": level, "includeThoughts": true }),
        None if request.show_thinking => config["thinkingConfig"] = json!({ "includeThoughts": true }),
        None => {}
    }
    let sampling = [("temperature", request.temperature.map(Value::from)), ("topP", request.top_p.map(Value::from)), ("topK", request.top_k.map(Value::from))];
    for (key, value) in sampling {
        if let Some(value) = value {
            config[key] = value;
        }
    }
    body
}

/// Tool results need the function's name, which only the earlier call carries; `names` remembers it.
fn content(message: &ChatMessage, names: &mut HashMap<String, String>) -> Value {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "model",
    };
    let mut parts: Vec<Value> = Vec::new();
    for block in &message.blocks {
        let first = parts.len();
        match block.unsigned() {
            Block::Signed { .. } => unreachable!("unsigned blocks cannot be signed"),
            Block::Text(text) => parts.push(json!({ "text": text })),
            Block::Image { mime, base64 } => parts.push(json!({ "inlineData": { "mimeType": mime, "data": base64 } })),
            Block::Reasoning { text, signature, .. } => {
                let mut part = json!({ "text": text, "thought": true });
                if let Some(signature) = signature { part["thoughtSignature"] = json!(signature); }
                parts.push(part);
            }
            Block::Pdf { base64 } => parts.push(json!({ "inlineData": { "mimeType": "application/pdf", "data": base64 } })),
            Block::Stored { .. } => {}
            Block::ToolUse { id, name, input } => {
                names.insert(id.clone(), name.clone());
                parts.push(json!({ "functionCall": { "id": id, "name": name, "args": input } }));
            }
            Block::ToolResult { call_id, content, is_error } => {
                let name = names.get(call_id).cloned().unwrap_or_default();
                let key = if *is_error { "error" } else { "output" };
                parts.push(json!({ "functionResponse": { "id": call_id, "name": name, "response": { key: content } } }));
            }
        }
        if let (Block::Signed { signature, .. }, Some(part)) = (block, parts.get_mut(first)) {
            part["thoughtSignature"] = json!(signature);
        }
    }
    json!({ "role": role, "parts": parts })
}

fn api_error(status: u16, text: &str) -> Error {
    let parsed: Value = serde_json::from_str(text).unwrap_or_default();
    let kind = parsed["error"]["status"].as_str().unwrap_or("api_error").to_string();
    let message = parsed["error"]["message"].as_str().unwrap_or(text).to_string();
    match status {
        401 | 403 => Error::Unauthenticated(message),
        _ => Error::api(status, kind, message),
    }
}

#[derive(Default)]
struct StreamState {
    open: Option<Open>,
    called_tools: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Open {
    Text,
    Thought,
}

impl StreamState {
    fn chunks(&mut self, data: &str) -> Result<Vec<Chunk>, Error> {
        let value: Value = serde_json::from_str(data).map_err(|e| Error::Malformed(e.to_string()))?;
        if value["error"].is_object() {
            return Err(api_error(super::STREAMED, data));
        }
        let mut out = Vec::new();
        let candidate = &value["candidates"][0];
        for part in candidate["content"]["parts"].as_array().into_iter().flatten() {
            out.extend(self.part(part));
        }
        if let Some(usage) = value.get("usageMetadata").filter(|u| u.is_object()) {
            let count = |key: &str| usage[key].as_u64().unwrap_or(0);
            let cache_read = count("cachedContentTokenCount");
            out.push(Chunk::Usage(Usage {
                input: count("promptTokenCount").saturating_sub(cache_read),
                output: count("candidatesTokenCount") + count("thoughtsTokenCount"),
                cache_read,
                cache_write: 0,
            }));
        }
        if let Some(reason) = candidate["finishReason"].as_str() {
            out.extend(self.close());
            out.push(Chunk::Stop(match reason {
                "MAX_TOKENS" => StopReason::MaxTokens,
                "STOP" if self.called_tools => StopReason::ToolUse,
                "STOP" => StopReason::EndTurn,
                "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY" => StopReason::Refused,
                _ => StopReason::Other,
            }));
        }
        Ok(out)
    }

    fn part(&mut self, part: &Value) -> Vec<Chunk> {
        let mut out = Vec::new();
        let signature = part["thoughtSignature"].as_str();
        if let Some(call) = part.get("functionCall") {
            out.extend(self.close());
            self.called_tools = true;
            let id = call["id"].as_str().filter(|id| !id.is_empty()).map_or_else(|| crate::id::new("call"), str::to_string);
            out.push(Chunk::ToolUseStart { id, name: call["name"].as_str().unwrap_or_default().into() });
            out.push(Chunk::ToolInputDelta(call.get("args").cloned().unwrap_or_else(|| json!({})).to_string()));
            if let Some(signature) = signature { out.push(Chunk::PartSignature(signature.into())); }
            out.push(Chunk::BlockStop);
            return out;
        }
        let text = match (part["text"].as_str(), signature) {
            (Some(text), _) => text,
            (None, Some(_)) => "",
            _ => return out,
        };
        let kind = if part["thought"].as_bool().unwrap_or(false) { Open::Thought } else { Open::Text };
        if self.open != Some(kind) || signature.is_some() {
            out.extend(self.close());
            out.push(if kind == Open::Thought { Chunk::ReasoningStart } else { Chunk::TextStart });
            self.open = Some(kind);
        }
        out.push(if self.open == Some(Open::Thought) { Chunk::ReasoningDelta(text.into()) } else { Chunk::TextDelta(text.into()) });
        if let Some(signature) = signature {
            out.push(Chunk::PartSignature(signature.into()));
            out.extend(self.close());
        }
        out
    }

    /// Closes only the open block; signatures have already been attached to their own parts.
    fn close(&mut self) -> Vec<Chunk> {
        let mut out = Vec::new();
        match self.open.take() {
            Some(Open::Thought) => out.push(Chunk::BlockStop),
            Some(Open::Text) => out.push(Chunk::BlockStop),
            None => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolSpec;

    #[test]
    fn a_pdf_is_inline_data() {
        let sent = content(&ChatMessage { role: Role::User, blocks: vec![Block::Pdf { base64: "JVBERi0=".into() }] }, &mut HashMap::new());
        assert_eq!(sent["parts"][0], json!({ "inlineData": { "mimeType": "application/pdf", "data": "JVBERi0=" } }));
    }

    #[test]
    fn thoughts_are_asked_for_at_the_default_level_and_sampling_is_sent() {
        assert!(body(&Request { reasoning: None, ..request() }).pointer("/generationConfig/thinkingConfig").is_none());
        let built = body(&Request { reasoning: None, show_thinking: true, temperature: Some(1.0), top_p: Some(0.95), top_k: Some(64), ..request() });
        let config = &built["generationConfig"];
        assert_eq!(config["thinkingConfig"], json!({ "includeThoughts": true }));
        assert_eq!((config["temperature"].as_f64(), config["topP"].as_f64(), config["topK"].as_u64()), (Some(1.0), Some(0.95), Some(64)));
    }

    #[test]
    fn a_text_only_request_keeps_its_tools_but_forbids_calls() {
        assert!(body(&request()).get("toolConfig").is_none());
        let built = body(&Request { no_tool_calls: true, ..request() });
        assert_eq!(built["toolConfig"], json!({ "functionCallingConfig": { "mode": "NONE" } }));
        assert!(built["tools"][0]["functionDeclarations"].as_array().is_some_and(|tools| !tools.is_empty()));
    }

    fn request() -> Request {
        Request {
            model: "gemini-2.5-pro".into(),
            system: "sys".into(),
            messages: vec![
                ChatMessage { role: Role::User, blocks: vec![Block::Text("hi".into())] },
                ChatMessage {
                    role: Role::Assistant,
                    blocks: vec![
                        Block::Reasoning { text: "hm".into(), signature: Some("sig".into()), redacted: None },
                        Block::ToolUse { id: "call_1".into(), name: "read".into(), input: json!({ "path": "a" }) },
                    ],
                },
                ChatMessage { role: Role::User, blocks: vec![Block::ToolResult { call_id: "call_1".into(), content: "1: x".into(), is_error: false }] },
            ],
            tools: vec![ToolSpec { name: "read".into(), description: "r".into(), input_schema: json!({ "type": "object", "additionalProperties": false, "properties": {} }) }],
            max_tokens: 500,
            reasoning: Some(Reasoning::Budget { tokens: 2048 }),
            temperature: None,
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
    fn a_level_is_sent_as_gemini_3_names_it() {
        let mut request = request();
        request.reasoning = Some(Reasoning::Effort { level: "minimal".into() });
        assert_eq!(body(&request)["generationConfig"]["thinkingConfig"], json!({ "thinkingLevel": "minimal", "includeThoughts": true }));
    }

    #[test]
    fn body_matches_generate_content() {
        let built = body(&request());
        assert_eq!(built["systemInstruction"]["parts"][0]["text"], "sys");
        assert_eq!(built["generationConfig"]["thinkingConfig"]["thinkingBudget"], 2048);
        let contents = built["contents"].as_array().unwrap();
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"][1]["functionCall"]["name"], "read");
        assert_eq!(contents[1]["parts"][0]["thoughtSignature"], "sig");
        assert_eq!(contents[2]["parts"][0]["functionResponse"]["name"], "read");
        assert_eq!(contents[2]["parts"][0]["functionResponse"]["response"]["output"], "1: x");
        let declaration = &built["tools"][0]["functionDeclarations"][0];
        assert_eq!(declaration["parametersJsonSchema"]["additionalProperties"], false);
    }

    #[test]
    fn raw_json_schema_keeps_numeric_enums_and_all_union_types() {
        let input = serde_json::json!({
            "type": "object",
            "properties": {
                "default": { "type": "string", "default": "x" },
                "examples": { "type": ["integer", "null"], "examples": [1] },
                "level": { "type": "integer", "enum": [1, 2] }
            },
            "required": ["default"]
        });
        let mut request = request();
        request.tools[0].input_schema = input.clone();
        let out = body(&request);
        assert_eq!(out["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"], input);
        assert!(crate::tool::schema::problems(&input, &json!({ "default":"x", "level":1, "examples":null })).is_empty());
        assert!(!crate::tool::schema::problems(&input, &json!({ "default":"x", "level":"1" })).is_empty());
        let union = json!({ "type": ["string", "number", "null"] });
        request.tools[0].input_schema = union.clone();
        assert_eq!(body(&request)["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"], union);
    }

    #[test]
    fn stream_signatures_stay_on_the_function_call() {
        let mut state = StreamState::default();
        let feed = |state: &mut StreamState, json: &str| state.chunks(json).unwrap();
        assert_eq!(feed(&mut state, r#"{"candidates":[{"content":{"parts":[{"text":"th","thought":true}]}}]}"#), vec![Chunk::ReasoningStart, Chunk::ReasoningDelta("th".into())]);
        let mut call = feed(&mut state, r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"read","args":{"path":"a"}},"thoughtSignature":"sig"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":3,"thoughtsTokenCount":4}}"#);
        let Chunk::ToolUseStart { id, .. } = &mut call[1] else { panic!("{call:?}") };
        assert!(id.starts_with("call_") && id.len() > "call_1".len(), "a missing id is an engine id, unique across streams: {id}");
        *id = "call_1".into();
        assert_eq!(
            call,
            vec![
                Chunk::BlockStop,
                Chunk::ToolUseStart { id: "call_1".into(), name: "read".into() },
                Chunk::ToolInputDelta(r#"{"path":"a"}"#.into()),
                Chunk::PartSignature("sig".into()),
                Chunk::BlockStop,
                Chunk::Usage(Usage { input: 10, output: 7, cache_read: 0, cache_write: 0 }),
                Chunk::Stop(StopReason::ToolUse),
            ]
        );
        let mut plain = StreamState::default();
        assert_eq!(feed(&mut plain, r#"{"candidates":[{"content":{"parts":[{"text":"Hi"}]}}]}"#), vec![Chunk::TextStart, Chunk::TextDelta("Hi".into())]);
        assert_eq!(feed(&mut plain, r#"{"candidates":[{"content":{"parts":[]},"finishReason":"MAX_TOKENS"}]}"#), vec![Chunk::BlockStop, Chunk::Stop(StopReason::MaxTokens)]);
        assert!(matches!(StreamState::default().chunks(r#"{"error":{"status":"UNAVAILABLE","message":"x"}}"#), Err(Error::Api { retryable: true, .. })));
        assert!(matches!(StreamState::default().chunks(r#"{"error":{"status":"INVALID_ARGUMENT","message":"x"}}"#), Err(Error::Api { retryable: false, .. })));
    }

    #[test]
    fn a_call_signature_without_thought_text_round_trips_on_its_own_part() {
        let chunks = StreamState::default().chunks(r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"read","args":{"path":"a"}},"thoughtSignature":"call-sig"}]},"finishReason":"STOP"}]}"#).unwrap();
        assert!(chunks.contains(&Chunk::PartSignature("call-sig".into())));
        assert!(!chunks.iter().any(|chunk| matches!(chunk, Chunk::ReasoningSignature(_) | Chunk::ReasoningStart)));
        let sent = content(&ChatMessage { role: Role::Assistant, blocks: vec![Block::Text("before".into()), Block::Signed {
            part: Box::new(Block::ToolUse { id: "call_1".into(), name: "read".into(), input: json!({ "path":"a" }) }), signature: "call-sig".into(),
        }] }, &mut HashMap::new());
        assert!(sent["parts"][0].get("thoughtSignature").is_none());
        assert_eq!(sent["parts"][1]["thoughtSignature"], "call-sig");
        assert_eq!(sent["parts"][1]["functionCall"]["name"], "read");
    }
}
