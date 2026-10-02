//! Which models exist, what they cost and how they take tools. models.dev filtered to our providers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::config::{ProviderConfig, ProviderModel};
use crate::session::types::ModelRef;

const SNAPSHOT: &str = include_str!("../../data/models.json");
const SOURCE_URL: &str = "https://models.dev/api.json";
const CACHE_FILE: &str = "models.json";
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Limit {
    pub context: u64,
    pub output: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Cost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
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
    /// The reasoning levels the model offers, weakest first; empty when it has none to choose.
    #[serde(default)]
    pub variants: Vec<Variant>,
}

fn default_profile() -> ToolProfile {
    ToolProfile::Edit
}

/// The longest reply asked for, whatever the model allows.
pub const MAX_REPLY_TOKENS: u64 = 32_000;
/// Below this window the system prompt and tool schemas leave little room for any work.
pub const SMALL_CONTEXT: u64 = 16_384;

impl Model {
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
            .filter(|m| m.cost.input > 0.0 && m.limit.context >= SMALL_MODEL_MIN_CONTEXT)
            .min_by(|a, b| price(a).total_cmp(&price(b)).then_with(|| b.release_date.cmp(&a.release_date)))?;
        Some(ModelRef { provider: like.provider.clone(), model: chosen.id.clone() })
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
        if Self::cache_is_fresh(data_dir) {
            if let Ok(catalog) = std::fs::read_to_string(&cache).map_err(drop).and_then(|text| Self::parse(&text).map_err(drop)) {
                return catalog;
            }
        }
        Self::bundled()
    }

    pub async fn refresh(client: &reqwest::Client, data_dir: &Path) -> Result<Self, String> {
        let text = client.get(SOURCE_URL).send().await.map_err(|e| e.to_string())?.text().await.map_err(|e| e.to_string())?;
        let catalog = Self::parse(&text)?;
        let json = serde_json::to_string(&catalog.providers).map_err(|e| e.to_string())?;
        std::fs::write(cache_path(data_dir), json).map_err(|e| e.to_string())?;
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

    /// The user's providers over models.dev's: a base URL re-points one, listed models join it, and a
    /// new id becomes an OpenAI-compatible route.
    pub fn with_user(mut self, user: &BTreeMap<String, ProviderConfig>) -> Self {
        for (id, config) in user {
            let info = self.providers.entry(id.clone()).or_insert_with(|| ProviderInfo { id: id.clone(), name: id.clone(), env: Vec::new(), api: None, models: BTreeMap::new() });
            if let Some(name) = &config.name {
                info.name = name.clone();
            }
            if let Some(base) = &config.base_url {
                info.api = Some(base.clone());
            }
            if let Some(env) = &config.api_key_env {
                info.env = vec![env.clone()];
            }
            info.models.extend(config.models.iter().map(|(model, listed)| (model.clone(), user_model(model, listed))));
        }
        self
    }

    /// What a local server reports replaces the provider's listed models: it knows what is installed.
    pub fn with_local(&mut self, id: &str, name: &str, models: &[Model]) {
        let info = self.providers.entry(id.into()).or_insert_with(|| ProviderInfo { id: id.into(), name: name.into(), env: Vec::new(), api: None, models: BTreeMap::new() });
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
        assert_eq!(catalog.providers["lmstudio"].api.as_deref(), Some("http://192.168.1.5:1234/v1"));
        let gateway = &catalog.providers["gateway"];
        assert_eq!((gateway.name.as_str(), gateway.env.as_slice()), ("Our gateway", ["GW_KEY".to_string()].as_slice()));
        assert_eq!((gateway.models["big"].limit.context, gateway.models["big"].attachment), (200_000, true));
        assert!(matches!(crate::llm::provider_for("gateway", gateway.api.as_deref()), Some(crate::llm::Provider::Compat(_))), "a new id is an OpenAI-compatible route");
        assert!(crate::llm::provider_for("unknown", None).is_none());
        assert!(Catalog::bundled().providers.values().filter(|p| matches!(p.id.as_str(), "anthropic" | "openai" | "google")).all(|p| p.api.is_none()), "models.dev never re-points a native route");
    }
}

fn user_model(id: &str, listed: &ProviderModel) -> Model {
    Model {
        id: id.into(),
        name: listed.name.clone().unwrap_or_else(|| id.into()),
        family: String::new(),
        reasoning: false,
        attachment: listed.images,
        temperature: true,
        release_date: String::new(),
        limit: Limit { context: listed.context, output: listed.output },
        cost: Cost::default(),
        profile: ToolProfile::Edit,
        variants: Vec::new(),
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
    /// models.dev's description of how the model's reasoning is set.
    #[serde(default)]
    reasoning_options: Option<Vec<serde_json::Value>>,
    /// Already derived, as a cached catalog stores it.
    #[serde(default)]
    variants: Option<Vec<Variant>>,
}

impl RawProvider {
    fn into_info(self, provider_id: &str) -> ProviderInfo {
        let models = self
            .models
            .into_iter()
            .filter(|(_, model)| model.tool_call.unwrap_or(true) && !matches!(model.status.as_deref(), Some("deprecated" | "retired")))
            .filter(|(_, model)| speaks(provider_id, &model.id))
            .map(|(key, model)| {
                let family = model.family.unwrap_or_default();
                let profile = model.profile.unwrap_or_else(|| profile_for(provider_id, &family));
                let limit = model.limit.unwrap_or_default();
                let reasoning = model.reasoning.unwrap_or(false);
                let variants = match model.variants {
                    Some(variants) => variants,
                    None if reasoning => variants_for(provider_id, &model.id, limit.output, model.reasoning_options.as_deref().unwrap_or_default()),
                    None => Vec::new(),
                };
                (
                    key,
                    Model {
                        id: model.id,
                        name: model.name,
                        family,
                        reasoning,
                        attachment: model.attachment.unwrap_or(false),
                        temperature: model.temperature.unwrap_or(false),
                        release_date: model.release_date.unwrap_or_default(),
                        limit,
                        cost: model.cost.unwrap_or_default(),
                        profile,
                        variants,
                    },
                )
            })
            .collect();
        // The native routes' endpoints are ours; only the user's drift.json re-points them.
        let api = self.api.filter(|_| !matches!(provider_id, "anthropic" | "openai" | "google" | "amazon-bedrock" | "google-vertex" | "google-vertex-anthropic"));
        ProviderInfo { id: self.id, name: self.name, env: self.env.unwrap_or_default(), api, models }
    }
}

/// The reasoning levels a model offers, from models.dev's `reasoning_options`: decided here and nowhere else.
fn variants_for(provider: &str, model: &str, output: u64, options: &[serde_json::Value]) -> Vec<Variant> {
    let effort = options.iter().find(|o| o["type"] == "effort").and_then(|o| o["values"].as_array());
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

fn effort_variant(level: &serde_json::Value) -> Variant {
    let level = level.as_str().unwrap_or("none").to_string();
    Variant { name: level.clone(), reasoning: Reasoning::Effort { level } }
}

/// `high` at half the most the model takes, `max` at the most, as opencode offers them.
fn budget_variants(budget: &serde_json::Value, output: u64) -> Vec<Variant> {
    let most = budget["max"].as_u64().unwrap_or(MAX_THINKING_BUDGET).min(output.saturating_sub(1)).min(MAX_THINKING_BUDGET);
    if most == 0 {
        return Vec::new();
    }
    let high = budget["min"].as_u64().unwrap_or(0).max(most.div_ceil(2)).min(most);
    [("high", high), ("max", most)].into_iter().map(|(name, tokens)| Variant { name: name.into(), reasoning: Reasoning::Budget { tokens: tokens as u32 } }).collect()
}

/// The routes that reach Claude through the Anthropic Messages API.
fn speaks_claude(provider: &str, model: &str) -> bool {
    matches!(provider, "anthropic" | "google-vertex-anthropic" | "amazon-bedrock") || (provider == "google-vertex" && model.starts_with("claude"))
}

/// The routes whose wire can carry a thinking token budget.
fn takes_budget(provider: &str) -> bool {
    matches!(provider, "anthropic" | "google-vertex-anthropic" | "amazon-bedrock" | "google" | "google-vertex" | "openrouter")
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
        assert!(catalog.providers["amazon-bedrock"].models.values().all(|m| m.id.contains("anthropic.")));
        assert!(catalog.providers["google-vertex"].models.values().all(|m| m.id.starts_with("claude") || m.id.starts_with("gemini")));
        let raw = r#"{"openrouter":{"id":"openrouter","name":"OpenRouter","env":["OPENROUTER_API_KEY"],"api":"https://openrouter.ai/api/v1","models":{"anthropic/claude-sonnet-4.5":{"id":"anthropic/claude-sonnet-4.5","name":"Claude Sonnet 4.5","tool_call":true}}}}"#;
        let parsed = Catalog::parse(raw).unwrap();
        assert!(parsed.model("openrouter", "anthropic/claude-sonnet-4.5").is_some(), "selectable once the catalog lists it");
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
        let budget = |name: &str, tokens| Variant { name: name.into(), reasoning: Reasoning::Budget { tokens } };
        let effort = options(r#"[{"type":"effort","values":["low","medium","high","xhigh","max"]}]"#);
        assert_eq!(names(variants_for("anthropic", "claude-opus-5-5", 128_000, &effort)), ["low", "medium", "high", "xhigh", "max"]);
        assert_eq!(variants_for("amazon-bedrock", "anthropic.claude-opus-5-5", 128_000, &effort)[4].reasoning, Reasoning::Effort { level: "max".into() });
        let both = options(r#"[{"type":"effort","values":["low","medium","high"]},{"type":"budget_tokens","min":1024}]"#);
        assert_eq!(variants_for("anthropic", "claude-opus-4-5", 64_000, &both), [budget("high", 16_000), budget("max", 31_999)], "Claude takes the budget it lists");
        assert_eq!(variants_for("anthropic", "claude-haiku", 8_000, &both), [budget("high", 4_000), budget("max", 7_999)], "within the output limit");
        assert_eq!(names(variants_for("openrouter", "z-ai/glm", 64_000, &both)), ["low", "medium", "high"], "elsewhere the effort wins");
        assert_eq!(names(variants_for("openai", "gpt-6-sol", 128_000, &options(r#"[{"type":"effort","values":[null,"low","high"]}]"#))), ["none", "low", "high"]);
        let range = options(r#"[{"type":"budget_tokens","min":128,"max":32768}]"#);
        assert_eq!(variants_for("google", "gemini-2.5-pro", 65_536, &range), [budget("high", 16_000), budget("max", 31_999)]);
        assert!(variants_for("xai", "grok", 64_000, &range).is_empty(), "a chat completions route has no budget to send");
        assert!(variants_for("lmstudio", "qwen", 64_000, &options(r#"[{"type":"toggle"}]"#)).is_empty());
    }

    #[test]
    fn the_bundled_snapshot_carries_each_models_reasoning_levels() {
        let catalog = Catalog::bundled();
        let names = |provider: &str, model: &str| catalog.model(provider, model).unwrap().variants.iter().map(|v| v.name.clone()).collect::<Vec<_>>();
        assert_eq!(names("anthropic", "claude-opus-5-5"), ["low", "medium", "high", "xhigh", "max"]);
        assert_eq!(names("anthropic", "claude-sonnet-4-5"), ["high", "max"]);
        assert!(names("google", "gemini-3.8-flash").contains(&"high".to_string()));
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
