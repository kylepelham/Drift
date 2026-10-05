//! What a ChatGPT sign-in can use: the Codex backend takes only some models, with its own limits. They keep their API
//! prices, so a turn shows what the plan saves.

use crate::llm::catalog::{Limit, ProviderInfo};

/// Accepted although their version alone would not be.
const ALLOWED: [&str; 6] = ["gpt-5.5", "gpt-5.3-codex-spark", "gpt-5.4", "gpt-5.4-mini", "gpt-6-sol", "gpt-6-luna"];
/// Refused although their version alone would be.
const REFUSED: [&str; 2] = ["gpt-5.5-pro", "gpt-5.6"];

/// Keeps the models the backend takes, with the 400k window and 272k prompt it gives the 5.5 and 5.6 lines.
pub fn shape(provider: &mut ProviderInfo) {
    provider.models.retain(|id, _| accepted(id));
    for model in provider.models.values_mut() {
        if model.id.contains("gpt-5.5") || model.id.contains("gpt-5.6") {
            model.limit = Limit { context: 400_000, output: 128_000, input: 272_000 };
        }
    }
}

/// The model small jobs (titles) use under a ChatGPT sign-in: it spends least of the plan, whatever the API prices say.
const SMALL: &str = "gpt-5.4-mini";

/// `SMALL` when the conversation runs on an OpenAI model through a ChatGPT sign-in and the backend offers it.
pub fn small_model(catalog: &crate::llm::catalog::Catalog, like: &crate::session::types::ModelRef, credential: &crate::llm::Credential) -> Option<crate::session::types::ModelRef> {
    let signed_in = like.provider == "openai" && matches!(credential, crate::llm::Credential::OAuth { .. });
    let offered = catalog.model("openai", SMALL).is_some() && like.model != SMALL;
    (signed_in && offered).then(|| crate::session::types::ModelRef { provider: "openai".into(), model: SMALL.into() })
}

fn accepted(id: &str) -> bool {
    if id.ends_with("-pro") || REFUSED.contains(&id) {
        return false;
    }
    if ALLOWED.contains(&id) {
        return true;
    }
    let Some(version) = id.strip_prefix("gpt-") else { return false };
    let mut numbers = version.split(|c: char| !c.is_ascii_digit()).map(str::parse::<u32>);
    let major = numbers.next().and_then(Result::ok);
    let minor = if version.trim_start_matches(|c: char| c.is_ascii_digit()).starts_with('.') { numbers.next().and_then(Result::ok).unwrap_or(0) } else { 0 };
    major.is_some_and(|major| major > 5 || (major == 5 && minor > 4))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_models_the_backend_takes_are_offered_with_its_limits() {
        for (id, offered) in [
            ("gpt-5.5", true),
            ("gpt-5.4-mini", true),
            ("gpt-5.6-codex", true),
            ("gpt-6-sol", true),
            ("gpt-7", true),
            ("gpt-5.6", false),
            ("gpt-5.5-pro", false),
            ("gpt-5.3-codex", false),
            ("gpt-5", false),
            ("gpt-4.1", false),
            ("o3", false),
        ] {
            assert_eq!(accepted(id), offered, "{id}");
        }
        let mut provider = crate::llm::catalog::Catalog::bundled().providers["openai"].clone();
        shape(&mut provider);
        assert!(provider.models.keys().all(|id| accepted(id)) && !provider.models.is_empty());
        assert!(provider.models.values().any(|model| model.cost.input > 0.0), "API prices are kept");
        if let Some(model) = provider.models.get("gpt-5.5") {
            assert_eq!(model.limit, Limit { context: 400_000, output: 128_000, input: 272_000 });
        }
    }
}
