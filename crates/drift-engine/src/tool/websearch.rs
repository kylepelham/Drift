use serde_json::{Value, json};

use super::{Ask, Context, Output, RunFuture, Tool, ToolError, ToolMetadata, required_str};
use crate::llm::ToolSpec;
use crate::websearch::{Hit, MAX_RESULTS};

const DEFAULT_RESULTS: u32 = 8;

pub struct WebSearch;

impl Tool for WebSearch {
    fn permissions(&self) -> &'static [&'static str] {
        &["websearch"]
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "websearch".into(),
            description: include_str!("prompts/websearch.txt").trim().into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What to search for, as you would type it into a search engine." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_RESULTS, "description": "How many results. Default 8." }
                },
                "required": ["query"]
            }),
        }
    }

    fn ask(&self, _ctx: &Context, input: &Value) -> Option<Ask> {
        let query = input["query"].as_str()?;
        // As webfetch: searching runs without asking; a rule can still ask or deny by query.
        Some(Ask::new("websearch", query, format!("Search the web for {query}")).allow_by_default())
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let query = required_str(&input, "query")?.trim();
            if query.is_empty() {
                return Err(ToolError("query is empty".into()));
            }
            let limit = input["limit"]
                .as_u64()
                .map_or(DEFAULT_RESULTS, |limit| limit.min(MAX_RESULTS.into()) as u32);
            let (provider, hits) = ctx
                .engine
                .web_search(&ctx.session_id, query, limit)
                .await
                .map_err(ToolError)?;

            Ok(Output {
                title: query.to_string(),
                output: listed(&hits),
                metadata: ToolMetadata {
                    provider: Some(provider),
                    count: Some(hits.len()),
                    ..Default::default()
                },
            })
        })
    }
}

fn listed(hits: &[Hit]) -> String {
    if hits.is_empty() {
        return "No results. Try other words.".into();
    }

    hits.iter()
        .enumerate()
        .map(|(index, hit)| format!("{}. {}\n{}\n{}", index + 1, hit.title, hit.url, hit.snippet))
        .collect::<Vec<_>>()
        .join("\n\n")
}
