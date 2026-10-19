//! Web search for the `websearch` tool. Every provider, built in or a plugin's, answers the same way, so
//! routing never knows which kind it called. Built-in providers take a list of keys: a key refused (401,
//! 402) is set aside and the next tried, and a key rate limited (429) cools for the wait it names. In
//! `auto`, a provider with no key left to try hands over to the next in the user's order. Cooldowns are
//! shared by every session; a session keeps the provider that last answered it.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::Engine;

mod providers;

pub use providers::BUILT_IN;

pub const SETTINGS_KEY: &str = "webSearch";
/// A rate limit that names no wait cools its key this long.
const DEFAULT_COOLDOWN: Duration = Duration::from_secs(60);
/// The most results one search asks for.
pub const MAX_RESULTS: u32 = 20;

/// One result: where it is, what it is called, and the text that matched.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    pub url: String,
    pub title: String,
    pub snippet: String,
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
        let settings = self.web_search_settings();
        settings.provider != "off" && !self.attempts(&settings.provider).is_empty()
    }

    /// Who to ask, in turn: one provider's keys, or every provider's in the user's order for `auto`.
    fn attempts(&self, choice: &str) -> Vec<Attempt> {
        let plugins: Vec<String> = self.hooks.search_providers().into_iter().map(|(id, _)| id).collect();
        let mut ids: Vec<String> = self.web_search_settings().order;
        ids.extend(BUILT_IN.iter().map(|provider| provider.id.to_string()));
        ids.extend(plugins.iter().cloned());
        let mut seen = HashSet::new();
        ids.retain(|id| seen.insert(id.clone()) && (choice == "auto" || id == choice));

        ids.into_iter()
            .flat_map(|id| match BUILT_IN.iter().find(|provider| provider.id == id) {
                Some(provider) => self
                    .search_keys(provider)
                    .into_iter()
                    .map(|key| Attempt::Key { provider, key })
                    .collect(),
                None if plugins.contains(&id) => vec![Attempt::Plugin { provider: id }],
                None => Vec::new(),
            })
            .collect()
    }

    /// Searches for `session_id`: its last provider first, then the rest in order, skipping what cools.
    pub async fn web_search(&self, session_id: &str, query: &str, limit: u32) -> Result<(String, Vec<Hit>), String> {
        let settings = self.web_search_settings();
        if settings.provider == "off" {
            return Err("web search is off in Settings".into());
        }
        let mut attempts = self.attempts(&settings.provider);
        if attempts.is_empty() {
            return Err("no web search provider has a key; add one in Settings > Web search".into());
        }
        let sticky = self.search_routing.sticky.lock().unwrap().get(session_id).cloned();
        attempts.sort_by_key(|attempt| sticky.as_deref() != Some(provider_of(attempt)));

        let mut last = String::from("every web search provider is cooling down or refused its keys");
        for attempt in attempts {
            if self.unusable(&attempt.slot()) {
                continue;
            }
            match self.ask_provider(&attempt, query, limit.clamp(1, MAX_RESULTS)).await {
                Ok(hits) => {
                    let provider = provider_of(&attempt).to_string();
                    self.search_routing
                        .sticky
                        .lock()
                        .unwrap()
                        .insert(session_id.into(), provider.clone());
                    return Ok((provider, hits));
                }
                Err(error) => last = self.note_failure(&attempt, error),
            }
        }

        Err(last)
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

    async fn ask_provider(&self, attempt: &Attempt, query: &str, limit: u32) -> Result<Vec<Hit>, SearchError> {
        match attempt {
            Attempt::Key { provider, key } => {
                #[cfg(test)]
                let base = self.search_routing.base.lock().unwrap().clone();
                #[cfg(not(test))]
                let base: Option<String> = None;
                providers::search(provider, key, query, limit, base.as_deref()).await
            }
            Attempt::Plugin { provider } => self.hooks.search(provider, query, limit).await,
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
        401..=403 => Err(SearchError::Refused(crate::llm::error_message(&text))),
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

pub(crate) fn json_hits(values: Option<&Vec<Value>>, snippet: &[&str]) -> Vec<Hit> {
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
            Some(Hit {
                title: value["title"].as_str().unwrap_or(&url).to_string(),
                snippet: trimmed(&text, 400),
                url,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
