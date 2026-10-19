//! Web search for the `websearch` tool, and the pages it and `webfetch` read through a provider. Every provider,
//! built in or a plugin's, answers the same way, so routing never knows which kind it called. Built-in providers
//! take a list of keys: a key refused (401 to 403, or out of credit) is set aside and the next tried, and a key
//! rate limited (429) cools for the wait it names. In `auto`, a provider with no key left to try hands over to the
//! next in the user's order that can do what is asked. Cooldowns are shared by every session; a session keeps the
//! provider that last answered it.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::Engine;

mod providers;

pub use providers::{BUILT_IN, Query, Site};

pub const SETTINGS_KEY: &str = "webSearch";
/// A rate limit that names no wait cools its key this long.
const DEFAULT_COOLDOWN: Duration = Duration::from_secs(60);
/// The most results one search asks for.
pub const MAX_RESULTS: u32 = 20;
/// The most pages one map lists.
pub const MAX_MAPPED: u32 = 100;
/// The most of one page a result carries, so several pages fit in one tool result.
pub const PAGE_CHARS: usize = 12_000;
/// How many pages Tavily is asked to find before they are ranked and cut to the limit.
const MAP_SCAN: u32 = 100;

/// One result: where it is, what it is called, the text that matched, and its page when that was asked for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    pub url: String,
    pub title: String,
    pub snippet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// One page as a provider read it, as markdown.
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub url: String,
    pub title: String,
    pub text: String,
}

/// What a provider is asked to do; only some can do each.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Want {
    Search { developer: bool },
    Scrape,
    Map,
}

/// One request routed to a provider.
enum Job<'a> {
    Search(Query<'a>),
    Scrape(&'a str),
    Map(Site<'a>),
}

impl Job<'_> {
    fn want(&self) -> Want {
        match self {
            Self::Search(query) => Want::Search {
                developer: query.developer,
            },
            Self::Scrape(_) => Want::Scrape,
            Self::Map(_) => Want::Map,
        }
    }
}

/// What a provider answered: results, or one page.
enum Answer {
    Hits(Vec<Hit>),
    Page(Page),
}

impl Want {
    fn what(self) -> &'static str {
        match self {
            Self::Search { developer: false } => "search the web",
            Self::Search { developer: true } => "search the developer index",
            Self::Scrape => "read pages",
            Self::Map => "list a site's pages",
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum SearchError {
    /// Busy for now; try again after the wait, when it says.
    RateLimited(Option<Duration>),
    /// The key is not accepted (wrong, or out of credit).
    Refused(String),
    Failed(String),
}

/// Settings > Web search, kept by the engine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct WebSearch {
    /// A provider's id, `auto` (the first that can answer, in `order`) or `off`, which removes the tool.
    pub provider: String,
    /// The providers `auto` tries, in turn; ones left out follow in their own order.
    pub order: Vec<String>,
}

impl Default for WebSearch {
    fn default() -> Self {
        Self {
            provider: "auto".into(),
            order: Vec::new(),
        }
    }
}

/// A provider as Settings lists it.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchProvider {
    pub id: String,
    pub name: String,
    /// Built in, so it takes keys; a plugin's brings its own.
    pub built_in: bool,
    /// Keys saved for it, shown by their last four characters.
    pub keys: Vec<String>,
    /// Whether a key is found in the environment instead.
    pub from_environment: bool,
}

/// What routing remembers between searches.
#[derive(Default)]
pub(crate) struct Routing {
    cooling: Mutex<HashMap<String, Instant>>,
    refused: Mutex<HashSet<String>>,
    sticky: Mutex<HashMap<String, String>>,
    /// Where tests serve the built-in providers instead.
    #[cfg(test)]
    pub(crate) base: Mutex<Option<String>>,
}

/// One way to ask a provider: a built-in's key, or a plugin.
enum Attempt {
    Key {
        provider: &'static providers::Provider,
        key: String,
    },
    Plugin {
        provider: String,
    },
}

impl Attempt {
    /// What cooldowns and set-aside keys are recorded under.
    fn slot(&self) -> String {
        match self {
            Self::Key { provider, key } => format!("{}:{}", provider.id, crate::hex_bytes(&sha2_digest(key))),
            Self::Plugin { provider } => provider.clone(),
        }
    }
}

fn sha2_digest(text: &str) -> Vec<u8> {
    use sha2::Digest;
    sha2::Sha256::digest(text.as_bytes()).to_vec()
}

/// A built-in provider's name, for a provider id; a plugin's id is its own name.
pub fn provider_name(id: &str) -> &str {
    BUILT_IN
        .iter()
        .find(|provider| provider.id == id)
        .map_or(id, |provider| provider.name)
}

/// A key as Settings shows it: only its last four characters.
pub(crate) fn last_four(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    let tail: String = chars[chars.len().saturating_sub(4)..].iter().collect();

    format!("...{tail}")
}

fn secret_key(provider: &str) -> String {
    format!("websearch:{provider}")
}

impl Engine {
    pub fn web_search_settings(&self) -> WebSearch {
        self.store.setting(SETTINGS_KEY).ok().flatten().unwrap_or_default()
    }

    /// The keys saved in Settings for a built-in provider.
    fn saved_keys(&self, provider: &str) -> Vec<String> {
        self.credentials
            .secret(&secret_key(provider))
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    }

    /// The keys saved for a built-in provider, then its environment variable's.
    fn search_keys(&self, provider: &providers::Provider) -> Vec<String> {
        let from_environment = std::env::var(provider.env).ok().filter(|key| !key.trim().is_empty());

        self.saved_keys(provider.id)
            .into_iter()
            .chain(from_environment)
            .collect()
    }

    fn save_keys(&self, provider: &str, keys: &[String]) -> Result<(), String> {
        let result = if keys.is_empty() {
            self.credentials.remove_secret(&secret_key(provider))
        } else {
            self.credentials
                .set_secret(&secret_key(provider), &serde_json::to_string(keys).unwrap_or_default())
        };

        result.map_err(|error| error.to_string())
    }

    /// Adds a key to a built-in provider's list and tries every set-aside key again.
    pub fn add_search_key(&self, provider: &str, key: &str) -> Result<(), String> {
        if !BUILT_IN.iter().any(|known| known.id == provider) {
            return Err(format!("{provider} is not a provider that takes keys"));
        }
        let key = key.trim();
        if key.is_empty() {
            return Err("the key is empty".into());
        }
        let mut keys = self.saved_keys(provider);
        if !keys.iter().any(|known| known == key) {
            keys.push(key.to_string());
        }
        self.save_keys(provider, &keys)?;
        self.reset_search_keys();

        Ok(())
    }

    pub fn remove_search_key(&self, provider: &str, index: usize) -> Result<(), String> {
        let mut keys = self.saved_keys(provider);
        if index >= keys.len() {
            return Err(format!("{provider} has no key {index}"));
        }
        keys.remove(index);

        self.save_keys(provider, &keys)
    }

    /// Every provider, built in or a loaded plugin's, as Settings lists them.
    pub fn search_providers(&self) -> Vec<SearchProvider> {
        let mut found: Vec<SearchProvider> = BUILT_IN
            .iter()
            .map(|provider| SearchProvider {
                id: provider.id.into(),
                name: provider.name.into(),
                built_in: true,
                keys: self.saved_keys(provider.id).iter().map(|key| last_four(key)).collect(),
                from_environment: std::env::var(provider.env).is_ok_and(|key| !key.trim().is_empty()),
            })
            .collect();
        found.extend(
            self.hooks
                .search_providers()
                .into_iter()
                .map(|(id, name)| SearchProvider {
                    id,
                    name,
                    built_in: false,
                    keys: Vec::new(),
                    from_environment: false,
                }),
        );

        found
    }

    /// Whether the `websearch` tool is offered: not off, and some provider could answer.
    pub fn web_search_available(&self) -> bool {
        self.web_provider_for(Want::Search { developer: false })
    }

    /// Whether a page that does not load directly can be read through a provider instead.
    pub fn web_scrape_available(&self) -> bool {
        self.web_provider_for(Want::Scrape)
    }

    fn web_provider_for(&self, want: Want) -> bool {
        let settings = self.web_search_settings();
        settings.provider != "off" && !self.attempts(&settings.provider, want).is_empty()
    }

    /// Who to ask, in turn: one provider's keys, or every provider's in the user's order for `auto`; only
    /// providers that can do what is wanted, and plugins only for a plain search.
    fn attempts(&self, choice: &str, want: Want) -> Vec<Attempt> {
        let plugins: Vec<String> = match want {
            Want::Search { developer: false } => self.hooks.search_providers().into_iter().map(|(id, _)| id).collect(),
            _ => Vec::new(),
        };
        let mut ids: Vec<String> = self.web_search_settings().order;
        ids.extend(BUILT_IN.iter().map(|provider| provider.id.to_string()));
        ids.extend(plugins.iter().cloned());
        let mut seen = HashSet::new();
        ids.retain(|id| seen.insert(id.clone()) && (choice == "auto" || id == choice));

        ids.into_iter()
            .flat_map(|id| match BUILT_IN.iter().find(|provider| provider.id == id) {
                Some(provider) if provider.can(want) => self
                    .search_keys(provider)
                    .into_iter()
                    .map(|key| Attempt::Key { provider, key })
                    .collect(),
                None if plugins.contains(&id) => vec![Attempt::Plugin { provider: id }],
                _ => Vec::new(),
            })
            .collect()
    }

    /// Searches for `session_id`, returning the provider that answered and its results.
    pub async fn web_search(&self, session_id: &str, query: Query<'_>) -> Result<(String, Vec<Hit>), String> {
        let query = Query {
            limit: query.limit.clamp(1, MAX_RESULTS),
            ..query
        };
        let (provider, Answer::Hits(hits)) = self.route(session_id, &Job::Search(query)).await? else {
            return Err("the provider answered a search with a page".into());
        };

        Ok((provider, hits))
    }

    /// One page read through a provider, for a page that would not load directly.
    pub async fn web_scrape(&self, session_id: &str, url: &str) -> Result<(String, Page), String> {
        let (provider, Answer::Page(page)) = self.route(session_id, &Job::Scrape(url)).await? else {
            return Err("the provider answered a page with a list".into());
        };

        Ok((provider, page))
    }

    /// The pages of the site at `url`, those matching `query` first.
    pub async fn web_map(
        &self,
        session_id: &str,
        url: &str,
        query: Option<&str>,
        limit: u32,
    ) -> Result<(String, Vec<Hit>), String> {
        let job = Job::Map(Site {
            url,
            query,
            limit: limit.clamp(1, MAX_MAPPED),
        });
        let (provider, Answer::Hits(hits)) = self.route(session_id, &job).await? else {
            return Err("the provider answered a map with a page".into());
        };

        Ok((provider, hits))
    }

    /// Asks the providers that can do `job` in turn: the session's last provider first, then the rest in order,
    /// skipping keys that cool or were refused. The first answer wins.
    async fn route(&self, session_id: &str, job: &Job<'_>) -> Result<(String, Answer), String> {
        let settings = self.web_search_settings();
        if settings.provider == "off" {
            return Err("web search is off in Settings".into());
        }
        let want = job.want();
        let mut attempts = self.attempts(&settings.provider, want);
        if attempts.is_empty() {
            return Err(format!(
                "no web search provider with a key can {}; add one in Settings > Web search",
                want.what()
            ));
        }
        let sticky = self.search_routing.sticky.lock().unwrap().get(session_id).cloned();
        attempts.sort_by_key(|attempt| sticky.as_deref() != Some(provider_of(attempt)));

        let mut last = String::from("every web search provider is cooling down or refused its keys");
        for attempt in attempts {
            if self.unusable(&attempt.slot()) {
                continue;
            }
            match self.ask_provider(&attempt, job).await {
                Ok(answer) => {
                    let provider = provider_of(&attempt).to_string();
                    self.search_routing
                        .sticky
                        .lock()
                        .unwrap()
                        .insert(session_id.into(), provider.clone());
                    return Ok((provider, answer));
                }
                Err(error) => last = self.note_failure(&attempt, error),
            }
        }

        Err(last)
    }

    async fn ask_provider(&self, attempt: &Attempt, job: &Job<'_>) -> Result<Answer, SearchError> {
        let base = self.test_base();
        let base = base.as_deref();

        match (attempt, job) {
            (Attempt::Key { provider, key }, Job::Search(query)) => {
                providers::search(provider, key, *query, base).await.map(Answer::Hits)
            }
            (Attempt::Key { provider, key }, Job::Scrape(url)) => {
                providers::scrape(provider, key, url, base).await.map(Answer::Page)
            }
            (Attempt::Key { provider, key }, Job::Map(site)) => {
                providers::map(provider, key, *site, base).await.map(Answer::Hits)
            }
            (Attempt::Plugin { provider }, Job::Search(query)) => self
                .hooks
                .search(provider, query.text, query.limit)
                .await
                .map(Answer::Hits),
            (Attempt::Plugin { .. }, _) => Err(SearchError::Failed("a plugin provider only searches".into())),
        }
    }

    /// Where tests serve the built-in providers instead; never set outside them.
    fn test_base(&self) -> Option<String> {
        #[cfg(test)]
        return self.search_routing.base.lock().unwrap().clone();
        #[cfg(not(test))]
        None
    }

    fn unusable(&self, slot: &str) -> bool {
        let cooling = self
            .search_routing
            .cooling
            .lock()
            .unwrap()
            .get(slot)
            .is_some_and(|until| *until > Instant::now());
        cooling || self.search_routing.refused.lock().unwrap().contains(slot)
    }

    fn note_failure(&self, attempt: &Attempt, error: SearchError) -> String {
        let provider = provider_of(attempt);
        match error {
            SearchError::RateLimited(wait) => {
                let until = Instant::now() + wait.unwrap_or(DEFAULT_COOLDOWN);
                self.search_routing
                    .cooling
                    .lock()
                    .unwrap()
                    .insert(attempt.slot(), until);
                format!("{provider} is rate limited")
            }
            SearchError::Refused(why) => {
                self.search_routing.refused.lock().unwrap().insert(attempt.slot());
                format!("{provider} refused a key: {why}")
            }
            SearchError::Failed(why) => format!("{provider} failed: {why}"),
        }
    }

    /// Forgets which keys were set aside, so new or topped-up keys are tried again.
    pub fn reset_search_keys(&self) {
        self.search_routing.refused.lock().unwrap().clear();
    }
}

fn provider_of(attempt: &Attempt) -> &str {
    match attempt {
        Attempt::Key { provider, .. } => provider.id,
        Attempt::Plugin { provider } => provider,
    }
}

/// The `Retry-After` wait in seconds, when a response names one.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// Sends `body` and reads the answer as JSON, telling a refused key and a rate limit apart.
async fn post(request: reqwest::RequestBuilder, body: Value) -> Result<Value, SearchError> {
    let response = request
        .json(&body)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|error| SearchError::Failed(error.to_string()))?;
    let status = response.status().as_u16();
    let wait = retry_after(response.headers());
    let text = response.text().await.unwrap_or_default();

    match status {
        200..=299 => serde_json::from_str(&text).map_err(|error| SearchError::Failed(error.to_string())),
        // Tavily says a plan or pay-as-you-go limit was reached with 432 and 433.
        401..=403 | 432 | 433 => Err(SearchError::Refused(crate::llm::error_message(&text))),
        429 => Err(SearchError::RateLimited(wait)),
        _ => Err(SearchError::Failed(format!(
            "{status} {}",
            crate::llm::error_message(&text)
        ))),
    }
}

/// `text` cut to about `chars` characters on a word boundary.
fn trimmed(text: &str, chars: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= chars {
        return flat;
    }
    let cut: String = flat.chars().take(chars).collect();
    let end = cut.rfind(' ').unwrap_or(cut.len());

    format!("{}...", &cut[..end])
}

/// `text` kept to about `chars` characters, its lines as they were, cut at a line or word when it is longer.
pub(crate) fn capped(text: &str, chars: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(chars).collect();
    let end = cut.rfind('\n').or_else(|| cut.rfind(' ')).unwrap_or(cut.len());

    format!("{}\n\n(cut at {chars} characters)", cut[..end].trim_end())
}

/// A snippet read as text: markdown links become their words, empty anchor links and heading marks go, and so
/// does the search engine's own "More results from" tail.
fn plain(snippet: &str) -> String {
    let words = links(snippet, true);
    let words = words.split("More results from ").next().unwrap_or_default();

    words
        .split_whitespace()
        .filter(|word| !word.chars().all(|c| c == '#'))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text` with each markdown link `[words](target)` as its words, or gone when `keep_words` is false.
fn links(text: &str, keep_words: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let link = after
            .find("](")
            .and_then(|close| after[close + 2..].find(')').map(|end| (close, close + 2 + end)));
        match link {
            Some((close, end)) => {
                if keep_words {
                    out.push_str(after[..close].trim_matches(|c: char| c == '\u{200b}' || c.is_whitespace()));
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push('[');
                rest = after;
            }
        }
    }
    out.push_str(rest);

    out
}

/// A page's text without the run of link-only lines a site puts before it (skip links, settings, help); a page that
/// is nothing but links stays as it is.
pub(crate) fn without_chrome(text: &str) -> &str {
    let only_links = |line: &str| {
        let line = line.trim().trim_start_matches(['-', '*', '+']).trim();
        links(line, false).trim().is_empty()
    };
    let mut rest = text.trim_start();
    while let Some((line, after)) = rest.split_once('\n') {
        if !only_links(line) {
            break;
        }
        rest = after.trim_start();
    }

    if rest.is_empty() || only_links(rest) {
        text.trim()
    } else {
        rest
    }
}

/// Results out of a provider's list: the first of `snippet`'s fields that is there, and `content`'s field as the
/// page when it holds one.
pub(crate) fn json_hits(values: Option<&Vec<Value>>, snippet: &[&str], content: &str) -> Vec<Hit> {
    values
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let url = value["url"].as_str()?.to_string();
            let text = snippet
                .iter()
                .find_map(|key| match &value[*key] {
                    Value::String(text) => Some(text.clone()),
                    Value::Array(parts) => Some(parts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")),
                    _ => None,
                })
                .unwrap_or_default();
            let page = (!content.is_empty())
                .then(|| value[content].as_str())
                .flatten()
                .filter(|page| !page.trim().is_empty())
                .map(|page| capped(without_chrome(page), PAGE_CHARS));

            Some(Hit {
                title: value["title"].as_str().unwrap_or(&url).to_string(),
                snippet: trimmed(&plain(&text), 400),
                content: page,
                url,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
