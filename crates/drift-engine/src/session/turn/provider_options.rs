//! What the user chooses per provider under Settings > Providers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::Engine;

pub const PROVIDER_OPTIONS_KEY: &str = "providerOptions";

/// The routes that cache Claude for an hour when asked, at twice the write price: Anthropic's API by key, Bedrock,
/// Vertex and OpenRouter. A Claude subscription caches for an hour whatever its option says.
pub const HOUR_CACHE_PROVIDERS: [&str; 5] = [
    "anthropic",
    "amazon-bedrock",
    "google-vertex-anthropic",
    "google-vertex",
    "openrouter",
];

/// One provider's options, all off by default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderOptions {
    /// Requests keep their prompt cache for an hour instead of five minutes; only on [`HOUR_CACHE_PROVIDERS`].
    pub hour_cache: bool,
}

impl Engine {
    /// Each provider's options, by provider id; a provider not listed has the defaults.
    pub fn provider_options(&self) -> BTreeMap<String, ProviderOptions> {
        self.store
            .setting(PROVIDER_OPTIONS_KEY)
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    pub(super) fn options_for(&self, provider: &str) -> ProviderOptions {
        self.provider_options().remove(provider).unwrap_or_default()
    }

    /// Replaces the options of each provider given and keeps the others.
    pub fn set_provider_options(&self, changed: BTreeMap<String, ProviderOptions>) -> rusqlite::Result<()> {
        let mut options = self.provider_options();
        options.extend(changed);

        self.store.set_setting(PROVIDER_OPTIONS_KEY, &options)
    }
}
