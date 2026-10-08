use serde::Deserialize;
use std::collections::BTreeMap;

use super::{Cost, Limit, Model, ModelMode, PromptFamily, ProviderInfo, ToolProfile, Variant};
use super::{profile_for, prompt_for, speaks, variants_for};

/// Routes that carry a whole PDF when models.dev lists attachments but omits modalities.
const PDF_ROUTES: [&str; 6] = [
    "anthropic",
    "openai",
    "google",
    "google-vertex",
    "google-vertex-anthropic",
    "amazon-bedrock",
];

#[derive(Deserialize)]
pub(super) struct RawProvider {
    id: String,
    name: String,
    #[serde(default)]
    env: Option<Vec<String>>,
    #[serde(default)]
    api: Option<String>,
    #[serde(default)]
    models: BTreeMap<String, RawModel>,
}

#[derive(Deserialize)]
struct RawModel {
    id: String,
    name: String,
    #[serde(default)]
    family: Option<String>,
    #[serde(default)]
    reasoning: Option<bool>,
    #[serde(default)]
    attachment: Option<bool>,
    /// Already derived, as a cached catalog stores it.
    #[serde(default)]
    pdf: Option<bool>,
    #[serde(default)]
    modalities: Option<Modalities>,
    #[serde(default)]
    temperature: Option<bool>,
    #[serde(default)]
    tool_call: Option<bool>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    limit: Option<Limit>,
    #[serde(default)]
    cost: Option<Cost>,
    #[serde(default)]
    profile: Option<ToolProfile>,
    /// Already derived, as a cached catalog stores it.
    #[serde(default)]
    prompt: Option<PromptFamily>,
    /// models.dev's description of how the model's reasoning is set.
    #[serde(default)]
    reasoning_options: Option<Vec<serde_json::Value>>,
    /// Already derived, as a cached catalog stores it.
    #[serde(default)]
    variants: Option<Vec<Variant>>,
    #[serde(default)]
    experimental: Option<RawExperimental>,
    /// Already derived, as a cached catalog stores it.
    #[serde(default)]
    mode: Option<ModelMode>,
}

#[derive(Deserialize)]
struct Modalities {
    #[serde(default)]
    input: Vec<String>,
}

#[derive(Default, Deserialize)]
struct RawExperimental {
    #[serde(default)]
    modes: BTreeMap<String, RawMode>,
}

#[derive(Deserialize)]
struct RawMode {
    #[serde(default)]
    cost: Option<RawCost>,
    #[serde(default)]
    provider: Option<RawModeWire>,
}

/// A mode's prices replace the base model's prices only where supplied.
#[derive(Deserialize)]
struct RawCost {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

#[derive(Default, Deserialize)]
struct RawModeWire {
    #[serde(default)]
    body: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
}

impl RawProvider {
    pub(super) fn into_info(self, provider_id: &str) -> ProviderInfo {
        let listed = self.models.into_iter().filter(|(_, model)| {
            model.tool_call.unwrap_or(true)
                && !matches!(model.status.as_deref(), Some("deprecated" | "retired"))
                && speaks(provider_id, &model.id)
        });
        let mut models = BTreeMap::new();

        for (key, mut raw) in listed {
            let modes = raw.experimental.take().unwrap_or_default().modes;
            let base = model_from_raw(raw, provider_id);

            // An explicitly listed model wins over a synthesized mode with the same id.
            for (name, mode) in modes {
                models
                    .entry(format!("{key}-{name}"))
                    .or_insert_with(|| mode_model(&key, &base, &name, mode));
            }
            models.insert(key, base);
        }

        // Only the user's config may redirect a native provider's endpoint.
        let api = self.api.filter(|_| {
            !matches!(
                provider_id,
                "anthropic" | "openai" | "google" | "amazon-bedrock" | "google-vertex" | "google-vertex-anthropic"
            )
        });

        ProviderInfo {
            id: self.id,
            name: self.name,
            env: self.env.unwrap_or_default(),
            api,
            models,
        }
    }
}

fn model_from_raw(model: RawModel, provider_id: &str) -> Model {
    let family = model.family.unwrap_or_default();
    let profile = model.profile.unwrap_or_else(|| profile_for(provider_id, &family));
    let prompt = model.prompt.unwrap_or_else(|| prompt_for(profile, &family));
    let limit = model.limit.unwrap_or_default();
    let reasoning = model.reasoning.unwrap_or(false);
    let attachment = model.attachment.unwrap_or(false);

    let listed_pdf = model
        .modalities
        .as_ref()
        .map(|modalities| modalities.input.iter().any(|kind| kind == "pdf"));
    let pdf = model
        .pdf
        .or(listed_pdf)
        .unwrap_or(attachment && PDF_ROUTES.contains(&provider_id));
    let variants = match model.variants {
        Some(variants) => variants,
        None if reasoning => variants_for(
            provider_id,
            &model.id,
            limit.output,
            model.reasoning_options.as_deref().unwrap_or_default(),
        ),
        None => Vec::new(),
    };

    Model {
        id: model.id,
        name: model.name,
        family,
        reasoning,
        attachment,
        pdf,
        temperature: model.temperature.unwrap_or(false),
        release_date: model.release_date.unwrap_or_default(),
        limit,
        cost: model.cost.unwrap_or_default(),
        profile,
        prompt,
        variants,
        mode: model.mode,
    }
}

/// Runs a base model in a named mode, replacing only the prices the mode specifies.
fn mode_model(key: &str, base: &Model, name: &str, mode: RawMode) -> Model {
    let mut characters = name.chars();
    let title: String = characters
        .next()
        .map(|first| first.to_uppercase().chain(characters).collect())
        .unwrap_or_default();
    let given = mode.cost.unwrap_or(RawCost {
        input: None,
        output: None,
        cache_read: None,
        cache_write: None,
    });

    // A mode with its own prices has no inherited long-prompt tiers.
    let priced =
        given.input.is_some() || given.output.is_some() || given.cache_read.is_some() || given.cache_write.is_some();
    let cost = Cost {
        input: given.input.unwrap_or(base.cost.input),
        output: given.output.unwrap_or(base.cost.output),
        cache_read: given.cache_read.unwrap_or(base.cost.cache_read),
        cache_write: given.cache_write.unwrap_or(base.cost.cache_write),
        tiers: if priced { Vec::new() } else { base.cost.tiers.clone() },
    };
    let wire = mode.provider.unwrap_or_default();

    Model {
        id: format!("{}-{name}", base.id),
        name: format!("{} {title}", base.name),
        cost,
        mode: Some(ModelMode {
            name: name.into(),
            base: key.into(),
            body: wire.body,
            headers: wire.headers,
        }),
        ..base.clone()
    }
}
