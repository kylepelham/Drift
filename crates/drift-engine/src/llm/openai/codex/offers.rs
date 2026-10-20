//! What each signed-in ChatGPT account's Codex model list offers, per model: its access programs (Daybreak)
//! and its speed tiers. Fetched in the background and kept per account; the catalog shows what any account
//! offers, and a turn goes to an account that offers what it asks for.

use std::collections::{BTreeMap, HashMap};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::llm::Credential;
use crate::llm::catalog::{Model, ProviderInfo};

mod catalog;
mod list;

/// A fetched list is trusted this long.
const TTL: Duration = Duration::from_secs(15 * 60);
/// A failed fetch is tried again after this long.
const RETRY: Duration = Duration::from_secs(60);
/// The longest a fetch may take.
const WAIT: Duration = Duration::from_secs(10);

/// What the account's list says about one model.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Offer {
    /// The Daybreak program to request, when the model has one.
    pub program: Option<String>,
    /// The `service_tier` values the account may send for it.
    pub tiers: Vec<String>,
    /// It loads deferred tools through a client-run `tool_search` (`supports_search_tool`).
    pub tool_search: bool,
}

#[derive(Default)]
pub(crate) struct Offers {
    state: RwLock<State>,
    /// Only one fetch at a time; the others wait and use its result.
    refreshing: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct State {
    /// Bumped by every sign-in or sign-out, so a fetch that started before one cannot repopulate the cache.
    generation: u64,
    /// Each account's list, by its hashed identity.
    cached: HashMap<String, Cached>,
}

struct Cached {
    until: Instant,
    offers: BTreeMap<String, Offer>,
}

impl Cached {
    fn fresh(&self) -> bool {
        self.until > Instant::now()
    }
}

impl State {
    fn fresh_offers(&self, credential: &Credential) -> Option<&BTreeMap<String, Offer>> {
        let identity = identity(credential)?;

        self.cached
            .get(&identity)
            .filter(|cached| cached.fresh())
            .map(|cached| &cached.offers)
    }
}

impl Offers {
    /// Forgets every account's list; true when that changes what the catalog shows.
    pub(crate) fn clear(&self) -> bool {
        let mut state = self.state.write().unwrap();
        state.generation += 1;

        let cleared = std::mem::take(&mut state.cached);
        cleared.values().any(|cached| !cached.offers.is_empty())
    }

    /// Drops speed modes no account can use and adds a Daybreak entry for every model any account offers a program
    /// for. `credentials` are the provider's accounts in the user's order. Nothing changes without a current list.
    pub(crate) fn apply(&self, provider: &mut ProviderInfo, credentials: &[Credential]) {
        let state = self.state.read().unwrap();
        let lists: Vec<_> = credentials
            .iter()
            .filter_map(|credential| state.fresh_offers(credential))
            .collect();
        if lists.is_empty() {
            return;
        }

        let offers = merged(&lists);
        catalog::drop_unoffered_speeds(provider, &offers);
        catalog::mark_tool_search(provider, &offers);
        catalog::add_daybreak(provider, &offers);
    }

    /// Whether this account offers what `model` asks for: its Daybreak program and its speed tier.
    /// `None` when the model asks for neither, or the account has no current list.
    pub(crate) fn serves(&self, credential: &Credential, model: &Model) -> Option<bool> {
        let mode = model.mode.as_ref()?;
        let tier = mode.body.get("service_tier").and_then(|tier| tier.as_str());
        let daybreak = mode.name == catalog::DAYBREAK;
        if tier.is_none() && !daybreak {
            return None;
        }

        let state = self.state.read().unwrap();
        let offer = state.fresh_offers(credential)?.get(&mode.base);

        // A model the list does not name keeps its speed modes, as the catalog does.
        let tier_offered = tier.is_none_or(|tier| offer.is_none_or(|offer| offer.tiers.iter().any(|t| t == tier)));
        let program_offered = !daybreak || offer.is_some_and(|offer| offer.program.is_some());

        Some(tier_offered && program_offered)
    }

    /// Puts this account's own Daybreak program in `model`'s request body; accounts can be in different programs.
    pub(crate) fn fit(&self, model: &mut Model, credential: &Credential) {
        let Some(mode) = model.mode.as_mut().filter(|mode| mode.name == catalog::DAYBREAK) else {
            return;
        };

        let state = self.state.read().unwrap();
        let program = state
            .fresh_offers(credential)
            .and_then(|offers| offers.get(&mode.base))
            .and_then(|offer| offer.program.clone());
        if let Some(program) = program {
            mode.body
                .insert("access_programs".into(), serde_json::json!({ "cyber": program }));
        }
    }

    /// Fetches the account's list unless a current one is cached; true when what the catalog shows changed.
    pub(crate) async fn refresh(&self, client: &reqwest::Client, base: &str, credential: &Credential) -> bool {
        let Some(identity) = identity(credential) else {
            return self.clear();
        };
        let _held = self.refreshing.lock().await;

        let generation = {
            let state = self.state.read().unwrap();
            if state.cached.get(&identity).is_some_and(Cached::fresh) {
                return false;
            }
            state.generation
        };

        // A failed fetch caches an empty list for a minute: nothing is changed or added meanwhile.
        let fetched = tokio::time::timeout(WAIT, list::fetch(client, base, credential))
            .await
            .ok()
            .flatten();
        let until = Instant::now() + if fetched.is_some() { TTL } else { RETRY };
        let offers = fetched.unwrap_or_default();

        let mut state = self.state.write().unwrap();
        if state.generation != generation {
            return false;
        }

        let changed = state
            .cached
            .get(&identity)
            .is_none_or(|cached| cached.offers != offers || !cached.fresh());
        state.cached.insert(identity, Cached { until, offers });

        changed
    }
}

/// Every account's offers as one list: the tiers any account offers, the first account's program per model, and
/// tool search where any account has it.
fn merged(lists: &[&BTreeMap<String, Offer>]) -> BTreeMap<String, Offer> {
    let mut merged: BTreeMap<String, Offer> = BTreeMap::new();

    for list in lists {
        for (model, offer) in *list {
            let entry = merged.entry(model.clone()).or_default();
            if entry.program.is_none() {
                entry.program.clone_from(&offer.program);
            }
            entry.tool_search |= offer.tool_search;
            for tier in &offer.tiers {
                if !entry.tiers.contains(tier) {
                    entry.tiers.push(tier.clone());
                }
            }
        }
    }

    merged
}

/// The ChatGPT account and user a sign-in belongs to, hashed; a refreshed token keeps the same identity.
fn identity(credential: &Credential) -> Option<String> {
    let Credential::OAuth { access, account, .. } = credential else {
        return None;
    };

    let claims: Option<Value> = access
        .split('.')
        .nth(1)
        .and_then(|payload| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(payload.trim_end_matches('='))
                .ok()
        })
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let user = claims.as_ref().and_then(|claims| claims["sub"].as_str());
    let account = account.clone().or_else(|| super::super::oauth::account_id(access));

    let scope = serde_json::to_vec(&(account, user.unwrap_or(access))).ok()?;
    Some(crate::hex_bytes(&Sha256::digest(&scope)))
}

#[cfg(test)]
mod tests;
