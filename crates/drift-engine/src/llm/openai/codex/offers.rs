//! What the signed-in ChatGPT account's Codex model list offers, per model: its access programs (Daybreak)
//! and its speed tiers. Fetched in the background, kept per account, and applied to the catalog view.

use std::collections::BTreeMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::llm::Credential;
use crate::llm::catalog::ProviderInfo;

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
}

#[derive(Default)]
pub(crate) struct Offers {
    state: RwLock<State>,
    /// Only one fetch at a time; the others wait and use its result.
    refreshing: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct State {
    /// Bumped by every sign-out, so a fetch that started before one cannot repopulate the cache.
    generation: u64,
    cached: Option<Cached>,
}

struct Cached {
    /// The account the list belongs to, hashed.
    identity: String,
    until: Instant,
    offers: BTreeMap<String, Offer>,
}

impl Cached {
    fn fresh_for(&self, identity: &str) -> bool {
        self.identity == identity && self.until > Instant::now()
    }
}

impl Offers {
    /// Forgets the account's list; true when that changes what the catalog shows.
    pub(crate) fn clear(&self) -> bool {
        let mut state = self.state.write().unwrap();
        state.generation += 1;

        state.cached.take().is_some_and(|cached| !cached.offers.is_empty())
    }

    /// Drops speed modes the account cannot use and adds its Daybreak entries; nothing without a current list.
    pub(crate) fn apply(&self, provider: &mut ProviderInfo, credential: &Credential) {
        let Some(identity) = identity(credential) else {
            return;
        };
        let state = self.state.read().unwrap();
        let Some(cached) = state.cached.as_ref().filter(|cached| cached.fresh_for(&identity)) else {
            return;
        };

        catalog::drop_unoffered_speeds(provider, &cached.offers);
        catalog::add_daybreak(provider, &cached.offers);
    }

    /// Fetches the account's list unless a current one is cached; true when what the catalog shows changed.
    pub(crate) async fn refresh(&self, client: &reqwest::Client, base: &str, credential: &Credential) -> bool {
        let Some(identity) = identity(credential) else {
            return self.clear();
        };
        let _held = self.refreshing.lock().await;

        let generation = {
            let state = self.state.read().unwrap();
            if state.cached.as_ref().is_some_and(|cached| cached.fresh_for(&identity)) {
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

        let changed = state.cached.as_ref().is_none_or(|cached| {
            cached.identity != identity || cached.offers != offers || cached.until <= Instant::now()
        });
        state.cached = Some(Cached {
            identity,
            until,
            offers,
        });

        changed
    }
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
