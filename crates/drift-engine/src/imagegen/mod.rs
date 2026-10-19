//! Image generation for the `generate_image` tool. One provider at a time, chosen in Settings and paid
//! with the user's own API key for it; a subscription sign-in is never used, since it pays for chat and
//! not for the provider's image API. Each call's price comes from the answer where the provider reports
//! it, else from its token counts and the prices in `providers.rs`, and is added to the conversation.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::Engine;
use crate::event::Event;
use crate::tool::image::Image;

mod providers;
mod size;

pub use providers::PROVIDERS;
pub use size::Size;

pub const SETTINGS_KEY: &str = "imageGeneration";
/// The most reference images one call takes: xAI's limit, the smallest of the three.
pub const MAX_REFERENCES: usize = 5;

/// Settings > Image generation, kept by the engine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct ImageGeneration {
    /// `off`, which removes the tool, or a provider's id: `openai`, `google` or `xai`.
    pub provider: String,
    /// The provider's image model; empty for its default.
    pub model: String,
    /// The size a call that names none gets: `auto`, a ratio such as `16:9`, or `WIDTHxHEIGHT`.
    pub size: String,
}

impl Default for ImageGeneration {
    fn default() -> Self {
        Self {
            provider: "off".into(),
            model: String::new(),
            size: "auto".into(),
        }
    }
}

/// A provider as Settings lists it.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImageProvider {
    pub id: String,
    pub name: String,
    /// Its image models, the default first.
    pub models: Vec<String>,
    /// Whether an API key for it is saved or in the environment.
    pub has_key: bool,
}

/// One image to make.
pub struct ImageRequest<'a> {
    pub prompt: &'a str,
    pub size: Size,
    /// Images to work from.
    pub references: Vec<Image>,
}

/// One image made.
pub struct Generated {
    pub provider: &'static str,
    pub model: String,
    pub image: Image,
    /// In US dollars; `None` when neither the answer nor Drift's prices say.
    pub cost: Option<f64>,
    /// How the provider rewrote the prompt, or what it said beside the image.
    pub revised_prompt: Option<String>,
}

/// Where tests serve the providers instead, by engine data directory.
#[cfg(test)]
pub(crate) static TEST_BASES: std::sync::Mutex<Vec<(std::path::PathBuf, String)>> = std::sync::Mutex::new(Vec::new());

impl Engine {
    pub fn image_settings(&self) -> ImageGeneration {
        self.store.setting(SETTINGS_KEY).ok().flatten().unwrap_or_default()
    }

    /// The user's API key for a provider: a saved one, else its catalog environment variables'.
    fn image_key(&self, provider: &str) -> Option<String> {
        let env = self
            .catalog
            .read()
            .unwrap()
            .providers
            .get(provider)
            .map(|info| info.env.clone())
            .unwrap_or_default();

        self.credentials.api_key(provider, &env)
    }

    pub fn image_providers(&self) -> Vec<ImageProvider> {
        PROVIDERS
            .iter()
            .map(|provider| ImageProvider {
                id: provider.id.into(),
                name: provider.name.into(),
                models: provider.models.iter().map(ToString::to_string).collect(),
                has_key: self.image_key(provider.id).is_some(),
            })
            .collect()
    }

    /// Whether `generate_image` is offered: a provider is chosen and an API key for it is found.
    pub fn image_generation_available(&self) -> bool {
        let settings = self.image_settings();
        providers::find(&settings.provider).is_some_and(|provider| self.image_key(provider.id).is_some())
    }

    /// The chosen provider's name, for asks and results; `None` while off.
    pub fn image_provider_name(&self) -> Option<&'static str> {
        providers::find(&self.image_settings().provider).map(|provider| provider.name)
    }

    pub async fn generate_image(&self, request: ImageRequest<'_>) -> Result<Generated, String> {
        let settings = self.image_settings();
        let provider = providers::find(&settings.provider).ok_or("image generation is off in Settings")?;
        let key = self.image_key(provider.id).ok_or_else(|| {
            format!(
                "there is no {} API key; add one in Settings > Providers (a subscription sign-in cannot make images)",
                provider.name
            )
        })?;
        let model = match settings.model.trim() {
            "" => provider.models[0],
            chosen => chosen,
        };
        let base = self.image_base().map(|base| format!("{base}/{}", provider.id));

        let call = providers::Call {
            http: &self.http,
            key: &key,
            model,
            base: base.as_deref(),
        };

        providers::generate(provider, call, &request).await
    }

    #[cfg(test)]
    fn image_base(&self) -> Option<String> {
        let bases = TEST_BASES.lock().unwrap();
        bases
            .iter()
            .find(|(directory, _)| *directory == self.data_dir)
            .map(|(_, base)| base.clone())
    }

    #[cfg(not(test))]
    fn image_base(&self) -> Option<String> {
        None
    }

    /// Adds `cost` to a reply, so the conversation's total includes what its calls spent.
    pub(crate) fn charge_reply(&self, message_id: &str, cost: f64) {
        if let Ok(Some(message)) = self.store.add_cost(message_id, cost) {
            self.hub.publish(Event::MessageUpdated { message });
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
