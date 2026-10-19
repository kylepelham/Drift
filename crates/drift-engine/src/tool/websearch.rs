use serde_json::{Value, json};

use super::{Ask, Context, Output, RunFuture, Tool, ToolError, ToolMetadata};
use crate::llm::ToolSpec;
use crate::websearch::{Hit, MAX_MAPPED, MAX_RESULTS, Query};

const DEFAULT_RESULTS: u32 = 8;
/// Results when each carries its whole page: fewer, as each is long.
const DEFAULT_PAGES: u32 = 3;
/// Pages read at once for a site's pages with their content, the most a bounded crawl reads.
const MAX_SITE_PAGES: u32 = 10;
const DEFAULT_MAPPED: u32 = 30;

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
                    "query": { "type": "string", "description": "What to search for, as you would type it into a search engine. With `site`, orders its pages by relevance and may be left out." },
                    "site": { "type": "string", "description": "A site's address, such as https://docs.rs/tokio: lists that site's pages instead of searching the web." },
                    "content": { "type": "boolean", "description": "Return each result's page as markdown, not only a snippet. Slower and costs more; default false." },
                    "source": { "type": "string", "enum": ["web", "developer"], "description": "developer searches GitHub issues, pull requests, READMEs and documentation instead of the web. Default web." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_MAPPED, "description": "How many results. Default 8, 3 with content, 30 pages of a site." }
                }
            }),
        }
    }

    fn ask(&self, _ctx: &Context, input: &Value) -> Option<Ask> {
        let (subject, title) = match (text(input, "site"), text(input, "query")) {
            (Some(site), _) => (site, format!("List the pages of {site}")),
            (None, Some(query)) => (query, format!("Search the web for {query}")),
            (None, None) => return None,
        };

        // As webfetch: searching runs without asking; a rule can still ask or deny by query or site.
        Some(Ask::new("websearch", subject, title).allow_by_default())
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let content = input["content"].as_bool().unwrap_or(false);
            let limit = |default: u32| input["limit"].as_u64().map_or(default, |limit| limit as u32);
            let (title, (provider, hits)) = match (text(&input, "site"), text(&input, "query")) {
                (Some(site), query) => {
                    let pages = if content { DEFAULT_PAGES } else { DEFAULT_MAPPED };
                    (
                        site.to_string(),
                        site_pages(ctx, site, query, content, limit(pages)).await?,
                    )
                }
                (None, Some(query)) => {
                    let found = search(
                        ctx,
                        &input,
                        query,
                        content,
                        limit(if content { DEFAULT_PAGES } else { DEFAULT_RESULTS }),
                    )
                    .await?;
                    (query.to_string(), found)
                }
                (None, None) => return Err(ToolError("give a query, or a site to list the pages of".into())),
            };

            Ok(Output {
                title,
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

/// A field given as non-empty text.
fn text<'a>(input: &'a Value, field: &str) -> Option<&'a str> {
    input[field].as_str().map(str::trim).filter(|value| !value.is_empty())
}

async fn search(
    ctx: &Context,
    input: &Value,
    query: &str,
    content: bool,
    limit: u32,
) -> Result<(String, Vec<Hit>), ToolError> {
    let query = Query {
        text: query,
        limit: limit.min(MAX_RESULTS),
        content,
        developer: input["source"].as_str() == Some("developer"),
    };

    ctx.engine.web_search(&ctx.session_id, query).await.map_err(ToolError)
}

/// A site's pages, each read in full too when `content` asks for it: a crawl kept to `limit` pages.
async fn site_pages(
    ctx: &Context,
    site: &str,
    query: Option<&str>,
    content: bool,
    limit: u32,
) -> Result<(String, Vec<Hit>), ToolError> {
    if !site.starts_with("http://") && !site.starts_with("https://") {
        return Err(ToolError("site must start with http:// or https://".into()));
    }
    let limit = if content { limit.min(MAX_SITE_PAGES) } else { limit };
    let (provider, mut hits) = ctx
        .engine
        .web_map(&ctx.session_id, site, query, limit)
        .await
        .map_err(ToolError)?;
    if !content {
        return Ok((provider, hits));
    }

    let read = hits.iter().map(|hit| ctx.engine.web_scrape(&ctx.session_id, &hit.url));
    let pages = tokio::select! {
        pages = futures_util::future::join_all(read) => pages,
        () = ctx.abort.cancelled() => return Err(ToolError("aborted".into())),
    };
    for (hit, page) in hits.iter_mut().zip(pages) {
        hit.content = Some(match page {
            Ok((_, page)) => crate::websearch::capped(&page.text, crate::websearch::PAGE_CHARS),
            Err(why) => format!("(could not read this page: {why})"),
        });
    }

    Ok((provider, hits))
}

fn listed(hits: &[Hit]) -> String {
    if hits.is_empty() {
        return "No results. Try other words.".into();
    }

    hits.iter()
        .enumerate()
        .map(|(index, hit)| {
            let mut entry = format!("{}. {}\n{}", index + 1, hit.title, hit.url);
            if !hit.snippet.is_empty() {
                entry.push('\n');
                entry.push_str(&hit.snippet);
            }
            if let Some(page) = &hit.content {
                entry.push_str("\n\n");
                entry.push_str(page);
            }
            entry
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}
