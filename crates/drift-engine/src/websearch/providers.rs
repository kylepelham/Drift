//! The built-in providers' wire shapes: search, read one page (scrape) and list a site's pages (map), each a POST
//! with a key. Not every provider does everything; `can` says which do.

use serde_json::{Value, json};

use super::{Hit, Page, SearchError, Want, json_hits, post};

pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    /// The environment variable a key is also read from.
    pub env: &'static str,
    /// The API root its endpoints hang off.
    root: &'static str,
    scrapes: bool,
    maps: bool,
    /// Searches an index of code: issues, pull requests, READMEs and documentation.
    developer: bool,
}

pub const BUILT_IN: &[Provider] = &[
    Provider {
        id: "firecrawl",
        name: "Firecrawl",
        env: "FIRECRAWL_API_KEY",
        root: "https://api.firecrawl.dev/v2",
        scrapes: true,
        maps: true,
        developer: true,
    },
    Provider {
        id: "exa",
        name: "Exa",
        env: "EXA_API_KEY",
        root: "https://api.exa.ai",
        scrapes: true,
        maps: false,
        developer: false,
    },
    Provider {
        id: "tavily",
        name: "Tavily",
        env: "TAVILY_API_KEY",
        root: "https://api.tavily.com",
        scrapes: true,
        maps: true,
        developer: false,
    },
];

/// Page parts Firecrawl leaves out of a page's text: site navigation, sidebars, footers and pop-ups.
const CHROME_SELECTORS: [&str; 7] = [
    "nav",
    "aside",
    "footer",
    "[role=\"navigation\"]",
    "[role=\"banner\"]",
    "[role=\"dialog\"]",
    "[aria-modal=\"true\"]",
];
/// The same parts as Exa classifies them; it applies this as best it can.
const CHROME_SECTIONS: [&str; 5] = ["navigation", "header", "banner", "sidebar", "footer"];

/// A site whose pages are listed: most relevant to `query` first, at most `limit` of them.
#[derive(Clone, Copy, Debug)]
pub struct Site<'a> {
    pub url: &'a str,
    pub query: Option<&'a str>,
    pub limit: u32,
}

/// One search as the tool asks for it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Query<'a> {
    pub text: &'a str,
    pub limit: u32,
    /// Each result's page as markdown besides its snippet.
    pub content: bool,
    /// The developer index instead of the web.
    pub developer: bool,
}

impl Provider {
    /// Whether it can answer `want`; plugins only search, plainly.
    pub(super) fn can(&self, want: Want) -> bool {
        match want {
            Want::Search { developer } => !developer || self.developer,
            Want::Scrape => self.scrapes,
            Want::Map => self.maps,
        }
    }

    /// Where `endpoint` answers: its own API, or the stand-in tests serve.
    fn url(&self, endpoint: &str, base: Option<&str>) -> String {
        match base {
            Some(base) => format!("{base}/{}/{endpoint}", self.id),
            None => format!("{}/{endpoint}", self.root),
        }
    }

    fn request(&self, endpoint: &str, key: &str, base: Option<&str>) -> reqwest::RequestBuilder {
        let request = crate::llm::http::client().post(self.url(endpoint, base));

        match self.id {
            "exa" => request.header("x-api-key", key),
            _ => request.bearer_auth(key),
        }
    }
}

pub(super) async fn search(
    provider: &Provider,
    key: &str,
    query: Query<'_>,
    base: Option<&str>,
) -> Result<Vec<Hit>, SearchError> {
    let body = match provider.id {
        "exa" if query.content => json!({
            "query": query.text,
            "numResults": query.limit,
            "contents": {
                "highlights": true,
                "text": { "maxCharacters": super::PAGE_CHARS, "excludeSections": CHROME_SECTIONS },
            },
        }),
        "exa" => json!({ "query": query.text, "numResults": query.limit, "contents": { "highlights": true } }),
        "tavily" if query.content => {
            json!({ "query": query.text, "max_results": query.limit, "include_raw_content": "markdown" })
        }
        "tavily" => json!({ "query": query.text, "max_results": query.limit }),
        _ => firecrawl_search(query),
    };
    let answer = post(provider.request("search", key, base), body).await?;

    Ok(hits(provider.id, &answer))
}

fn firecrawl_search(query: Query<'_>) -> Value {
    let mut body = json!({ "query": query.text, "limit": query.limit });
    if query.content {
        body["scrapeOptions"] =
            json!({ "formats": ["markdown"], "onlyMainContent": true, "excludeTags": CHROME_SELECTORS });
    }
    if query.developer {
        body["categories"] = json!([{ "type": "developer" }]);
    }

    body
}

fn hits(provider: &str, answer: &Value) -> Vec<Hit> {
    match provider {
        "exa" => json_hits(answer["results"].as_array(), &["highlights", "text", "summary"], "text"),
        "tavily" => json_hits(answer["results"].as_array(), &["content"], "raw_content"),
        _ => json_hits(
            answer["data"]["web"].as_array(),
            &["description", "markdown"],
            "markdown",
        ),
    }
}

/// One page, read by the provider: rendered when it builds itself with JavaScript, past what blocks plain fetches.
pub(super) async fn scrape(provider: &Provider, key: &str, url: &str, base: Option<&str>) -> Result<Page, SearchError> {
    let (endpoint, body) = match provider.id {
        "exa" => (
            "contents",
            json!({ "urls": [url], "text": { "excludeSections": CHROME_SECTIONS } }),
        ),
        "tavily" => ("extract", json!({ "urls": [url], "format": "markdown" })),
        _ => (
            "scrape",
            json!({ "url": url, "formats": ["markdown"], "onlyMainContent": true, "excludeTags": CHROME_SELECTORS }),
        ),
    };
    let answer = post(provider.request(endpoint, key, base), body).await?;

    page(provider.id, url, &answer)
}

/// The page out of each provider's answer; a provider that answered but could not read the page says why.
fn page(provider: &str, url: &str, answer: &Value) -> Result<Page, SearchError> {
    let (found, title, failure) = match provider {
        "exa" => (
            &answer["results"][0]["text"],
            &answer["results"][0]["title"],
            &answer["statuses"][0]["error"]["tag"],
        ),
        "tavily" => (
            &answer["results"][0]["raw_content"],
            &Value::Null,
            &answer["failed_results"][0]["error"],
        ),
        _ => (
            &answer["data"]["markdown"],
            &answer["data"]["metadata"]["title"],
            &answer["data"]["metadata"]["error"],
        ),
    };
    let text = found.as_str().map(str::trim).filter(|text| !text.is_empty());
    let Some(text) = text else {
        let why = failure.as_str().unwrap_or("it returned no text");
        return Err(SearchError::Failed(format!("could not read {url}: {why}")));
    };

    Ok(Page {
        url: url.to_string(),
        title: title.as_str().unwrap_or(url).to_string(),
        text: super::without_chrome(text).to_string(),
    })
}

/// The site's pages, most relevant to `query` first when there is one.
pub(super) async fn map(
    provider: &Provider,
    key: &str,
    site: Site<'_>,
    base: Option<&str>,
) -> Result<Vec<Hit>, SearchError> {
    let Site { url, query, limit } = site;
    let request = provider.request("map", key, base);
    if provider.id == "tavily" {
        let answer = post(request, json!({ "url": url, "limit": super::MAP_SCAN.max(limit) })).await?;
        let found = answer["results"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str);
        return Ok(ranked(found, query, limit));
    }

    let mut body = json!({ "url": url, "limit": limit });
    if let Some(query) = query {
        body["search"] = json!(query);
    }
    let answer = post(request, body).await?;

    Ok(json_hits(answer["links"].as_array(), &["description"], ""))
}

/// Tavily orders nothing, so its list is ranked here: URLs naming more of `query`'s words first.
fn ranked<'a>(urls: impl Iterator<Item = &'a str>, query: Option<&str>, limit: u32) -> Vec<Hit> {
    let words: Vec<String> = query
        .unwrap_or_default()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.len() > 2)
        .map(str::to_lowercase)
        .collect();
    let score = |url: &str| {
        let url = url.to_lowercase();
        words.iter().filter(|word| url.contains(word.as_str())).count()
    };
    let mut found: Vec<&str> = urls.collect();
    found.sort_by_key(|url| std::cmp::Reverse(score(url)));

    found
        .into_iter()
        .take(limit as usize)
        .map(|url| Hit {
            url: url.to_string(),
            title: url.to_string(),
            snippet: String::new(),
            content: None,
        })
        .collect()
}
