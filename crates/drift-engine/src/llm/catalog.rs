//! Which models exist, what they cost and how they take tools. models.dev filtered to our providers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::config::{ProviderConfig, ProviderModel};
use crate::session::types::ModelRef;

mod cost;
mod raw;

pub use cost::{Cost, CostTier};
use raw::RawProvider;

const SNAPSHOT: &str = include_str!("../../data/models.json");
const SOURCE_URL: &str = "https://models.dev/api.json";
/// Renamed whenever the entries made from models.dev change, so a cache an older build wrote is not read.
const CACHE_FILE: &str = "models-2.json";
const OLDER_CACHES: [&str; 1] = ["models.json"];
const CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
const SMALL_MODEL_MIN_CONTEXT: u64 = 16_000;
pub const PROVIDERS: [&str; 11] = [
    "anthropic",
    "openai",
    "google",
    "xai",
    "zai",
    "openrouter",
    "amazon-bedrock",
    "google-vertex",
    "google-vertex-anthropic",
    "lmstudio",
    "ollama",
];

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error(transparent)]
    Download(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Cache(#[from] std::io::Error),
}

/// How a model edits files: what its training makes it good at, decided here and nowhere else.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolProfile {
    Edit,
    ApplyPatch,
}

/// Which base prompt a model gets, the one written for how its family works; decided here and nowhere else.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PromptFamily {
    /// OpenAI's GPT-5 generation: exactly the models that edit with `apply_patch`.
    Codex,
    Claude,
    Gemini,
    #[default]
    Default,
}

impl PromptFamily {
    pub const ALL: [PromptFamily; 4] = [Self::Codex, Self::Claude, Self::Gemini, Self::Default];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Gemini => "gemini",
            Self::Default => "default",
        }
    }
}

/// From the tool profile (so the prompt and the edit tool always agree) and models.dev's `family`, never the id.
fn prompt_for(profile: ToolProfile, family: &str) -> PromptFamily {
    match (profile, family) {
        (ToolProfile::ApplyPatch, _) => PromptFamily::Codex,
        (_, family) if family.starts_with("claude") => PromptFamily::Claude,
        (_, family) if family.starts_with("gemini") => PromptFamily::Gemini,
        _ => PromptFamily::Default,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Limit {
    pub context: u64,
    pub output: u64,
    /// The most prompt the provider takes, when it is less than the window (gpt-5.4: 922k of 1.05M); 0 when not given.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub input: u64,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// Of the reply room, what is kept below an input cap; the UI's meter uses the same (`compactionReserveTokens`).
const INPUT_RESERVE: u64 = 20_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Model {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub attachment: bool,
    /// Whether it reads PDFs sent whole.
    #[serde(default)]
    pub pdf: bool,
    #[serde(default)]
    pub temperature: bool,
    #[serde(default)]
    pub release_date: String,
    #[serde(default)]
    pub limit: Limit,
    #[serde(default)]
    pub cost: Cost,
    #[serde(default = "default_profile")]
    pub profile: ToolProfile,
    #[serde(default)]
    pub prompt: PromptFamily,
    /// The reasoning levels the model offers, weakest first; empty when it has none to choose.
    #[serde(default)]
    pub variants: Vec<Variant>,
    /// Set on an entry that runs another model a different way (`<id>-fast`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ModelMode>,
}

/// A faster, cheaper or deeper way to run a model (models.dev `experimental.modes`), listed as its own
/// model `<id>-<mode>` as opencode lists it: the base model's id on the wire, with these fields and headers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ModelMode {
    pub name: String,
    /// The model it runs, as the provider names it.
    pub base: String,
    /// Request body fields (`speed`, `service_tier`, `reasoning.mode`), laid over the adapter's own.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub body: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

fn default_profile() -> ToolProfile {
    ToolProfile::Edit
}

/// The longest reply asked for, whatever the model allows.
pub const MAX_REPLY_TOKENS: u64 = 32_000;
/// Below this window the system prompt and tool schemas leave little room for any work.
pub const SMALL_CONTEXT: u64 = 16_384;

impl Model {
    /// The id the provider is sent for the entry listed under `key`: a mode's base model, else the entry itself.
    pub fn wire<'a>(&'a self, key: &'a str) -> &'a str {
        self.mode.as_ref().map_or(key, |mode| mode.base.as_str())
    }

    /// Reserves the output limit when known, otherwise a quarter of the context window.
    /// Small local models must leave room for the prompt rather than reserve their whole window.
    /// Never reserves more than half a known window or [`MAX_REPLY_TOKENS`].
    pub fn reply_room(&self) -> u64 {
        let room = match (self.limit.output, self.limit.context) {
            (0, 0) => MAX_REPLY_TOKENS,
            (0, context) => context / 4,
            (output, 0) => output,
            (output, context) => output.min(context / 2),
        };
        room.min(MAX_REPLY_TOKENS)
    }

    /// How many tokens a conversation may use before it compacts: under the input cap when the provider
    /// sets one below the window (keeping a margin for the next step), else the window less the reply room.
    pub fn compaction_point(&self) -> u64 {
        let room = self.reply_room();
        match self.limit.input {
            input if input > 0 && input < self.limit.context => input.saturating_sub(room.min(INPUT_RESERVE)),
            _ => self.limit.context.saturating_sub(room),
        }
    }
}

/// What one reasoning level asks of the provider: an effort its API names, or a thinking token budget.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reasoning {
    Effort { level: String },
    Budget { tokens: u32 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Variant {
    pub name: String,
    #[serde(flatten)]
    pub reasoning: Reasoning,
}

/// A thinking budget never asks for more than this, whatever the model allows.
const MAX_THINKING_BUDGET: u64 = 31_999;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub api: Option<String>,
    pub models: BTreeMap<String, Model>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    pub providers: BTreeMap<String, ProviderInfo>,
}

impl Catalog {
    pub fn bundled() -> Self {
        Self::parse(SNAPSHOT).expect("bundled catalog is valid")
    }

    /// The cheapest priced model from the same provider, for small jobs like titles. Free models
    /// (local providers) keep the conversation's own model rather than loading another.
    pub fn small_model(&self, like: &ModelRef) -> Option<ModelRef> {
        let provider = self.providers.get(&like.provider)?;
        let current = provider.models.get(&like.model)?;
        if current.cost.input + current.cost.output == 0.0 {
            return None;
        }
        let price = |model: &Model| model.cost.input + model.cost.output;
        let chosen = provider
            .models
            .values()
            // Small jobs use a plain model; modes can trade a lower price for slower replies.
            .filter(|model| {
                model.mode.is_none() && model.cost.input > 0.0 && model.limit.context >= SMALL_MODEL_MIN_CONTEXT
            })
            .min_by(|a, b| {
                price(a)
                    .total_cmp(&price(b))
                    .then_with(|| b.release_date.cmp(&a.release_date))
            })?;
        Some(ModelRef {
            provider: like.provider.clone(),
            model: chosen.id.clone(),
        })
    }

    pub fn cache_is_fresh(data_dir: &Path) -> bool {
        std::fs::metadata(cache_path(data_dir))
            .and_then(|meta| meta.modified())
            .is_ok_and(|modified| modified.elapsed().unwrap_or(CACHE_TTL) < CACHE_TTL)
    }

    /// The cached download if it is fresh, else the bundled snapshot.
    pub fn load(data_dir: &Path) -> Self {
        let cache = cache_path(data_dir);
        if Self::cache_is_fresh(data_dir)
            && let Ok(catalog) = std::fs::read_to_string(&cache)
                .map_err(drop)
                .and_then(|text| Self::parse(&text).map_err(drop))
        {
            return catalog;
        }

        Self::bundled()
    }

    pub async fn refresh(client: &reqwest::Client, data_dir: &Path) -> Result<Self, CatalogError> {
        let text = client.get(SOURCE_URL).send().await?.text().await?;
        let catalog = Self::parse(&text)?;
        let json = serde_json::to_string(&catalog.providers)?;

        std::fs::write(cache_path(data_dir), json)?;
        for older in OLDER_CACHES {
            let _ = std::fs::remove_file(data_dir.join(older));
        }

        Ok(catalog)
    }

    fn parse(text: &str) -> Result<Self, CatalogError> {
        let raw: BTreeMap<String, RawProvider> = serde_json::from_str(text)?;
        let providers = raw
            .into_iter()
            .filter(|(id, _)| PROVIDERS.contains(&id.as_str()))
            .map(|(id, provider)| (id.clone(), provider.into_info(&id)))
            .collect();

        Ok(Self { providers })
    }

    pub fn model(&self, provider: &str, model: &str) -> Option<&Model> {
        self.providers.get(provider)?.models.get(model)
    }

    /// Whether two entries run one model, so each takes the other's signed reasoning: the same entry,
    /// or a mode and its base (Claude Opus 5.5 and Claude Opus 5.5 Fast). An entry no longer listed is only itself.
    pub fn same_model(&self, a: &ModelRef, b: &ModelRef) -> bool {
        let wire = |of: &ModelRef| {
            self.model(&of.provider, &of.model)
                .map_or(of.model.clone(), |model| model.wire(&of.model).to_string())
        };

        a.provider == b.provider && wire(a) == wire(b)
    }

    /// The user's providers over models.dev's: a base URL re-points one, listed models join it, and a
    /// new id becomes an OpenAI-compatible route.
    pub fn with_user(mut self, user: &BTreeMap<String, ProviderConfig>) -> Self {
        for (id, config) in user {
            let info = self.providers.entry(id.clone()).or_insert_with(|| ProviderInfo {
                id: id.clone(),
                name: id.clone(),
                env: Vec::new(),
                api: None,
                models: BTreeMap::new(),
            });

            if let Some(name) = &config.name {
                info.name = name.clone();
            }
            if let Some(base) = &config.base_url {
                info.api = Some(base.clone());
            }
            if let Some(env) = &config.api_key_env {
                info.env = vec![env.clone()];
            }

            info.models.extend(
                config
                    .models
                    .iter()
                    .map(|(model, listed)| (model.clone(), user_model(model, listed))),
            );
        }

        self
    }

    /// What a local server reports replaces the provider's listed models: it knows what is installed.
    pub fn with_local(&mut self, id: &str, name: &str, models: &[Model]) {
        let info = self.providers.entry(id.into()).or_insert_with(|| ProviderInfo {
            id: id.into(),
            name: name.into(),
            env: Vec::new(),
            api: None,
            models: BTreeMap::new(),
        });

        info.models = models.iter().map(|model| (model.id.clone(), model.clone())).collect();
    }
}

#[cfg(test)]
mod overlay_tests {
    use super::*;

    #[test]
    fn the_users_providers_repoint_add_and_list_models() {
        let user: BTreeMap<String, ProviderConfig> = serde_json::from_value(serde_json::json!({
            "lmstudio": { "baseUrl": "http://192.168.1.5:1234/v1" },
            "gateway": { "name": "Our gateway", "baseUrl": "https://gw.example/v1", "apiKeyEnv": "GW_KEY", "models": { "big": { "context": 200000, "images": true } } }
        }))
        .unwrap();
        let catalog = Catalog::bundled().with_user(&user);
        assert_eq!(
            catalog.providers["lmstudio"].api.as_deref(),
            Some("http://192.168.1.5:1234/v1")
        );
        let gateway = &catalog.providers["gateway"];
        assert_eq!(
            (gateway.name.as_str(), gateway.env.as_slice()),
            ("Our gateway", ["GW_KEY".to_string()].as_slice())
        );
        assert_eq!(
            (gateway.models["big"].limit.context, gateway.models["big"].attachment),
            (200_000, true)
        );
        assert!(
            matches!(
                crate::llm::provider_for("gateway", gateway.api.as_deref()),
                Some(crate::llm::Provider::Compat(_))
            ),
            "a new id is an OpenAI-compatible route"
        );
        assert!(crate::llm::provider_for("unknown", None).is_none());
        assert!(
            Catalog::bundled()
                .providers
                .values()
                .filter(|p| matches!(p.id.as_str(), "anthropic" | "openai" | "google"))
                .all(|p| p.api.is_none()),
            "models.dev never re-points a native route"
        );
    }
}

/// Builds a user-listed model without inventing reasoning modes or prices the config did not give.
fn user_model(id: &str, listed: &ProviderModel) -> Model {
    Model {
        id: id.into(),
        name: listed.name.clone().unwrap_or_else(|| id.into()),
        family: String::new(),
        reasoning: false,
        attachment: listed.images,
        pdf: false,
        temperature: true,
        release_date: String::new(),
        limit: Limit {
            context: listed.context,
            output: listed.output,
            input: 0,
        },
        cost: Cost::default(),
        profile: ToolProfile::Edit,
        prompt: PromptFamily::Default,
        variants: Vec::new(),
        mode: None,
    }
}

/// Cloud routes host many vendors; only the wires the adapters speak are offered: Claude on Bedrock, Claude and Gemini on Vertex.
fn speaks(provider: &str, model: &str) -> bool {
    match provider {
        "amazon-bedrock" => model.contains("anthropic."),
        "google-vertex" | "google-vertex-anthropic" => model.starts_with("claude") || model.starts_with("gemini"),
        _ => true,
    }
}

fn cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CACHE_FILE)
}

/// The reasoning levels a model offers, from models.dev's `reasoning_options`: decided here and nowhere else.
fn variants_for(provider: &str, model: &str, output: u64, options: &[serde_json::Value]) -> Vec<Variant> {
    let effort = options
        .iter()
        .find(|o| o["type"] == "effort")
        .and_then(|o| o["values"].as_array());
    let budget = options.iter().find(|o| o["type"] == "budget_tokens");
    // Older Claude models accept their listed token budget but reject adaptive effort levels.
    let claude = speaks_claude(provider, model);
    match (effort, budget) {
        (_, Some(budget)) if claude => budget_variants(budget, output),
        (Some(levels), _) => levels.iter().map(effort_variant).collect(),
        (None, Some(budget)) if takes_budget(provider) => budget_variants(budget, output),
        _ => Vec::new(),
    }
}

/// What a turn asks for when the user picked no level, as opencode does: OpenAI reasoning models
/// think at `medium` (their own default) so summaries come back; models without that level ask nothing.
pub fn default_reasoning(provider: &str, model: &Model) -> Option<Reasoning> {
    let medium = model.variants.iter().find(|variant| variant.name == "medium")?;

    (provider == "openai" && model.reasoning).then(|| medium.reasoning.clone())
}

/// OpenAI's GPT reasoning models answer tersely when asked (`text.verbosity`); Codex models are left as they are.
pub fn verbosity(provider: &str, model: &Model) -> Option<&'static str> {
    let gpt = model.family.starts_with("gpt") && !model.family.contains("codex") && !model.family.contains("pro");

    (provider == "openai" && model.reasoning && gpt).then_some("low")
}

/// Sampling a model's makers tune it for, sent when the user set none, as opencode does: Kimi,
/// GLM, MiniMax and Gemini (not Lite). Read from the catalog's family and reasoning flag.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sampling {
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub top_k: Option<u32>,
}

/// Gemini 2.5's first release; earlier Gemini models are tuned differently.
const GEMINI_TUNED_SINCE: &str = "2025-03";

pub fn sampling(model: &Model) -> Sampling {
    let family = model.family.as_str();
    let tuned = |temperature, top_p, top_k| Sampling {
        temperature: Some(temperature),
        top_p,
        top_k,
    };

    match family {
        _ if !model.temperature => Sampling::default(),
        f if f.starts_with("kimi") && model.reasoning => tuned(1.0, Some(0.95), None),
        f if f.starts_with("kimi") => tuned(0.6, None, None),
        f if f.starts_with("glm") => tuned(1.0, None, None),
        f if f.starts_with("minimax") => tuned(1.0, Some(0.95), Some(40)),
        // From the 2.5 generation on, as opencode lists them; 1.5 and 2.0 keep their own defaults.
        f if f.starts_with("gemini") && !f.contains("lite") && model.release_date.as_str() >= GEMINI_TUNED_SINCE => {
            tuned(1.0, Some(0.95), Some(64))
        }
        _ => Sampling::default(),
    }
}

/// Gemini keeps its thinking to itself unless asked, even at its default level.
pub fn shows_thinking(provider: &str, model: &Model) -> bool {
    matches!(provider, "google" | "google-vertex") && model.reasoning && !speaks_claude(provider, &model.id)
}

fn effort_variant(level: &serde_json::Value) -> Variant {
    let level = level.as_str().unwrap_or("none").to_string();
    Variant {
        name: level.clone(),
        reasoning: Reasoning::Effort { level },
    }
}

/// `high` at half the most the model takes, `max` at the most, as opencode offers them.
fn budget_variants(budget: &serde_json::Value, output: u64) -> Vec<Variant> {
    let most = budget["max"]
        .as_u64()
        .unwrap_or(MAX_THINKING_BUDGET)
        .min(output.saturating_sub(1))
        .min(MAX_THINKING_BUDGET);
    if most == 0 {
        return Vec::new();
    }

    let high = budget["min"].as_u64().unwrap_or(0).max(most.div_ceil(2)).min(most);
    [("high", high), ("max", most)]
        .into_iter()
        .map(|(name, tokens)| Variant {
            name: name.into(),
            reasoning: Reasoning::Budget { tokens: tokens as u32 },
        })
        .collect()
}

/// The routes that reach Claude through the Anthropic Messages API.
fn speaks_claude(provider: &str, model: &str) -> bool {
    matches!(provider, "anthropic" | "google-vertex-anthropic" | "amazon-bedrock")
        || (provider == "google-vertex" && model.starts_with("claude"))
}

/// The routes whose wire can carry a thinking token budget.
fn takes_budget(provider: &str) -> bool {
    matches!(
        provider,
        "anthropic" | "google-vertex-anthropic" | "amazon-bedrock" | "google" | "google-vertex" | "openrouter"
    )
}

/// OpenAI trains its GPT-5 generation on the apply_patch format; everything else gets search/replace.
fn profile_for(provider: &str, family: &str) -> ToolProfile {
    match (provider, family) {
        ("openai", family) if family.starts_with("gpt") && !family.starts_with("gpt-4") => ToolProfile::ApplyPatch,
        _ => ToolProfile::Edit,
    }
}

#[cfg(test)]
mod tests;
