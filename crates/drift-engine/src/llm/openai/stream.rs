use crate::llm::limits::Limits;
use crate::llm::{Chunk, Error, STREAMED, StopReason};
use crate::session::types::Usage;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

use super::api_error;

/// The item kinds a reply is made of; others (hosted tools, compaction) carry nothing Drift shows.
const SHOWN: [&str; 3] = ["message", "reasoning", "function_call"];

/// Which streamed items are open and which had deltas, so item-level events map to the right block kind and an
/// item given whole only at its end is neither lost nor doubled.
#[derive(Default)]
pub(super) struct StreamState {
    /// Each item's key by every name an event gave it: its id and `#output_index`.
    aliases: HashMap<String, String>,
    /// Items opened by `output_item.added`.
    begun: HashSet<String>,
    /// Items whose content arrived in deltas.
    streamed: HashSet<String>,
    /// The item opened last, for a delta that names none.
    last: String,
    called_tools: bool,
}

impl StreamState {
    pub(super) fn chunks(&mut self, data: &str) -> Result<Vec<Chunk>, Error> {
        let value: Value = serde_json::from_str(data).map_err(|error| Error::Malformed(error.to_string()))?;
        self.chunks_value(&value)
    }

    pub(super) fn chunks_value(&mut self, value: &Value) -> Result<Vec<Chunk>, Error> {
        let kind = kind(value);

        Ok(match kind {
            "response.output_item.added" => self.item_added(value),
            "response.output_text.delta"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_text.delta"
            | "response.function_call_arguments.delta" => self.delta(kind, value),
            "response.reasoning_summary_part.added" if value["summary_index"].as_u64().unwrap_or(0) > 0 => {
                vec![Chunk::ReasoningDelta("\n\n".into())]
            }
            "response.output_item.done" => self.item_done(value),
            "response.completed" | "response.incomplete" => {
                self.finished(&value["response"], kind == "response.incomplete")
            }
            "response.failed" => return Err(api_error(STREAMED, &value["response"].to_string())),
            "codex.rate_limits" => Limits::from_codex_event(value).map(Chunk::Limits).into_iter().collect(),
            "error" => {
                let status = value["status"]
                    .as_u64()
                    .or_else(|| value["status_code"].as_u64())
                    .and_then(|status| u16::try_from(status).ok())
                    .unwrap_or(STREAMED);
                return Err(api_error(status, &value.to_string()).with_header_object(&value["headers"]));
            }
            _ => Vec::new(),
        })
    }

    /// One name for an item however an event refers to it: gateways leave out its id on some events and its index
    /// on others.
    fn key(&mut self, id: Option<&str>, event: &Value) -> String {
        let names: Vec<String> = [
            id.filter(|id| !id.is_empty()).map(str::to_string),
            event["output_index"].as_u64().map(|index| format!("#{index}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        let key = names
            .iter()
            .find_map(|name| self.aliases.get(name).cloned())
            .or_else(|| names.first().cloned())
            .unwrap_or_else(|| self.last.clone());

        for name in names {
            self.aliases.entry(name).or_insert_with(|| key.clone());
        }

        key
    }

    fn delta(&mut self, kind: &str, event: &Value) -> Vec<Chunk> {
        let key = self.key(event["item_id"].as_str(), event);
        self.streamed.insert(key);
        let delta = event["delta"].as_str().unwrap_or_default().to_string();

        vec![match kind {
            "response.output_text.delta" => Chunk::TextDelta(delta),
            "response.function_call_arguments.delta" => Chunk::ToolInputDelta(delta),
            _ => Chunk::ReasoningDelta(delta),
        }]
    }

    fn item_added(&mut self, event: &Value) -> Vec<Chunk> {
        let item = &event["item"];
        let key = self.key(item["id"].as_str(), event);
        let start = self.start(item);
        if !start.is_empty() {
            self.last.clone_from(&key);
            self.begun.insert(key);
        }

        start
    }

    /// The chunk that opens an item of a kind Drift shows.
    fn start(&mut self, item: &Value) -> Vec<Chunk> {
        match item["type"].as_str().unwrap_or_default() {
            "message" => vec![Chunk::TextStart],
            "reasoning" => vec![Chunk::ReasoningStart],
            "function_call" => {
                self.called_tools = true;
                let id = item["call_id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .map_or_else(|| crate::id::new("call"), str::to_string);
                vec![Chunk::ToolUseStart {
                    id,
                    name: item["name"].as_str().unwrap_or_default().into(),
                }]
            }
            _ => Vec::new(),
        }
    }

    /// Closes the item, opening it first and giving its whole content when its start or deltas never came.
    fn item_done(&mut self, event: &Value) -> Vec<Chunk> {
        let item = &event["item"];
        let kind = item["type"].as_str().unwrap_or_default();
        if kind == "tool_search_call" {
            return self.search_call(item);
        }

        let key = self.key(item["id"].as_str(), event);
        let begun = self.begun.remove(&key);
        let nameless = kind == "function_call" && item["name"].as_str().is_none_or(str::is_empty);
        if !SHOWN.contains(&kind) || (!begun && nameless) {
            return Vec::new();
        }

        let mut chunks = if begun { Vec::new() } else { self.start(item) };
        if !self.streamed.remove(&key) {
            chunks.extend(whole(item));
        }
        if let Some(encrypted) = item["encrypted_content"].as_str().filter(|_| kind == "reasoning") {
            chunks.push(Chunk::ReasoningSignature(encrypted.into()));
        }
        chunks.push(Chunk::BlockStop);

        chunks
    }

    /// A search Drift runs, given whole when done, as a call of the `tool_search` tool; a hosted one is the API's own.
    fn search_call(&mut self, item: &Value) -> Vec<Chunk> {
        if item["execution"] != "client" {
            return Vec::new();
        }

        self.called_tools = true;
        let id = item["call_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map_or_else(|| crate::id::new("call"), str::to_string);
        let arguments = match &item["arguments"] {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };

        vec![
            Chunk::ToolUseStart {
                id,
                name: crate::llm::TOOL_SEARCH.into(),
            },
            Chunk::ToolInputDelta(arguments),
            Chunk::BlockStop,
        ]
    }

    pub(super) fn finished(&self, response: &Value, incomplete: bool) -> Vec<Chunk> {
        let usage = &response["usage"];
        let count = |path: &[&str]| path.iter().fold(usage, |value, key| &value[*key]).as_u64().unwrap_or(0);
        let cache_read = count(&["input_tokens_details", "cached_tokens"]);
        // GPT-5.6 and later bill what they write to the cache, at more than plain input.
        let cache_write = count(&["input_tokens_details", "cache_write_tokens"]);
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
                input: count(&["input_tokens"]).saturating_sub(cache_read + cache_write),
                output: count(&["output_tokens"]),
                cache_read,
                cache_write,
            }),
            Chunk::Stop(stop),
        ]
    }
}

/// An item's whole content as its `output_item.done` gives it, for an item whose deltas never came.
fn whole(item: &Value) -> Option<Chunk> {
    let joined = |parts: &Value, kind: &str, separator: &str| {
        let texts: Vec<&str> = parts
            .as_array()
            .into_iter()
            .flatten()
            .filter(|part| part["type"] == kind)
            .filter_map(|part| part["text"].as_str())
            .collect();
        texts.join(separator)
    };
    let (text, chunk): (String, fn(String) -> Chunk) = match item["type"].as_str()? {
        "message" => (joined(&item["content"], "output_text", ""), Chunk::TextDelta),
        "reasoning" => (joined(&item["summary"], "summary_text", "\n\n"), Chunk::ReasoningDelta),
        _ => (item["arguments"].as_str()?.to_string(), Chunk::ToolInputDelta),
    };

    (!text.is_empty()).then(|| chunk(text))
}

/// The event's `type`. Gateways send untyped frames: keep-alives, ignored, and bare `{"error": ...}` envelopes,
/// which are errors.
fn kind(value: &Value) -> &str {
    match value["type"].as_str() {
        Some(kind) => kind,
        None if !value["error"].is_null() => "error",
        None => "",
    }
}
