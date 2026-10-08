//! Gemini generateContent over SSE, for the Gemini API and, through `stream_from`, Vertex.

use std::collections::HashMap;

use futures_util::StreamExt;
use serde_json::{Value, json};

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
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: super::http::client(),
            timeouts: super::http::Timeouts::default(),
        }
    }

    pub async fn stream(&self, request: &Request, credential: &Credential) -> Result<ChunkStream, Error> {
        let url = format!(
            "{}/models/{}:streamGenerateContent?alt=sse",
            self.base_url, request.model
        );

        let http = match credential {
            Credential::ApiKey { key } => self.client.post(url).header("x-goog-api-key", key),
            Credential::OAuth { access, .. } => self.client.post(url).bearer_auth(access),
            Credential::Ambient { .. } => return Err(Error::Unauthenticated(String::new())),
        };

        stream_from(http, request, &self.timeouts).await
    }
}

/// Sends a generateContent request already addressed and authorised (the Gemini API or Vertex) and reads its events.
pub(super) async fn stream_from(
    http: reqwest::RequestBuilder,
    request: &Request,
    timeouts: &super::http::Timeouts,
) -> Result<ChunkStream, Error> {
    let response = super::http::send(
        http.header("accept", "text/event-stream").json(&body(request)),
        timeouts,
    )
    .await?;
    let status = response.status();
    if !status.is_success() {
        let headers = response.headers().clone();
        let text = super::http::bounded_body(response, timeouts).await;

        return Err(api_error(status.as_u16(), &text).with_headers(&headers));
    }

    let mut state = StreamState::default();
    let events = sse::events(response.bytes_stream(), timeouts.idle);
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

fn body(request: &Request) -> Value {
    let mut names: HashMap<String, String> = HashMap::new();
    let mut body = json!({
        "contents": request.messages.iter().map(|message| content(message, &mut names)).collect::<Vec<_>>(),
        "generationConfig": { "maxOutputTokens": request.max_tokens },
    });

    if !request.system.is_empty() {
        body["systemInstruction"] = json!({ "parts": [{ "text": request.system }] });
    }
    if !request.tools.is_empty() {
        let declarations: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parametersJsonSchema": tool.input_schema
                })
            })
            .collect();
        body["tools"] = json!([{ "functionDeclarations": declarations }]);
        if request.no_tool_calls {
            body["toolConfig"] = json!({ "functionCallingConfig": { "mode": "NONE" } });
        }
    }

    let config = &mut body["generationConfig"];
    match &request.reasoning {
        Some(Reasoning::Budget { tokens }) => {
            config["thinkingConfig"] = json!({ "thinkingBudget": tokens, "includeThoughts": true });
        }
        Some(Reasoning::Effort { level }) => {
            config["thinkingConfig"] = json!({ "thinkingLevel": level, "includeThoughts": true });
        }
        None if request.show_thinking => config["thinkingConfig"] = json!({ "includeThoughts": true }),
        None => {}
    }

    let sampling = [
        ("temperature", request.temperature.map(Value::from)),
        ("topP", request.top_p.map(Value::from)),
        ("topK", request.top_k.map(Value::from)),
    ];
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
                if let Some(signature) = signature {
                    part["thoughtSignature"] = json!(signature);
                }
                parts.push(part);
            }
            Block::Pdf { base64 } => {
                parts.push(json!({ "inlineData": { "mimeType": "application/pdf", "data": base64 } }));
            }
            Block::Stored { .. } => {}
            Block::ToolUse { id, name, input } => {
                names.insert(id.clone(), name.clone());
                parts.push(json!({ "functionCall": { "id": id, "name": name, "args": input } }));
            }
            Block::ToolResult {
                call_id,
                content,
                is_error,
            } => {
                let name = names.get(call_id).cloned().unwrap_or_default();
                let key = if *is_error { "error" } else { "output" };
                let response = json!({ "id": call_id, "name": name, "response": { key: content } });
                parts.push(json!({ "functionResponse": response }));
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
                "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY" => {
                    StopReason::Refused
                }
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
            let id = call["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map_or_else(|| crate::id::new("call"), str::to_string);
            out.push(Chunk::ToolUseStart {
                id,
                name: call["name"].as_str().unwrap_or_default().into(),
            });
            out.push(Chunk::ToolInputDelta(
                call.get("args").cloned().unwrap_or_else(|| json!({})).to_string(),
            ));
            if let Some(signature) = signature {
                out.push(Chunk::PartSignature(signature.into()));
            }
            out.push(Chunk::BlockStop);
            return out;
        }

        let text = match (part["text"].as_str(), signature) {
            (Some(text), _) => text,
            (None, Some(_)) => "",
            _ => return out,
        };

        let kind = if part["thought"].as_bool().unwrap_or(false) {
            Open::Thought
        } else {
            Open::Text
        };
        if self.open != Some(kind) || signature.is_some() {
            out.extend(self.close());
            out.push(if kind == Open::Thought {
                Chunk::ReasoningStart
            } else {
                Chunk::TextStart
            });
            self.open = Some(kind);
        }
        out.push(if self.open == Some(Open::Thought) {
            Chunk::ReasoningDelta(text.into())
        } else {
            Chunk::TextDelta(text.into())
        });
        if let Some(signature) = signature {
            out.push(Chunk::PartSignature(signature.into()));
            out.extend(self.close());
        }

        out
    }

    /// Closes only the open block; signatures have already been attached to their own parts.
    fn close(&mut self) -> Vec<Chunk> {
        let mut out = Vec::new();
        if self.open.take().is_some() {
            out.push(Chunk::BlockStop);
        }

        out
    }
}

#[cfg(test)]
mod tests;
