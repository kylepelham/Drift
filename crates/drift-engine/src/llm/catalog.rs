//! Which models exist, what they cost and how they take tools. models.dev filtered to our providers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

const SNAPSHOT: &str = include_str!("../../data/models.json");
const SOURCE_URL: &str = "https://models.dev/api.json";
const CACHE_FILE: &str = "models.json";
const CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
pub const PROVIDERS: [&str; 10] = [
    "anthropic",
    "openai",
    "google",
    "xai",
    "zai",
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
}

fn default_profile() -> ToolProfile {
    ToolProfile::Edit
}

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
}

impl RawProvider {
    fn into_info(self, provider_id: &str) -> ProviderInfo {
        let models = self
            .models
            .into_iter()
            .filter(|(_, model)| model.tool_call.unwrap_or(true) && !matches!(model.status.as_deref(), Some("deprecated" | "retired")))
            .map(|(key, model)| {
                let family = model.family.unwrap_or_default();
                let profile = model.profile.unwrap_or_else(|| profile_for(provider_id, &family));
                (
                    key,
                    Model {
                        id: model.id,
                        name: model.name,
                        family,
                        reasoning: model.reasoning.unwrap_or(false),
                        attachment: model.attachment.unwrap_or(false),
                        temperature: model.temperature.unwrap_or(false),
                        release_date: model.release_date.unwrap_or_default(),
                        limit: model.limit.unwrap_or_default(),
                        cost: model.cost.unwrap_or_default(),
                        profile,
                    },
                )
            })
            .collect();
        ProviderInfo { id: self.id, name: self.name, env: self.env.unwrap_or_default(), api: self.api, models }
    }
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
    fn openai_gpt_models_use_apply_patch() {
        assert_eq!(profile_for("openai", "gpt"), ToolProfile::ApplyPatch);
        assert_eq!(profile_for("openai", "gpt-4o"), ToolProfile::Edit);
        assert_eq!(profile_for("openai", "o"), ToolProfile::Edit);
        assert_eq!(profile_for("anthropic", "claude-sonnet"), ToolProfile::Edit);
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
