use std::collections::BTreeMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::llm::Credential;
use crate::llm::catalog::{ModelMode, ProviderInfo};

const TTL: Duration = Duration::from_secs(15 * 60);
const RETRY: Duration = Duration::from_secs(60);
const WAIT: Duration = Duration::from_secs(10);
const MAX_BODY: usize = 2 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct Daybreak {
    state: RwLock<State>,
    refreshing: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct State {
    generation: u64,
    cached: Option<Cached>,
}

struct Cached {
    identity: String,
    until: Instant,
    programs: BTreeMap<String, String>,
}

impl Daybreak {
    pub(crate) fn clear(&self) -> bool {
        let mut state = self.state.write().unwrap();
        state.generation += 1;
        state.cached.take().is_some_and(|cached| !cached.programs.is_empty())
    }

    pub(crate) fn apply(&self, provider: &mut ProviderInfo, credential: &Credential) {
        let Some(identity) = identity(credential) else { return };
        let state = self.state.read().unwrap();
        let Some(cached) = state
            .cached
            .as_ref()
            .filter(|cached| cached.identity == identity && cached.until > Instant::now())
        else {
            return;
        };
        let additions: Vec<_> = provider
            .models
            .values()
            .filter_map(|base| {
                if base.mode.as_ref().is_some_and(|mode| mode.name == "daybreak") {
                    return None;
                }
                let wire = base.wire(&base.id);
                let program = cached.programs.get(wire)?;
                let mut model = base.clone();
                let mut mode = base.mode.clone().unwrap_or_else(|| ModelMode {
                    name: String::new(),
                    base: wire.into(),
                    body: Default::default(),
                    headers: Default::default(),
                });
                mode.name = "daybreak".into();
                mode.body.insert("access_programs".into(), json!({ "cyber": program }));
                model.id = format!("{}-daybreak", base.id);
                model.name = format!("{} Daybreak", base.name);
                model.mode = Some(mode);
                Some((model.id.clone(), model))
            })
            .collect();
        provider.models.extend(additions);
    }

    pub(crate) async fn refresh(&self, client: &reqwest::Client, base: &str, credential: &Credential) -> bool {
        let Some(identity) = identity(credential) else {
            return self.clear();
        };
        let _held = self.refreshing.lock().await;
        let generation = {
            let state = self.state.read().unwrap();
            if state
                .cached
                .as_ref()
                .is_some_and(|cached| cached.identity == identity && cached.until > Instant::now())
            {
                return false;
            }
            state.generation
        };
        let found = tokio::time::timeout(WAIT, discover(client, base, credential))
            .await
            .ok()
            .flatten();
        let until = Instant::now() + if found.is_some() { TTL } else { RETRY };
        let programs = found.unwrap_or_default();
        let mut state = self.state.write().unwrap();
        if state.generation != generation {
            return false;
        }
        let changed = state.cached.as_ref().is_none_or(|cached| {
            cached.identity != identity || cached.programs != programs || cached.until <= Instant::now()
        });
        state.cached = Some(Cached {
            identity,
            until,
            programs,
        });
        changed
    }
}

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

#[derive(Deserialize)]
struct ModelList {
    models: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    slug: String,
    #[serde(default)]
    available_access_programs: Programs,
}

#[derive(Default, Deserialize)]
struct Programs {
    #[serde(default)]
    cyber: Vec<String>,
}

async fn discover(client: &reqwest::Client, base: &str, credential: &Credential) -> Option<BTreeMap<String, String>> {
    let Credential::OAuth { access, account, .. } = credential else {
        return None;
    };
    let mut request = client
        .get(format!("{base}/models?client_version={}", crate::VERSION))
        .bearer_auth(access)
        .header("originator", super::super::CODEX_ORIGINATOR);
    if let Some(account) = account {
        request = request.header("chatgpt-account-id", account);
    }
    if let Some(residency) = super::super::residency(access) {
        request = request.header("x-openai-internal-codex-residency", residency);
    }
    let mut response = request.send().await.ok()?.error_for_status().ok()?;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if body.len() + chunk.len() > MAX_BODY {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    let list: ModelList = serde_json::from_slice(&body).ok()?;
    Some(list.models.into_iter().filter_map(program).collect())
}

fn program(entry: Entry) -> Option<(String, String)> {
    ["daybreak_blue", "daybreak_red"]
        .into_iter()
        .find(|program| {
            entry
                .available_access_programs
                .cyber
                .iter()
                .any(|value| value == program)
        })
        .map(|program| (entry.slug, program.into()))
}

#[cfg(test)]
mod tests;
