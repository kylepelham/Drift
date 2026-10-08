//! Which models exist, what they cost and how they take tools. models.dev filtered to our providers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::config::{ProviderConfig, ProviderModel};
use crate::session::types::ModelRef;

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

/// Prices per million tokens. A long prompt can cost more: `tiers` are models.dev's context tiers and
/// its `context_over_200k`, and the largest one a request's prompt passes prices the whole request.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(from = "RawPrices")]
pub struct Cost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tiers: Vec<CostTier>,
}

/// The prices for a request whose prompt is longer than `above` tokens.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CostTier {
    pub above: u64,
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

impl Cost {
    /// The `(input, output, cache_read, cache_write)` prices for a request with a prompt of `prompt` tokens.
    pub fn at(&self, prompt: u64) -> (f64, f64, f64, f64) {
        let tier = self
            .tiers
            .iter()
            .filter(|tier| prompt > tier.above)
            .max_by_key(|tier| tier.above);
        tier.map_or((self.input, self.output, self.cache_read, self.cache_write), |tier| {
            (tier.input, tier.output, tier.cache_read, tier.cache_write)
        })
    }
}

/// Prices as models.dev gives them (`tiers` of `{ tier: { type, size } }`, `context_over_200k`) or as a cached catalog stores them.
#[derive(Default, Deserialize)]
#[serde(default)]
struct RawPrices {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
    tiers: Vec<RawTier>,
    context_over_200k: Option<RawTier>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawTier {
    above: Option<u64>,
    tier: Option<TierSize>,
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

#[derive(Deserialize)]
struct TierSize {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    size: u64,
}

impl From<RawPrices> for Cost {
    fn from(raw: RawPrices) -> Self {
        let (input, output, cache_read, cache_write) = (raw.input, raw.output, raw.cache_read, raw.cache_write);
        // A price a tier leaves out is the base one.
        let tier = |above: u64, given: RawTier| CostTier {
            above,
            input: given.input.unwrap_or(input),
            output: given.output.unwrap_or(output),
            cache_read: given.cache_read.unwrap_or(cache_read),
            cache_write: given.cache_write.unwrap_or(cache_write),
        };
        let mut tiers: Vec<CostTier> = Vec::new();
        for given in raw.tiers {
            let above = given.above.or(given
                .tier
                .as_ref()
                .filter(|size| size.kind.as_deref().is_none_or(|kind| kind == "context"))
                .map(|size| size.size));
            if let Some(above) = above {
                tiers.push(tier(above, given));
            }
        }
        if let Some(over) = raw
            .context_over_200k
            .filter(|_| !tiers.iter().any(|tier| tier.above == 200_000))
        {
            tiers.push(tier(200_000, over));
        }
        tiers.sort_by_key(|tier| tier.above);
        Self {
            input,
            output,
            cache_read,
            cache_write,
            tiers,
        }
    }
}

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

    /// Room kept for the reply, and the most a request asks for: the model's output limit when known,
    /// else a quarter of its window (a small local model's whole window would otherwise go to a reply
    /// it can never give), never more than half a known window (an output limit as large as the window
    /// would leave the prompt nothing) nor [`MAX_REPLY_TOKENS`].
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
        let price = |m: &Model| m.cost.input + m.cost.output;
        let chosen = provider
            .models
            .values()
            // A mode trades price for speed or speed for price (`flex` is slow); small jobs take the plain model.
            .filter(|m| m.mode.is_none() && m.cost.input > 0.0 && m.limit.context >= SMALL_MODEL_MIN_CONTEXT)
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
            .map(|modified| modified.elapsed().unwrap_or(CACHE_TTL) < CACHE_TTL)
            .unwrap_or(false)
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

    pub async fn refresh(client: &reqwest::Client, data_dir: &Path) -> Result<Self, String> {
        let text = client
            .get(SOURCE_URL)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .text()
            .await
            .map_err(|e| e.to_string())?;
        let catalog = Self::parse(&text)?;
        let json = serde_json::to_string(&catalog.providers).map_err(|e| e.to_string())?;
        std::fs::write(cache_path(data_dir), json).map_err(|e| e.to_string())?;
        for older in OLDER_CACHES {
            let _ = std::fs::remove_file(data_dir.join(older));
        }
        Ok(catalog)
    }

    fn parse(text: &str) -> Result<Self, String> {
        let raw: BTreeMap<String, RawProvider> = serde_json::from_str(text).map_err(|e| e.to_string())?;
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

#[derive(Deserialize)]
struct RawProvider {
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

/// A mode's prices, each one given replacing the base model's.
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

/// Routes whose wire carries a PDF whole, for a model models.dev says takes attachments but gives no modalities for.
const PDF_ROUTES: [&str; 6] = [
    "anthropic",
    "openai",
    "google",
    "google-vertex",
    "google-vertex-anthropic",
    "amazon-bedrock",
];

impl RawProvider {
    fn into_info(self, provider_id: &str) -> ProviderInfo {
        let mut models = BTreeMap::new();
        let listed = self
            .models
            .into_iter()
            .filter(|(_, model)| {
                model.tool_call.unwrap_or(true) && !matches!(model.status.as_deref(), Some("deprecated" | "retired"))
            })
            .filter(|(_, model)| speaks(provider_id, &model.id));
        for (key, mut raw) in listed {
            let modes = raw.experimental.take().unwrap_or_default().modes;
            let base = model_of(raw, provider_id);
            // A model models.dev lists under the same id wins over a mode's entry.
            for (name, mode) in modes {
                models
                    .entry(format!("{key}-{name}"))
                    .or_insert_with(|| moded(&key, &base, &name, mode));
            }
            models.insert(key, base);
        }
        // The native routes' endpoints are ours; only the user's drift.json re-points them.
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

fn model_of(model: RawModel, provider_id: &str) -> Model {
    let family = model.family.unwrap_or_default();
    let profile = model.profile.unwrap_or_else(|| profile_for(provider_id, &family));
    let prompt = model.prompt.unwrap_or_else(|| prompt_for(profile, &family));
    let limit = model.limit.unwrap_or_default();
    let reasoning = model.reasoning.unwrap_or(false);
    let attachment = model.attachment.unwrap_or(false);
    let listed_pdf = model
        .modalities
        .as_ref()
        .map(|m| m.input.iter().any(|kind| kind == "pdf"));
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

/// `base` run in a mode: "Claude Opus 5.5 Fast", at the mode's prices where it gives them.
fn moded(key: &str, base: &Model, name: &str, mode: RawMode) -> Model {
    let mut title = name.chars();
    let title: String = title
        .next()
        .map(|first| first.to_uppercase().chain(title).collect())
        .unwrap_or_default();
    let given = mode.cost.unwrap_or(RawCost {
        input: None,
        output: None,
        cache_read: None,
        cache_write: None,
    });
    // A mode that prices itself has no long-prompt tiers models.dev gives; one that does not keeps the base's.
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

/// The reasoning levels a model offers, from models.dev's `reasoning_options`: decided here and nowhere else.
fn variants_for(provider: &str, model: &str, output: u64, options: &[serde_json::Value]) -> Vec<Variant> {
    let effort = options
        .iter()
        .find(|o| o["type"] == "effort")
        .and_then(|o| o["values"].as_array());
    let budget = options.iter().find(|o| o["type"] == "budget_tokens");
    // On Claude an effort means adaptive thinking, which only the newest accept; a budget works on every one that lists it.
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
mod tests {
    use super::*;

    #[test]
    fn long_prompts_are_priced_at_the_largest_tier_they_pass() {
        let raw = r#"{ "input": 2.5, "output": 15, "cache_read": 0.25, "tiers": [{ "input": 5, "output": 22.5, "cache_read": 0.5, "tier": { "type": "context", "size": 272000 } }], "context_over_200k": { "input": 4, "output": 20 } }"#;
        let cost: Cost = serde_json::from_str(raw).unwrap();
        assert_eq!(
            cost.tiers.iter().map(|tier| tier.above).collect::<Vec<_>>(),
            [200_000, 272_000],
            "models.dev's over-200k becomes a tier"
        );
        assert_eq!(cost.at(150_000), (2.5, 15.0, 0.25, 0.0));
        assert_eq!(
            cost.at(250_000),
            (4.0, 20.0, 0.25, 0.0),
            "a price the tier leaves out is the base one"
        );
        assert_eq!(cost.at(300_000), (5.0, 22.5, 0.5, 0.0));
        let stored: Cost = serde_json::from_str(&serde_json::to_string(&cost).unwrap()).unwrap();
        assert_eq!(stored, cost, "a cached catalog reads back the same tiers");
        let bundled = Catalog::bundled();
        assert!(
            bundled
                .providers
                .values()
                .flat_map(|p| p.models.values())
                .any(|model| !model.cost.tiers.is_empty()),
            "the snapshot carries tiers"
        );
    }

    #[test]
    fn bundled_snapshot_has_anthropic_models_with_costs() {
        let catalog = Catalog::bundled();
        let anthropic = &catalog.providers["anthropic"];
        assert!(anthropic.models.len() > 5);
        let model = anthropic.models.values().next().unwrap();
        assert!(model.limit.context > 0);
        assert!(model.cost.input > 0.0);
        assert_eq!(model.profile, ToolProfile::Edit);
    }

    #[test]
    fn snapshot_only_keeps_our_providers() {
        let catalog = Catalog::bundled();
        for id in catalog.providers.keys() {
            assert!(PROVIDERS.contains(&id.as_str()), "{id}");
        }
    }

    #[test]
    fn cloud_routes_offer_only_what_their_adapters_speak_and_openrouter_is_a_provider() {
        let catalog = Catalog::bundled();
        assert!(!catalog.providers["amazon-bedrock"].models.is_empty());
        assert!(
            catalog.providers["amazon-bedrock"]
                .models
                .values()
                .all(|m| m.id.contains("anthropic."))
        );
        assert!(
            catalog.providers["google-vertex"]
                .models
                .values()
                .all(|m| m.id.starts_with("claude") || m.id.starts_with("gemini"))
        );
        let raw = r#"{"openrouter":{"id":"openrouter","name":"OpenRouter","env":["OPENROUTER_API_KEY"],"api":"https://openrouter.ai/api/v1","models":{"anthropic/claude-sonnet-4.5":{"id":"anthropic/claude-sonnet-4.5","name":"Claude Sonnet 4.5","tool_call":true}}}}"#;
        let parsed = Catalog::parse(raw).unwrap();
        assert!(
            parsed.model("openrouter", "anthropic/claude-sonnet-4.5").is_some(),
            "selectable once the catalog lists it"
        );
    }

    #[test]
    fn unpicked_levels_verbosity_thinking_and_sampling_follow_the_catalog() {
        let catalog = Catalog::bundled();
        let model = |provider: &str, id: &str| catalog.model(provider, id).unwrap().clone();
        let medium = Some(Reasoning::Effort { level: "medium".into() });
        assert_eq!(default_reasoning("openai", &model("openai", "gpt-5.5")), medium);
        assert_eq!(
            default_reasoning("openai", &model("openai", "gpt-5-pro")),
            None,
            "pro offers only high"
        );
        assert_eq!(
            default_reasoning("anthropic", &model("anthropic", "claude-sonnet-4-5")),
            None
        );
        assert_eq!(verbosity("openai", &model("openai", "gpt-5.5")), Some("low"));
        assert_eq!(
            verbosity("openai", &model("openai", "gpt-5.3-codex")),
            None,
            "Codex models are left as they are"
        );
        assert!(shows_thinking("google", &model("google", "gemini-2.5-pro")));
        assert!(!shows_thinking("anthropic", &model("anthropic", "claude-sonnet-4-5")));
        assert_eq!(sampling(&model("zai", "glm-4.6")).temperature, Some(1.0));
        assert_eq!(
            sampling(&model("google", "gemini-3.5-flash")),
            Sampling {
                temperature: Some(1.0),
                top_p: Some(0.95),
                top_k: Some(64)
            }
        );
        assert_eq!(sampling(&model("google", "gemini-3.5-flash-lite")), Sampling::default());
        assert_eq!(sampling(&model("google", "gemini-2.5-pro")).top_k, Some(64));
        let older = Model {
            release_date: "2024-12-11".into(),
            ..model("google", "gemini-2.5-flash")
        };
        assert_eq!(sampling(&older), Sampling::default(), "1.5 and 2.0 keep their own");
        assert_eq!(sampling(&model("anthropic", "claude-sonnet-4-5")), Sampling::default());
    }

    #[test]
    fn each_model_gets_its_family_s_prompt_from_the_catalog_never_its_id() {
        let catalog = Catalog::bundled();
        let family = |provider: &str, model: &str| catalog.model(provider, model).unwrap().prompt;
        assert_eq!(family("openai", "gpt-5.5"), PromptFamily::Codex);
        assert_eq!(family("openai", "gpt-5.3-codex"), PromptFamily::Codex);
        assert_eq!(family("anthropic", "claude-opus-5-5"), PromptFamily::Claude);
        assert_eq!(
            family("anthropic", "claude-opus-5-5-fast"),
            PromptFamily::Claude,
            "a mode keeps its base's prompt"
        );
        assert_eq!(family("google", "gemini-2.5-pro"), PromptFamily::Gemini);
        let bedrock = catalog.providers["amazon-bedrock"].models.values().next().unwrap();
        assert_eq!(
            bedrock.prompt,
            PromptFamily::Claude,
            "Claude on another route is still Claude"
        );
        assert_eq!(
            prompt_for(ToolProfile::Edit, "gpt-4o"),
            PromptFamily::Default,
            "a model that edits with search and replace is not given the apply_patch prompt"
        );
        assert_eq!(prompt_for(ToolProfile::Edit, "grok"), PromptFamily::Default);
    }

    #[test]
    fn openai_gpt_models_use_apply_patch() {
        assert_eq!(profile_for("openai", "gpt"), ToolProfile::ApplyPatch);
        assert_eq!(profile_for("openai", "gpt-4o"), ToolProfile::Edit);
        assert_eq!(profile_for("openai", "o"), ToolProfile::Edit);
        assert_eq!(profile_for("anthropic", "claude-sonnet"), ToolProfile::Edit);
    }

    #[test]
    fn reasoning_variants_come_from_models_dev_and_suit_each_wire() {
        let options = |json: &str| serde_json::from_str::<Vec<serde_json::Value>>(json).unwrap();
        let names = |variants: Vec<Variant>| variants.into_iter().map(|v| v.name).collect::<Vec<_>>();
        let budget = |name: &str, tokens| Variant {
            name: name.into(),
            reasoning: Reasoning::Budget { tokens },
        };
        let effort = options(r#"[{"type":"effort","values":["low","medium","high","xhigh","max"]}]"#);
        assert_eq!(
            names(variants_for("anthropic", "claude-opus-5-5", 128_000, &effort)),
            ["low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(
            variants_for("amazon-bedrock", "anthropic.claude-opus-5-5", 128_000, &effort)[4].reasoning,
            Reasoning::Effort { level: "max".into() }
        );
        let both =
            options(r#"[{"type":"effort","values":["low","medium","high"]},{"type":"budget_tokens","min":1024}]"#);
        assert_eq!(
            variants_for("anthropic", "claude-opus-4-5", 64_000, &both),
            [budget("high", 16_000), budget("max", 31_999)],
            "Claude takes the budget it lists"
        );
        assert_eq!(
            variants_for("anthropic", "claude-haiku", 8_000, &both),
            [budget("high", 4_000), budget("max", 7_999)],
            "within the output limit"
        );
        assert_eq!(
            names(variants_for("openrouter", "z-ai/glm", 64_000, &both)),
            ["low", "medium", "high"],
            "elsewhere the effort wins"
        );
        assert_eq!(
            names(variants_for(
                "openai",
                "gpt-6-sol",
                128_000,
                &options(r#"[{"type":"effort","values":[null,"low","high"]}]"#)
            )),
            ["none", "low", "high"]
        );
        let range = options(r#"[{"type":"budget_tokens","min":128,"max":32768}]"#);
        assert_eq!(
            variants_for("google", "gemini-2.5-pro", 65_536, &range),
            [budget("high", 16_000), budget("max", 31_999)]
        );
        assert!(
            variants_for("xai", "grok", 64_000, &range).is_empty(),
            "a chat completions route has no budget to send"
        );
        assert!(variants_for("lmstudio", "qwen", 64_000, &options(r#"[{"type":"toggle"}]"#)).is_empty());
    }

    #[test]
    fn the_bundled_snapshot_carries_each_models_reasoning_levels() {
        let catalog = Catalog::bundled();
        let names = |provider: &str, model: &str| {
            catalog
                .model(provider, model)
                .unwrap()
                .variants
                .iter()
                .map(|v| v.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names("anthropic", "claude-opus-5-5"),
            ["low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(names("anthropic", "claude-sonnet-4-5"), ["high", "max"]);
        assert!(names("google", "gemini-3.8-flash").contains(&"high".to_string()));
    }

    #[test]
    fn each_models_dev_mode_is_its_own_entry_at_its_own_price_and_survives_the_cache() {
        let raw = r#"{"openai":{"id":"openai","name":"OpenAI","models":{"gpt-6-astra":{"id":"gpt-6-astra","name":"GPT-6 Astra","family":"gpt","release_date":"2026-08-01","cost":{"input":5,"output":25,"cache_read":0.5},
            "experimental":{"modes":{"ultrafast":{"cost":{"input":60,"output":300},"provider":{"body":{"service_tier":"ultrafast"}}},"fast":{"provider":{"body":{"service_tier":"priority"}}}}}}}}}"#;
        let catalog = Catalog::parse(raw).unwrap();
        let ultrafast = catalog.model("openai", "gpt-6-astra-ultrafast").unwrap();
        assert_eq!(
            (ultrafast.id.as_str(), ultrafast.name.as_str()),
            ("gpt-6-astra-ultrafast", "GPT-6 Astra Ultrafast")
        );
        assert_eq!(
            (ultrafast.cost.input, ultrafast.cost.output, ultrafast.cost.cache_read),
            (60.0, 300.0, 0.5),
            "a price the mode leaves out is the base model's"
        );
        assert_eq!(ultrafast.wire("gpt-6-astra-ultrafast"), "gpt-6-astra");
        assert_eq!(ultrafast.mode.as_ref().unwrap().body["service_tier"], "ultrafast");
        assert_eq!(
            (ultrafast.profile, ultrafast.release_date.as_str()),
            (ToolProfile::ApplyPatch, "2026-08-01"),
            "everything else is the base model's"
        );
        assert_eq!(catalog.model("openai", "gpt-6-astra-fast").unwrap().cost.input, 5.0);
        let base = catalog.model("openai", "gpt-6-astra").unwrap();
        assert!(base.mode.is_none() && base.wire("gpt-6-astra") == "gpt-6-astra");
        let cached = Catalog::parse(&serde_json::to_string(&catalog.providers).unwrap()).unwrap();
        assert_eq!(cached, catalog, "the cache stores the entries already made");
    }

    #[test]
    fn small_jobs_never_take_a_mode_and_the_snapshot_lists_fast_and_ultrafast() {
        let catalog = Catalog::bundled();
        assert_eq!(
            catalog.model("anthropic", "claude-opus-5-5-fast").unwrap().name,
            "Claude Opus 5.5 Fast"
        );
        assert!(catalog.model("openai", "gpt-6-astra-ultrafast").is_some());
        assert!(catalog.model("openai", "gpt-5.5-fast").is_some());
        let small = catalog
            .small_model(&ModelRef {
                provider: "openai".into(),
                model: "gpt-5.5".into(),
            })
            .unwrap();
        assert!(
            catalog.model("openai", &small.model).unwrap().mode.is_none(),
            "{small:?} is a mode; flex is slow"
        );
        let model = |provider: &str, model: &str| ModelRef {
            provider: provider.into(),
            model: model.into(),
        };
        assert!(
            catalog.same_model(
                &model("anthropic", "claude-opus-5-5"),
                &model("anthropic", "claude-opus-5-5-fast")
            ),
            "a mode runs its base model"
        );
        assert!(
            !catalog.same_model(
                &model("anthropic", "claude-opus-5-5"),
                &model("anthropic", "claude-opus-5")
            ),
            "a sibling in the family does not"
        );
        assert!(!catalog.same_model(
            &model("anthropic", "claude-opus-5-5"),
            &model("amazon-bedrock", "claude-opus-5-5")
        ));
        assert!(
            catalog.same_model(&model("anthropic", "retired"), &model("anthropic", "retired"))
                && !catalog.same_model(&model("anthropic", "retired"), &model("anthropic", "gone"))
        );
    }

    #[test]
    fn cache_is_reused_when_fresh_and_round_trips() {
        let dir = std::env::temp_dir().join(format!("drift-catalog-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&dir).unwrap();
        let mut catalog = Catalog::bundled();
        catalog.providers.retain(|id, _| id == "anthropic");
        std::fs::write(cache_path(&dir), serde_json::to_string(&catalog.providers).unwrap()).unwrap();
        let loaded = Catalog::load(&dir);
        assert_eq!(loaded.providers.len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }
}
