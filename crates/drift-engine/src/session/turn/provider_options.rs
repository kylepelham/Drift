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

/// Whether a route keeps this model's cache for an hour when asked: only Claude does, so Gemini on Vertex or GPT,
/// DeepSeek and Qwen through OpenRouter keep their own lifetimes and are never sent an hour's breakpoint.
pub fn caches_for_an_hour(provider: &str, model: &str) -> bool {
    HOUR_CACHE_PROVIDERS.contains(&provider) && model.contains("claude")
}

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

#[cfg(test)]
mod tests {
    use super::caches_for_an_hour;

    #[test]
    fn only_claude_caches_for_an_hour_and_only_on_routes_that_offer_it() {
        for (provider, model) in [
            ("anthropic", "claude-sonnet-5-5"),
            ("openrouter", "anthropic/claude-sonnet-5.5"),
            ("openrouter", "~anthropic/claude-sonnet-latest"),
            ("amazon-bedrock", "anthropic.claude-opus-5-5-v1:0"),
            ("google-vertex", "claude-opus-5-5"),
        ] {
            assert!(caches_for_an_hour(provider, model), "{provider} {model}");
        }
        for (provider, model) in [
            ("openrouter", "openai/gpt-5.6"),
            ("openrouter", "qwen/qwen3-coder-plus"),
            ("openrouter", "deepseek/deepseek-v3.2"),
            ("google-vertex", "gemini-3-pro"),
            ("amazon-bedrock", "amazon.nova-pro-v1:0"),
            ("openai", "gpt-5.6"),
        ] {
            assert!(!caches_for_an_hour(provider, model), "{provider} {model}");
        }
    }
}
