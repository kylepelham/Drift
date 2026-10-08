use crate::llm::{Chunk, Error, STREAMED, StopReason};
use crate::session::types::Usage;
use serde_json::Value;
use std::collections::HashSet;

use super::api_error;

/// Which streamed items are open, so item-level events map to the right block kind.
#[derive(Default)]
pub(super) struct StreamState {
    reasoning: HashSet<String>,
    messages: HashSet<String>,
    calls_with_deltas: HashSet<String>,
    called_tools: bool,
}

impl StreamState {
    pub(super) fn chunks(&mut self, data: &str) -> Result<Vec<Chunk>, Error> {
        let value: Value = serde_json::from_str(data).map_err(|error| Error::Malformed(error.to_string()))?;
        let kind = value["type"].as_str().unwrap_or_default();
        let item_id = value["item_id"].as_str().unwrap_or_default();
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_string();

        Ok(match kind {
            "response.output_item.added" => self.item_added(&value["item"]),
            "response.output_text.delta" => vec![Chunk::TextDelta(text("delta"))],
            "response.reasoning_summary_part.added" if value["summary_index"].as_u64().unwrap_or(0) > 0 => {
                vec![Chunk::ReasoningDelta("\n\n".into())]
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                vec![Chunk::ReasoningDelta(text("delta"))]
            }
            "response.function_call_arguments.delta" => {
                self.calls_with_deltas.insert(item_id.into());
                vec![Chunk::ToolInputDelta(text("delta"))]
            }
            "response.output_item.done" => self.item_done(&value["item"]),
            "response.completed" | "response.incomplete" => {
                self.finished(&value["response"], kind == "response.incomplete")
            }
            "response.failed" => return Err(api_error(STREAMED, &value["response"].to_string())),
            "error" => return Err(api_error(STREAMED, data)),
            _ => Vec::new(),
        })
    }

    fn item_added(&mut self, item: &Value) -> Vec<Chunk> {
        let id = item["id"].as_str().unwrap_or_default().to_string();

        match item["type"].as_str().unwrap_or_default() {
            "message" => {
                self.messages.insert(id);
                vec![Chunk::TextStart]
            }
            "reasoning" => {
                self.reasoning.insert(id);
                vec![Chunk::ReasoningStart]
            }
            "function_call" => {
                self.called_tools = true;
                vec![Chunk::ToolUseStart {
                    id: item["call_id"].as_str().unwrap_or_default().into(),
                    name: item["name"].as_str().unwrap_or_default().into(),
                }]
            }
            _ => Vec::new(),
        }
    }

    fn item_done(&mut self, item: &Value) -> Vec<Chunk> {
        let id = item["id"].as_str().unwrap_or_default();

        match item["type"].as_str().unwrap_or_default() {
            "message" => {
                self.messages.remove(id);
                vec![Chunk::BlockStop]
            }
            "reasoning" => {
                self.reasoning.remove(id);
                let mut chunks = Vec::new();
                if let Some(encrypted) = item["encrypted_content"].as_str() {
                    chunks.push(Chunk::ReasoningSignature(encrypted.into()));
                }
                chunks.push(Chunk::BlockStop);

                chunks
            }
            "function_call" => {
                let mut chunks = Vec::new();
                if !self.calls_with_deltas.remove(id)
                    && let Some(arguments) = item["arguments"].as_str().filter(|arguments| !arguments.is_empty())
                {
                    chunks.push(Chunk::ToolInputDelta(arguments.into()));
                }
                chunks.push(Chunk::BlockStop);

                chunks
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn finished(&self, response: &Value, incomplete: bool) -> Vec<Chunk> {
        let usage = &response["usage"];
        let count = |path: &[&str]| path.iter().fold(usage, |value, key| &value[*key]).as_u64().unwrap_or(0);
        let cache_read = count(&["input_tokens_details", "cached_tokens"]);
        let reason = &response["incomplete_details"]["reason"];
        let stop = if incomplete && reason == "max_output_tokens" {
            StopReason::MaxTokens
        } else if incomplete && reason == "content_filter" {
            StopReason::Refused
        } else if self.called_tools {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        };

        vec![
            Chunk::Usage(Usage {
                input: count(&["input_tokens"]).saturating_sub(cache_read),
                output: count(&["output_tokens"]),
                cache_read,
                cache_write: 0,
            }),
            Chunk::Stop(stop),
        ]
    }
}
