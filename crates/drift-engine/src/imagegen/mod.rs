//! Image generation for the `generate_image` tool. One provider at a time, chosen in Settings.
//!
//! OpenAI's images can be paid two ways: a ChatGPT sign-in's plan allowance, through the Codex backend as
//! Codex itself does, or an API key. Settings picks which goes first; the other takes over when the first is
//! refused or spent. Other cloud providers take an API key, local servers none. A call's price comes from
//! the answer where the provider reports it, else from Drift's prices in `providers/`, and is added to the
//! conversation; images from the plan or a local server cost the conversation nothing.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::Engine;
use crate::event::Event;
use crate::llm::Credential;
use crate::tool::image::Image;

mod providers;
mod size;

pub use providers::PROVIDERS;
use providers::{Access, Auth, Call, Provider};
pub use size::Size;

pub const SETTINGS_KEY: &str = "imageGeneration";
/// The most reference images one call takes: xAI's and Codex's limit, the smallest of the cloud providers'.
pub const MAX_REFERENCES: usize = 5;

/// Which pays first for OpenAI images when both can.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum Payment {
    /// The ChatGPT sign-in's plan allowance.
    #[default]
    Plan,
    ApiKey,
}

/// Settings > Image generation, kept by the engine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct ImageGeneration {
    /// `off`, which removes the tool, or a provider's id.
    pub provider: String,
    /// The provider's image model; empty for its default.
    pub model: String,
    /// The size a call that names none gets: `auto`, a ratio such as `16:9`, or `WIDTHxHEIGHT`.
    pub size: String,
    /// For OpenAI with both a ChatGPT sign-in and an API key: which pays first.
    pub openai_first: Payment,
    /// Each server provider's address, by id; one left out uses its default.
    pub servers: BTreeMap<String, String>,
}

impl Default for ImageGeneration {
    fn default() -> Self {
        Self {
            provider: "off".into(),
            model: String::new(),
            size: "auto".into(),
            openai_first: Payment::Plan,
            servers: BTreeMap::new(),
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
    /// The model is typed rather than picked.
    pub typed: bool,
    /// `catalog` (a key under Settings > Providers, or one saved here for images), `key` (saved here) or `server`.
    pub access: String,
    /// The most reference images one call takes.
    pub references: usize,
    /// Whether an API key for it is found.
    pub has_key: bool,
    /// The key saved here, by its last four characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Whether its key comes from the environment instead.
    pub from_environment: bool,
    /// OpenAI only: whether a ChatGPT sign-in can pay for its images.
    pub has_plan: bool,
    /// A server provider's address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// Whether it could make an image now.
    pub ready: bool,
}

/// One image to make.
pub struct ImageRequest<'a> {
    pub prompt: &'a str,
    pub size: Size,
    /// Images to work from.
    pub references: Vec<Image>,
}

/// What paid for an image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paid {
    Billed,
    Plan,
    /// The user's own server, which bills nothing.
    Local,
}

/// One image made.
#[derive(Debug)]
pub struct Generated {
    pub provider: &'static str,
    pub model: String,
    pub image: Image,
    /// In US dollars; `None` when neither the answer nor Drift's prices say, or nothing was billed.
    pub cost: Option<f64>,
    pub paid: Paid,
    /// How the provider rewrote the prompt, or what it said beside the image.
    pub revised_prompt: Option<String>,
    /// Why the payer that went first did not pay, when another did.
    pub handed_over: Option<String>,
}

/// Who pays for one attempt.
enum Payer {
    Key(String),
    Plan(Credential),
    Server { url: String, key: Option<String> },
}

impl Payer {
    fn auth(&self) -> Auth<'_> {
        match self {
            Self::Key(key) => Auth::Key(key),
            Self::Plan(credential) => Auth::Plan(credential),
            Self::Server { key, .. } => Auth::Server(key.as_deref()),
        }
    }

    fn paid(&self) -> Paid {
        match self {
            Self::Key(_) | Self::Server { key: Some(_), .. } => Paid::Billed,
            Self::Plan(_) => Paid::Plan,
            Self::Server { key: None, .. } => Paid::Local,
        }
    }

    fn named(&self) -> &'static str {
        match self {
            Self::Plan(_) => "your ChatGPT plan",
            _ => "the API key",
        }
    }
}

fn secret_key(provider: &str) -> String {
    format!("imagegen:{provider}")
}

/// A key as Settings shows it: only its last four characters.
fn last_four(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    let tail: String = chars[chars.len().saturating_sub(4)..].iter().collect();

    format!("...{tail}")
}

/// Where tests serve the providers instead, by engine data directory.
#[cfg(test)]
pub(crate) static TEST_BASES: std::sync::Mutex<Vec<(std::path::PathBuf, String)>> = std::sync::Mutex::new(Vec::new());

impl Engine {
    pub fn image_settings(&self) -> ImageGeneration {
        self.store.setting(SETTINGS_KEY).ok().flatten().unwrap_or_default()
    }

    /// The API key saved for a catalog provider in Settings > Providers, else its environment variables'.
    fn catalog_key(&self, provider: &str) -> Option<String> {
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

    fn saved_image_key(&self, provider: &str) -> Option<String> {
        self.credentials
            .secret(&secret_key(provider))
            .filter(|key| !key.trim().is_empty())
    }

    fn environment_key(env: &str) -> Option<String> {
        std::env::var(env).ok().filter(|key| !key.trim().is_empty())
    }

    /// The key a provider's images are paid with: one saved for images here first, since Settings > Providers
    /// holds a sign-in or a key for a provider but not both.
    fn image_key(&self, provider: &Provider) -> Option<String> {
        match provider.access {
            Access::Catalog => self
                .saved_image_key(provider.id)
                .or_else(|| self.catalog_key(provider.id)),
            Access::Key { env } => self.saved_image_key(provider.id).or_else(|| Self::environment_key(env)),
            Access::Server { key, .. } => self.saved_image_key(provider.id).filter(|_| key),
        }
    }

    /// Whether a ChatGPT sign-in is linked, which can pay for OpenAI images.
    fn has_plan(&self) -> bool {
        self.credentials
            .accounts("openai")
            .iter()
            .any(|account| matches!(self.credentials.account(&account.key), Some(Credential::OAuth { .. })))
    }

    fn server_address(&self, provider: &Provider, settings: &ImageGeneration) -> String {
        let default = match provider.access {
            Access::Server { default, .. } => default,
            _ => "",
        };
        settings
            .servers
            .get(provider.id)
            .map(|address| address.trim())
            .filter(|address| !address.is_empty())
            .unwrap_or(default)
            .trim_end_matches('/')
            .to_string()
    }

    fn image_ready(&self, provider: &Provider) -> bool {
        match provider.access {
            Access::Server { .. } => true,
            _ => self.image_key(provider).is_some() || (provider.id == "openai" && self.has_plan()),
        }
    }

    fn listed(&self, provider: &Provider, settings: &ImageGeneration) -> ImageProvider {
        let (access, from_environment, server) = match provider.access {
            Access::Catalog => ("catalog", false, None),
            Access::Key { env } => (
                "key",
                self.saved_image_key(provider.id).is_none() && Self::environment_key(env).is_some(),
                None,
            ),
            Access::Server { .. } => ("server", false, Some(self.server_address(provider, settings))),
        };

        ImageProvider {
            id: provider.id.into(),
            name: provider.name.into(),
            models: provider.models.iter().map(ToString::to_string).collect(),
            typed: provider.typed,
            access: access.into(),
            references: provider.references,
            has_key: self.image_key(provider).is_some(),
            key: self.saved_image_key(provider.id).map(|key| last_four(&key)),
            from_environment,
            has_plan: provider.id == "openai" && self.has_plan(),
            server,
            ready: self.image_ready(provider),
        }
    }

    pub fn image_providers(&self) -> Vec<ImageProvider> {
        let settings = self.image_settings();
        PROVIDERS
            .iter()
            .map(|provider| self.listed(provider, &settings))
            .collect()
    }

    /// Whether `generate_image` is offered: a provider is chosen and something can pay for or run it.
    pub fn image_generation_available(&self) -> bool {
        providers::find(&self.image_settings().provider).is_some_and(|provider| self.image_ready(provider))
    }

    /// Who will make the next image, for its ask: the provider, and the plan, a bill or a server.
    pub fn image_ask_label(&self) -> Option<String> {
        let settings = self.image_settings();
        let provider = providers::find(&settings.provider)?;

        Some(match provider.access {
            Access::Server { .. } => format!("{} at {}", provider.name, self.server_address(provider, &settings)),
            _ if self.plan_goes_first(provider, &settings) => format!("{} on your ChatGPT plan", provider.name),
            _ => format!("{} (billed)", provider.name),
        })
    }

    fn plan_goes_first(&self, provider: &Provider, settings: &ImageGeneration) -> bool {
        let chosen = settings.openai_first == Payment::Plan || self.image_key(provider).is_none();
        provider.id == "openai" && self.has_plan() && chosen
    }

    /// Saves the key a provider's images are paid with; an empty key removes it.
    pub fn set_image_key(&self, provider: &str, key: &str) -> Result<(), String> {
        let found = providers::find(provider).ok_or_else(|| format!("{provider} is not an image provider"))?;
        if let Access::Server { key: false, .. } = found.access {
            return Err(format!("{} takes no key here", found.name));
        }
        let saved = match key.trim() {
            "" => self.credentials.remove_secret(&secret_key(provider)),
            key => self.credentials.set_secret(&secret_key(provider), key),
        };

        saved.map_err(|error| error.to_string())
    }

    /// The ChatGPT sign-in's credential, renewed first when it has expired.
    async fn plan_credential(&self) -> Option<Credential> {
        self.account_credentials("openai")
            .await
            .into_iter()
            .find_map(|(_, credential)| credential.filter(|found| matches!(found, Credential::OAuth { .. })))
    }

    /// Who pays, in the order they are tried.
    async fn payers(&self, provider: &Provider, settings: &ImageGeneration) -> Vec<Payer> {
        if let Access::Server { .. } = provider.access {
            let url = self.server_address(provider, settings);
            return vec![Payer::Server {
                url,
                key: self.image_key(provider),
            }];
        }
        let key = self.image_key(provider).map(Payer::Key);
        if provider.id != "openai" {
            return key.into_iter().collect();
        }

        let plan = self.plan_credential().await.map(Payer::Plan);
        let ordered = match settings.openai_first {
            Payment::Plan => [plan, key],
            Payment::ApiKey => [key, plan],
        };
        ordered.into_iter().flatten().collect()
    }

    fn missing_payer(provider: &Provider) -> String {
        match provider.access {
            Access::Catalog if provider.id == "openai" => {
                "there is no OpenAI API key or ChatGPT sign-in; sign in under Settings > Providers or add a key under Settings > Execution > Image generation".into()
            }
            Access::Catalog => format!(
                "there is no {} API key (a subscription sign-in cannot make its images); add one under Settings > Providers or Settings > Execution > Image generation",
                provider.name
            ),
            _ => format!(
                "there is no {} API key; add one under Settings > Execution > Image generation",
                provider.name
            ),
        }
    }

    pub async fn generate_image(&self, request: ImageRequest<'_>) -> Result<Generated, String> {
        let settings = self.image_settings();
        let provider = providers::find(&settings.provider).ok_or("image generation is off in Settings")?;
        let payers = self.payers(provider, &settings).await;
        if payers.is_empty() {
            return Err(Self::missing_payer(provider));
        }
        let model = match settings.model.trim() {
            "" => provider.models.first().copied().unwrap_or_default(),
            chosen => chosen,
        };

        let mut handed_over = None;
        for (index, payer) in payers.iter().enumerate() {
            let call = Call {
                http: &self.http,
                auth: payer.auth(),
                model,
                root: self.image_root(provider, payer),
            };
            match providers::generate(provider, &call, &request).await {
                Ok(made) => {
                    return Ok(Generated {
                        provider: provider.name,
                        model: model.to_string(),
                        image: made.image,
                        cost: made.cost,
                        paid: payer.paid(),
                        revised_prompt: made.revised_prompt.filter(|prompt| !prompt.trim().is_empty()),
                        handed_over,
                    });
                }
                Err(failure) if failure.moves_on() && index + 1 < payers.len() => {
                    let next = payers[index + 1].named();
                    handed_over = Some(format!(
                        "{} was refused ({failure}), so {next} paid instead",
                        payer.named()
                    ));
                }
                Err(failure) => return Err(format!("{} could not make the image: {failure}", provider.name)),
            }
        }

        Err(Self::missing_payer(provider))
    }

    /// Where a call goes instead of the provider's own host: a server's address, or the tests' stand-in.
    fn image_root(&self, provider: &Provider, payer: &Payer) -> Option<String> {
        #[cfg(test)]
        if let Some(base) = self.image_test_base() {
            let path = if matches!(payer, Payer::Plan(_)) {
                "chatgpt"
            } else {
                provider.id
            };
            return Some(format!("{base}/{path}"));
        }
        let _ = provider;

        match payer {
            Payer::Server { url, .. } => Some(url.clone()),
            _ => None,
        }
    }

    #[cfg(test)]
    fn image_test_base(&self) -> Option<String> {
        let bases = TEST_BASES.lock().unwrap();
        bases
            .iter()
            .find(|(directory, _)| *directory == self.data_dir)
            .map(|(_, base)| base.clone())
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
