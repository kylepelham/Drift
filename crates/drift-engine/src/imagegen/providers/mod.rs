//! Every image API's wire shape, one module each, and what they share: who is reached and how, the
//! request's credential, and reading an image out of an answer. Prices are US dollars, from each
//! provider's pricing page on 2026-10-19.

use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::Value;

use super::ImageRequest;
use crate::llm::Credential;
use crate::tool::image::Image;

mod comfyui;
mod fal;
mod flux;
mod google;
mod ideogram;
mod openai;
mod recraft;
mod sdwebui;
mod stability;
mod xai;

/// How Drift reaches a provider and what pays for its images.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Access {
    /// The API key saved for this catalog provider in Settings > Providers, or its environment variable's.
    Catalog,
    /// A key saved for image generation in Settings, or this environment variable's.
    Key { env: &'static str },
    /// A server the user runs, at `default` unless Settings names another; `key` when it may take one.
    Server { default: &'static str, key: bool },
}

pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    /// Its image models, the default first.
    pub models: &'static [&'static str],
    /// The model is typed rather than picked: a gateway's endpoint or a local checkpoint.
    pub typed: bool,
    pub access: Access,
    /// The most reference images one call takes; 0 for text only.
    pub references: usize,
}

const fn provider(id: &'static str, name: &'static str, models: &'static [&'static str], access: Access) -> Provider {
    Provider {
        id,
        name,
        models,
        typed: false,
        access,
        references: super::MAX_REFERENCES,
    }
}

pub const PROVIDERS: &[Provider] = &[
    provider("openai", "OpenAI", openai::MODELS, Access::Catalog),
    provider("google", "Google", google::MODELS, Access::Catalog),
    provider("xai", "xAI", xai::MODELS, Access::Catalog),
    Provider {
        references: 1,
        ..provider(
            "recraft",
            "Recraft",
            recraft::MODELS,
            Access::Key {
                env: "RECRAFT_API_TOKEN",
            },
        )
    },
    provider(
        "bfl",
        "Black Forest Labs (FLUX)",
        flux::MODELS,
        Access::Key { env: "BFL_API_KEY" },
    ),
    Provider {
        references: 1,
        ..provider(
            "stability",
            "Stability AI",
            stability::MODELS,
            Access::Key {
                env: "STABILITY_API_KEY",
            },
        )
    },
    Provider {
        references: 1,
        ..provider(
            "ideogram",
            "Ideogram",
            ideogram::MODELS,
            Access::Key {
                env: "IDEOGRAM_API_KEY",
            },
        )
    },
    Provider {
        typed: true,
        ..provider("fal", "fal.ai", fal::MODELS, Access::Key { env: "FAL_KEY" })
    },
    Provider {
        typed: true,
        references: 1,
        ..provider(
            "comfyui",
            "ComfyUI",
            &[],
            Access::Server {
                default: "http://127.0.0.1:8188",
                key: false,
            },
        )
    },
    Provider {
        typed: true,
        references: 1,
        ..provider(
            "sdwebui",
            "Stable Diffusion WebUI (A1111, Forge)",
            &[],
            Access::Server {
                default: "http://127.0.0.1:7860",
                key: false,
            },
        )
    },
    Provider {
        typed: true,
        ..provider(
            "compatible",
            "OpenAI-compatible server",
            &[],
            Access::Server {
                default: "http://127.0.0.1:8080/v1",
                key: true,
            },
        )
    },
];

pub(super) fn find(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|provider| provider.id == id)
}

/// What a request carries to prove who pays.
pub(super) enum Auth<'a> {
    Key(&'a str),
    /// A ChatGPT sign-in, which makes OpenAI images from its plan's allowance.
    Plan(&'a Credential),
    /// A server the user runs, with the key it was given, if any.
    Server(Option<&'a str>),
}

/// One request: the client, who pays, the model, and where it goes instead of the provider's own host.
pub(super) struct Call<'a> {
    pub http: &'a reqwest::Client,
    pub auth: Auth<'a>,
    pub model: &'a str,
    /// A server's address, or where tests serve the provider.
    pub root: Option<String>,
}

impl Call<'_> {
    fn root<'a>(&'a self, host: &'a str) -> &'a str {
        self.root.as_deref().unwrap_or(host).trim_end_matches('/')
    }

    /// The key, for a provider that always takes one.
    fn key(&self) -> &str {
        match self.auth {
            Auth::Key(key) | Auth::Server(Some(key)) => key,
            _ => "",
        }
    }
}

/// Why a call made no image; `status` lets the caller tell a spent allowance from a bad prompt.
#[derive(Debug)]
pub(super) struct Failure {
    pub status: Option<u16>,
    pub message: String,
}

impl Failure {
    /// The payer is refused or out of allowance, so another payer may still succeed.
    pub(super) fn moves_on(&self) -> bool {
        matches!(self.status, Some(401..=403 | 429))
    }
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self { status: None, message }
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(status) => write!(formatter, "{status} {}", self.message),
            None => write!(formatter, "{}", self.message),
        }
    }
}

/// What one wire answered.
pub(super) struct Made {
    pub image: Image,
    /// In US dollars; `None` when neither the answer nor Drift's prices say.
    pub cost: Option<f64>,
    pub revised_prompt: Option<String>,
}

pub(super) fn made(image: Image, cost: Option<f64>, revised_prompt: Option<String>) -> Made {
    Made {
        image,
        cost,
        revised_prompt,
    }
}

pub(super) async fn generate(
    provider: &Provider,
    call: &Call<'_>,
    request: &ImageRequest<'_>,
) -> Result<Made, Failure> {
    if request.references.len() > provider.references {
        return Err(match provider.references {
            0 => format!("{} makes images from text only", provider.name),
            most => format!("{} takes at most {most} reference image(s)", provider.name),
        }
        .into());
    }

    match provider.id {
        "openai" | "compatible" => openai::generate(call, request).await,
        "google" => google::generate(call, request).await,
        "xai" => xai::generate(call, request).await,
        "recraft" => recraft::generate(call, request).await,
        "bfl" => flux::generate(call, request).await,
        "stability" => stability::generate(call, request).await,
        "ideogram" => ideogram::generate(call, request).await,
        "fal" => fal::generate(call, request).await,
        "comfyui" => comfyui::generate(call, request).await,
        _ => sdwebui::generate(call, request).await,
    }
}

/// Image models can take a minute or more on a large, high-quality picture; a local one on a small GPU longer.
const TIMEOUT: Duration = Duration::from_secs(300);
/// How long a provider that answers later (FLUX, ComfyUI) is waited on, and how often it is asked.
const POLL_FOR: Duration = Duration::from_secs(600);
const POLL_EVERY: Duration = Duration::from_millis(500);

/// The prices of the longest entry `model` starts with, so a dated snapshot is priced as its model.
pub(super) fn priced<const N: usize>(table: &[(&str, [f64; N])], model: &str) -> Option<[f64; N]> {
    table
        .iter()
        .filter(|(name, _)| model.starts_with(name))
        .max_by_key(|(name, _)| name.len())
        .map(|(_, prices)| *prices)
}

pub(super) fn tokens(value: &Value) -> f64 {
    value.as_u64().unwrap_or(0) as f64
}

pub(super) fn data_url(image: &Image) -> String {
    format!("data:{};base64,{}", image.mime, image.base64)
}

/// The image in a provider's base64 (or a data URL's), by what its bytes are rather than what the answer says.
pub(super) fn decoded(base64: &str) -> Result<Image, Failure> {
    let base64 = base64.split_once(";base64,").map_or(base64, |(_, data)| data);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64.trim())
        .map_err(|_| "the answer's image is not valid base64")?;

    sniffed(&bytes)
}

fn sniffed(bytes: &[u8]) -> Result<Image, Failure> {
    let mime = crate::tool::image::sniff(bytes).ok_or("the answer's image is not PNG, JPEG, GIF or WebP")?;

    Ok(Image::from_bytes(mime, bytes))
}

/// Sends the request and reads a JSON answer, keeping the status of a refusal.
pub(super) async fn send(request: reqwest::RequestBuilder) -> Result<Value, Failure> {
    let response = request
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();

    if (200..300).contains(&status) {
        return serde_json::from_str(&text).map_err(|error| error.to_string().into());
    }

    Err(Failure {
        status: Some(status),
        message: crate::llm::error_message(&text),
    })
}

pub(super) async fn post(request: reqwest::RequestBuilder, body: &Value) -> Result<Value, Failure> {
    send(request.json(body)).await
}

/// The image at a URL an answer names, or inline as a data URL. A private URL is fetched only from `root`.
pub(super) async fn download(url: &str, root: Option<&str>) -> Result<Image, Failure> {
    if url.starts_with("data:") {
        return decoded(url);
    }
    let start = reqwest::Url::parse(url).map_err(|_| refused())?;
    let response = fetch_image(start, root).await?;
    if !response.status().is_success() {
        return Err(format!("the image could not be fetched: {}", response.status()).into());
    }

    sniffed(&capped(response).await?)
}

/// Redirects are followed here so each hop is checked; the shared client cannot change that policy.
const MAX_REDIRECTS: usize = 10;

fn refused() -> Failure {
    "the image URL is not one that can be fetched".into()
}

fn image_client() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            #[cfg(test)]
            let _ = rustls::crypto::ring::default_provider().install_default();
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("image client")
        })
        .clone()
}

async fn fetch_image(start: reqwest::Url, root: Option<&str>) -> Result<reqwest::Response, Failure> {
    let client = image_client();
    let mut url = start;
    for _ in 0..MAX_REDIRECTS {
        fetchable(&url, root)?;
        let response = client
            .get(url.clone())
            .timeout(TIMEOUT)
            .send()
            .await
            .map_err(|error| format!("the image could not be fetched: {error}"))?;
        if !response.status().is_redirection() {
            return Ok(response);
        }
        url = next_url(&url, &response)?;
    }

    Err("the image could not be fetched: too many redirects".into())
}

fn next_url(from: &reqwest::Url, response: &reqwest::Response) -> Result<reqwest::Url, Failure> {
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(refused)?;

    from.join(location).map_err(|_| refused())
}

fn fetchable(url: &reqwest::Url, root: Option<&str>) -> Result<(), Failure> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(refused());
    }
    if root.is_some_and(|root| same_origin(url, root)) {
        return Ok(());
    }
    if private_url(url) { Err(refused()) } else { Ok(()) }
}

fn same_origin(url: &reqwest::Url, root: &str) -> bool {
    let Ok(root) = reqwest::Url::parse(root) else {
        return false;
    };

    url.scheme() == root.scheme()
        && url.host() == root.host()
        && url.port_or_known_default() == root.port_or_known_default()
}

fn private_url(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str() else {
        return true;
    };
    let host = host.trim_matches(['[', ']']).to_lowercase();
    if let Ok(address) = host.parse::<std::net::IpAddr>() {
        return private_ip(address);
    }

    host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") || !host.contains('.')
}

fn private_ip(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast()
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_unicast_link_local()
                || v6.is_unique_local()
                || v6.to_ipv4_mapped().is_some_and(|mapped| private_ip(mapped.into()))
        }
    }
}

/// The most of a generated image read from a URL; far past any real one.
const MAX_DOWNLOAD: usize = 64 * 1024 * 1024;

async fn capped(mut response: reqwest::Response) -> Result<Vec<u8>, Failure> {
    let too_large = || Failure::from(format!("the image is over {} MB", MAX_DOWNLOAD / 1024 / 1024));
    if response.content_length().is_some_and(|size| size > MAX_DOWNLOAD as u64) {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_DOWNLOAD {
            return Err(too_large());
        }
    }

    Ok(bytes)
}

/// Asks `check` until it has an answer, or gives up after [`POLL_FOR`].
pub(super) async fn poll<T, F, Fut>(mut check: F) -> Result<T, Failure>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Option<T>, Failure>>,
{
    let deadline = Instant::now() + POLL_FOR;
    while Instant::now() < deadline {
        if let Some(done) = check().await? {
            return Ok(done);
        }
        tokio::time::sleep(POLL_EVERY).await;
    }

    Err("the image took too long".into())
}

/// A random seed for a server that needs one to vary its pictures.
pub(super) fn seed() -> u64 {
    u64::from_str_radix(&crate::random_hex(6), 16).unwrap_or(0)
}

#[cfg(test)]
mod tests;
