//! `tool_search`: loads deferred MCP tools into the model's context by name or by a search of their definitions.
//! The result names them as `references`; the Anthropic wire sends those as `tool_reference` blocks.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::{Ask, Context, Output, RunFuture, Tool, ToolError, ToolMetadata};
use crate::llm::ToolSpec;
use crate::session::types::{MessageWithParts, Part};

pub const NAME: &str = "tool_search";
/// The setting that defers large MCP servers' tools on models that take it; on unless turned off.
pub const SETTINGS_KEY: &str = "toolSearch";
const DEFAULT_LIMIT: usize = 5;
const MAX_LIMIT: usize = 20;

impl crate::Engine {
    pub fn tool_search(&self) -> bool {
        self.store.setting(SETTINGS_KEY).ok().flatten().unwrap_or(true)
    }
}

/// The deferred tools a turn offers, with the server each comes from.
pub struct ToolSearch {
    tools: Vec<(ToolSpec, String)>,
}

impl ToolSearch {
    pub fn new(tools: Vec<(ToolSpec, String)>) -> Self {
        Self { tools }
    }

    /// The tools `select:a,b` names, matched exactly or ignoring case.
    fn selected(&self, names: &str) -> Vec<String> {
        names
            .split(',')
            .map(str::trim)
            .filter_map(|wanted| {
                self.tools.iter().find(|(spec, _)| spec.name == wanted).or_else(|| {
                    self.tools
                        .iter()
                        .find(|(spec, _)| spec.name.eq_ignore_ascii_case(wanted))
                })
            })
            .map(|(spec, _)| spec.name.clone())
            .collect()
    }

    /// The best `limit` matches for `query`, best first; a tool matching no word is left out.
    fn ranked(&self, query: &str, limit: usize) -> Vec<String> {
        let words = words(query);
        let mut scored: Vec<(usize, &str)> = self
            .tools
            .iter()
            .map(|(spec, server)| (score(&words, spec, server), spec.name.as_str()))
            .filter(|(score, _)| *score > 0)
            .collect();
        scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(right.1)));

        scored
            .into_iter()
            .take(limit)
            .map(|(_, name)| name.to_string())
            .collect()
    }
}

fn words(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| word.len() > 1)
        .map(str::to_lowercase)
        .collect()
}

/// A word in the tool's name counts most, then its server, then its parameters and description.
fn score(words: &[String], spec: &ToolSpec, server: &str) -> usize {
    let name = self::words(&spec.name);
    let server = self::words(server);
    let parameters = spec.parameters();
    let fields: Vec<String> = parameters["properties"]
        .as_object()
        .into_iter()
        .flat_map(|properties| properties.keys())
        .flat_map(|key| self::words(key))
        .collect();
    let description = self::words(&spec.description);

    words
        .iter()
        .map(|word| {
            let found = |list: &[String]| list.iter().any(|known| known.starts_with(word.as_str()));
            let weights = [
                (found(&name), 4),
                (found(&server), 3),
                (found(&fields), 2),
                (found(&description), 1),
            ];
            weights
                .iter()
                .filter(|(hit, _)| *hit)
                .map(|(_, weight)| weight)
                .sum::<usize>()
        })
        .sum()
}

impl Tool for ToolSearch {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: NAME.into(),
            description: include_str!("prompts/tool_search.txt").trim().into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Words describing the tools you need, or `select:name,name` to load tools by name."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "How many tools a search loads at most. Default 5, max 20.",
                        "minimum": 1,
                        "maximum": MAX_LIMIT
                    }
                },
                "required": ["query"]
            }),
        }
    }

    fn ask(&self, _ctx: &Context, _input: &Value) -> Option<Ask> {
        None
    }

    fn run<'a>(&'a self, _ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let query = super::required_str(&input, "query")?.trim();
            let limit = input["limit"].as_u64().map_or(DEFAULT_LIMIT, |limit| {
                usize::try_from(limit).unwrap_or(MAX_LIMIT).clamp(1, MAX_LIMIT)
            });
            let found = match query.strip_prefix("select:") {
                Some(names) => self.selected(names),
                None => self.ranked(query, limit),
            };
            if found.is_empty() {
                return Err(ToolError(format!(
                    "No deferred tool matched `{query}`. Try other words, or `select:` with names from the system prompt's list."
                )));
            }

            Ok(Output {
                title: format!("Loaded {}", found.join(", ")),
                output: format!("Loaded {}. Call them as usual.", found.join(", ")),
                metadata: ToolMetadata {
                    references: Some(found),
                    ..Default::default()
                },
            })
        })
    }
}

/// The tools each `tool_search` call in `transcript` loaded, by call id.
pub fn references(transcript: &[MessageWithParts]) -> BTreeMap<String, Vec<String>> {
    transcript
        .iter()
        .flat_map(|message| message.parts.iter())
        .filter_map(|row| match &row.part {
            Part::ToolCall {
                call_id,
                name,
                metadata: Some(metadata),
                ..
            } if name == NAME => metadata.references.clone().map(|names| (call_id.clone(), names)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, description: &str, field: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: description.into(),
            input_schema: json!({ "type": "object", "properties": { field: { "type": "string" } } }),
        }
    }

    fn search() -> ToolSearch {
        ToolSearch::new(vec![
            (
                spec("apophis_get_fogbugz_case", "Read a case", "case_number"),
                "apophis".into(),
            ),
            (
                spec("apophis_search_emails", "Search my mailbox", "query"),
                "apophis".into(),
            ),
            (
                spec("sqlcompare_compare", "Compares two schemas", "project"),
                "sqlcompare".into(),
            ),
        ])
    }

    #[test]
    fn a_search_ranks_name_matches_before_description_matches() {
        assert_eq!(search().ranked("fogbugz case", 5), ["apophis_get_fogbugz_case"]);
        assert_eq!(
            search().ranked("search schemas", 5),
            ["apophis_search_emails", "sqlcompare_compare"]
        );
        assert_eq!(search().ranked("apophis", 1), ["apophis_get_fogbugz_case"]);
        assert!(search().ranked("weather", 5).is_empty());
    }

    #[test]
    fn select_loads_the_named_tools_and_skips_unknown_ones() {
        assert_eq!(
            search().selected("sqlcompare_compare, Apophis_Search_Emails,nothing"),
            ["sqlcompare_compare", "apophis_search_emails"]
        );
    }
}
