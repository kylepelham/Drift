//! Fetching and reading the account's Codex model list.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::Offer;
use crate::llm::Credential;

/// The list is a few kilobytes; anything far larger is not it.
const MAX_BODY: usize = 2 * 1024 * 1024;

/// The Daybreak programs Drift knows, in the order it prefers them.
const PROGRAMS: [&str; 2] = ["daybreak_blue", "daybreak_red"];

#[derive(Deserialize)]
struct ModelList {
    models: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    slug: String,
    #[serde(default)]
    available_access_programs: Programs,
    #[serde(default)]
    service_tiers: Vec<Tier>,
    #[serde(default)]
    supports_search_tool: bool,
}

#[derive(Default, Deserialize)]
struct Programs {
    #[serde(default)]
    cyber: Vec<String>,
}

#[derive(Deserialize)]
struct Tier {
    id: String,
}

/// The account's offers by model slug; `None` when the list could not be fetched or read.
pub(super) async fn fetch(
    client: &reqwest::Client,
    base: &str,
    credential: &Credential,
) -> Option<BTreeMap<String, Offer>> {
    let Credential::OAuth { access, account, .. } = credential else {
        return None;
    };

    // The backend refuses the list without a client version.
    let request = client.get(format!("{base}/models?client_version={}", crate::VERSION));
    let request = crate::llm::openai::signed_by_subscription(request, access, account.as_deref());

    let response = request.send().await.ok()?.error_for_status().ok()?;
    let body = bounded(response).await?;
    let list: ModelList = serde_json::from_slice(&body).ok()?;

    Some(list.models.into_iter().map(offer).collect())
}

async fn bounded(mut response: reqwest::Response) -> Option<Vec<u8>> {
    let mut body = Vec::new();

    while let Some(chunk) = response.chunk().await.ok()? {
        if body.len() + chunk.len() > MAX_BODY {
            return None;
        }
        body.extend_from_slice(&chunk);
    }

    Some(body)
}

fn offer(entry: Entry) -> (String, Offer) {
    let offered = &entry.available_access_programs.cyber;
    let program = PROGRAMS
        .into_iter()
        .find(|program| offered.iter().any(|value| value == program))
        .map(String::from);
    let tiers = entry.service_tiers.into_iter().map(|tier| tier.id).collect();

    let offer = Offer {
        program,
        tiers,
        tool_search: entry.supports_search_tool,
    };

    (entry.slug, offer)
}

#[cfg(test)]
pub(super) fn read(json: serde_json::Value) -> BTreeMap<String, Offer> {
    let list: ModelList = serde_json::from_value(json).unwrap();
    list.models.into_iter().map(offer).collect()
}
