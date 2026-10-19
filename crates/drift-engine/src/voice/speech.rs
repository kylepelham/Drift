//! Each speech provider's wire shape: a piece of a reply in, an MP3 out.

use base64::Engine as _;
use serde_json::json;

use super::{Provider, SpokenAudio, VoiceError, answer};

pub(super) const OPENAI_VOICE: &str = "marin";
/// "George", the premade voice ElevenLabs' own examples use; every account has it.
pub(super) const ELEVENLABS_VOICE: &str = "JBFqnCBsd6RMkjVDRZzb";

/// A voice is an id or a name; anything else could reach into the provider's URL.
fn checked(voice: Option<&str>, default: &'static str) -> Result<String, VoiceError> {
    let voice = voice
        .map(str::trim)
        .filter(|voice| !voice.is_empty())
        .unwrap_or(default);
    if voice.len() > 64 || !voice.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(VoiceError::Invalid(format!("{voice} is not a voice id")));
    }

    Ok(voice.to_string())
}

pub(super) async fn run(
    provider: &Provider,
    key: &str,
    text: &str,
    voice: Option<&str>,
    base: Option<&str>,
) -> Result<SpokenAudio, VoiceError> {
    let client = crate::llm::http::client();
    let request = match provider.id {
        "openai" => client
            .post(format!("{}/v1/audio/speech", base.unwrap_or("https://api.openai.com")))
            .bearer_auth(key)
            .json(&json!({
                "model": "gpt-4o-mini-tts",
                "input": text,
                "voice": checked(voice, OPENAI_VOICE)?,
                "response_format": "mp3",
            })),
        "elevenlabs" => client
            .post(format!(
                "{}/v1/text-to-speech/{}?output_format=mp3_44100_128",
                base.unwrap_or("https://api.elevenlabs.io"),
                checked(voice, ELEVENLABS_VOICE)?
            ))
            .header("xi-api-key", key)
            .json(&json!({ "text": text, "model_id": "eleven_flash_v2_5" })),
        other => return Err(VoiceError::Unknown(other.into())),
    };
    let audio = answer(provider.name, request)
        .await?
        .bytes()
        .await
        .map_err(|error| VoiceError::Failed(provider.name, error.to_string()))?;

    Ok(SpokenAudio {
        audio: base64::engine::general_purpose::STANDARD.encode(audio),
        mime: "audio/mpeg".into(),
    })
}
