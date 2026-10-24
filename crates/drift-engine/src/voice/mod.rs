//! Cloud speech for Settings > Voice: a dictated phrase or a reply's prose sent to the provider the user picked,
//! with the key saved here. A failure is reported as it is; no other provider is tried in its place.

use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::Engine;

mod speech;
mod transcribe;
mod voices;

pub use voices::{SpeechVoice, VoiceGroup};

/// Whisper's rate, which the webview records at; cloud providers are sent the same samples as a WAV.
pub const SAMPLE_RATE: u32 = 16_000;
/// Three minutes of 16-bit mono, the segmenter's ceiling with room to spare.
pub const MAX_AUDIO_BYTES: usize = SAMPLE_RATE as usize * 2 * 180;
/// OpenAI's limit for one speech request; the app sends a long reply in pieces below it.
pub const MAX_SPEECH_CHARS: usize = 4096;
const TIMEOUT: Duration = Duration::from_secs(120);

pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    pub transcribes: bool,
    pub speaks: bool,
}

pub const PROVIDERS: &[Provider] = &[
    Provider {
        id: "openai",
        name: "OpenAI",
        transcribes: true,
        speaks: true,
    },
    Provider {
        id: "groq",
        name: "Groq",
        transcribes: true,
        speaks: false,
    },
    Provider {
        id: "deepgram",
        name: "Deepgram",
        transcribes: true,
        speaks: false,
    },
    Provider {
        id: "elevenlabs",
        name: "ElevenLabs",
        transcribes: true,
        speaks: true,
    },
    Provider {
        id: "assemblyai",
        name: "AssemblyAI",
        transcribes: true,
        speaks: false,
    },
];

/// A provider as Settings > Voice lists it.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VoiceProvider {
    pub id: String,
    pub name: String,
    pub transcription: bool,
    pub speech: bool,
    /// The saved key's last four characters; never the key.
    pub key: Option<String>,
}

/// One dictated phrase for a cloud provider.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Transcription {
    pub provider: String,
    /// 16 kHz mono 16-bit little-endian samples, base64.
    pub audio: String,
    /// An ISO 639-1 code; left out or `auto`, the provider detects the language.
    #[serde(default)]
    pub language: Option<String>,
    /// Words the provider should expect: repo names, libraries, flags.
    #[serde(default)]
    pub keyterms: Vec<String>,
}

/// One piece of a reply to speak.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Speech {
    pub provider: String,
    pub text: String,
    /// The provider's voice; its default when left out.
    #[serde(default)]
    pub voice: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpokenAudio {
    /// The audio file, base64.
    pub audio: String,
    pub mime: String,
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum VoiceError {
    #[error("{0} does not offer this in Drift")]
    Unknown(String),
    #[error("{0} has no key; add one in Settings > Voice")]
    NoKey(&'static str),
    #[error("{0}")]
    Invalid(String),
    #[error("{0} refused the key: {1}")]
    Refused(&'static str, String),
    #[error("{0} is rate limited; try again shortly")]
    RateLimited(&'static str),
    #[error("{0} failed: {1}")]
    Failed(&'static str, String),
}

impl VoiceError {
    /// The request was the app's mistake rather than the provider's.
    pub fn is_request(&self) -> bool {
        matches!(self, Self::Unknown(_) | Self::NoKey(_) | Self::Invalid(_))
    }
}

fn secret_key(provider: &str) -> String {
    format!("voice:{provider}")
}

fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|provider| provider.id == id)
}

/// A 16-bit mono PCM WAV around `samples`, as whisper and every cloud provider read it.
pub fn wav(samples: &[u8]) -> Vec<u8> {
    let data = samples.len() as u32;
    let mut file = Vec::with_capacity(44 + samples.len());
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&(36 + data).to_le_bytes());
    file.extend_from_slice(b"WAVEfmt ");
    file.extend_from_slice(&16u32.to_le_bytes());
    file.extend_from_slice(&1u16.to_le_bytes());
    file.extend_from_slice(&1u16.to_le_bytes());
    file.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    file.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    file.extend_from_slice(&2u16.to_le_bytes());
    file.extend_from_slice(&16u16.to_le_bytes());
    file.extend_from_slice(b"data");
    file.extend_from_slice(&data.to_le_bytes());
    file.extend_from_slice(samples);

    file
}

/// What a phrase becomes on the wire, whichever provider takes it.
pub(crate) struct Phrase {
    pub wav: Vec<u8>,
    pub language: Option<String>,
    pub keyterms: Vec<String>,
}

impl Phrase {
    fn from_request(request: &Transcription) -> Result<Self, VoiceError> {
        let samples = base64::engine::general_purpose::STANDARD
            .decode(&request.audio)
            .map_err(|error| VoiceError::Invalid(format!("unreadable audio: {error}")))?;
        if samples.is_empty() || samples.len() > MAX_AUDIO_BYTES {
            return Err(VoiceError::Invalid("unusable audio length".into()));
        }
        let language = request
            .language
            .as_deref()
            .map(str::trim)
            .filter(|code| !code.is_empty() && *code != "auto")
            .map(str::to_string);
        let keyterms = request
            .keyterms
            .iter()
            .map(|term| term.trim().to_string())
            .filter(|term| !term.is_empty())
            .collect();

        Ok(Self {
            wav: wav(&samples),
            language,
            keyterms,
        })
    }
}

/// Reads a provider's answer, telling a refused key and a rate limit apart from any other failure.
async fn answer(name: &'static str, request: reqwest::RequestBuilder) -> Result<reqwest::Response, VoiceError> {
    let response = request
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|error| VoiceError::Failed(name, error.to_string()))?;
    let status = response.status().as_u16();
    if (200..300).contains(&status) {
        return Ok(response);
    }
    let text = response.text().await.unwrap_or_default();

    Err(match status {
        401 | 403 => VoiceError::Refused(name, crate::llm::error_message(&text)),
        429 => VoiceError::RateLimited(name),
        _ => VoiceError::Failed(name, format!("{status} {}", crate::llm::error_message(&text))),
    })
}

async fn answer_json(name: &'static str, request: reqwest::RequestBuilder) -> Result<serde_json::Value, VoiceError> {
    answer(name, request)
        .await?
        .json()
        .await
        .map_err(|error| VoiceError::Failed(name, error.to_string()))
}

impl Engine {
    fn voice_key(&self, provider: &str) -> Option<String> {
        self.credentials
            .secret(&secret_key(provider))
            .filter(|key| !key.is_empty())
    }

    pub fn voice_providers(&self) -> Vec<VoiceProvider> {
        PROVIDERS
            .iter()
            .map(|provider| VoiceProvider {
                id: provider.id.into(),
                name: provider.name.into(),
                transcription: provider.transcribes,
                speech: provider.speaks,
                key: self.voice_key(provider.id).map(|key| crate::websearch::last_four(&key)),
            })
            .collect()
    }

    pub fn set_voice_key(&self, provider: &str, key: &str) -> Result<(), VoiceError> {
        let known = self::provider(provider).ok_or_else(|| VoiceError::Unknown(provider.into()))?;
        let key = key.trim();
        if key.is_empty() {
            return Err(VoiceError::Invalid("the key is empty".into()));
        }

        self.credentials
            .set_secret(&secret_key(known.id), key)
            .map_err(|error| VoiceError::Invalid(error.to_string()))
    }

    pub fn remove_voice_key(&self, provider: &str) -> Result<(), VoiceError> {
        let known = self::provider(provider).ok_or_else(|| VoiceError::Unknown(provider.into()))?;

        self.credentials
            .remove_secret(&secret_key(known.id))
            .map_err(|error| VoiceError::Invalid(error.to_string()))
    }

    fn voice_route(&self, id: &str, speech: bool) -> Result<(&'static Provider, String), VoiceError> {
        let provider = provider(id)
            .filter(|provider| if speech { provider.speaks } else { provider.transcribes })
            .ok_or_else(|| VoiceError::Unknown(id.into()))?;
        let key = self.voice_key(provider.id).ok_or(VoiceError::NoKey(provider.name))?;

        Ok((provider, key))
    }

    /// Transcribes one phrase with the provider asked for; its failure is the answer, never another provider's text.
    pub async fn voice_transcribe(&self, request: &Transcription) -> Result<String, VoiceError> {
        let (provider, key) = self.voice_route(&request.provider, false)?;
        let phrase = Phrase::from_request(request)?;

        transcribe::run(provider, &key, &phrase, None).await
    }

    pub async fn voice_speak(&self, request: &Speech) -> Result<SpokenAudio, VoiceError> {
        let (provider, key) = self.voice_route(&request.provider, true)?;
        let text = request.text.trim();
        if text.is_empty() || text.chars().count() > MAX_SPEECH_CHARS {
            return Err(VoiceError::Invalid(format!(
                "speech text must be 1 to {MAX_SPEECH_CHARS} characters"
            )));
        }

        speech::run(provider, &key, text, request.voice.as_deref(), None).await
    }

    /// The voices the provider offers the account its key belongs to.
    pub async fn speech_voices(&self, provider: &str) -> Result<Vec<SpeechVoice>, VoiceError> {
        let (provider, key) = self.voice_route(provider, true)?;

        voices::list(provider, &key, None).await
    }
}

#[cfg(test)]
mod tests;
