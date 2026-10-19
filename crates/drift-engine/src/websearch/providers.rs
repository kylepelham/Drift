//! The built-in search providers' wire shapes: each a POST with a key, answering a list of pages.

use serde_json::{Value, json};

use super::{Hit, SearchError, json_hits, post};

pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    /// The environment variable a key is also read from.
    pub env: &'static str,
    url: &'static str,
}

pub const BUILT_IN: &[Provider] = &[
    Provider {
        id: "firecrawl",
        name: "Firecrawl",
        env: "FIRECRAWL_API_KEY",
        url: "https://api.firecrawl.dev/v2/search",
    },
    Provider {
        id: "exa",
        name: "Exa",
        env: "EXA_API_KEY",
        url: "https://api.exa.ai/search",
    },
    Provider {
        id: "tavily",
        name: "Tavily",
        env: "TAVILY_API_KEY",
        url: "https://api.tavily.com/search",
    },
];

pub(super) async fn search(
    provider: &Provider,
    key: &str,
    query: &str,
    limit: u32,
    base: Option<&str>,
) -> Result<Vec<Hit>, SearchError> {
    let url = base.map_or_else(|| provider.url.to_string(), |base| format!("{base}/{}", provider.id));
    let client = crate::llm::http::client();
    let (request, body) = match provider.id {
        "exa" => (
            client.post(url).header("x-api-key", key),
            json!({ "query": query, "numResults": limit, "contents": { "highlights": true } }),
        ),
        "tavily" => (
            client.post(url).bearer_auth(key),
            json!({ "query": query, "max_results": limit }),
        ),
        _ => (
            client.post(url).bearer_auth(key),
            json!({ "query": query, "limit": limit }),
        ),
    };
    let answer = post(request, body).await?;

    Ok(hits(provider.id, &answer))
}

fn hits(provider: &str, answer: &Value) -> Vec<Hit> {
    match provider {
        "exa" => json_hits(answer["results"].as_array(), &["highlights", "text", "summary"]),
        "tavily" => json_hits(answer["results"].as_array(), &["content"]),
        _ => json_hits(answer["data"]["web"].as_array(), &["description", "markdown"]),
    }
}
