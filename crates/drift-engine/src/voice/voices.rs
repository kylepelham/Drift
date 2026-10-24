//! The voices a speech provider offers the account behind a key, for Settings > Voice to list.

use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use super::{Provider, VoiceError, answer_json};

/// Enough for any account's own and saved voices; the public library is not listed.
const MAX_PAGES: usize = 10;

/// Where a voice comes from, as ElevenLabs sorts its voices.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum VoiceGroup {
    /// The provider's own premade voices.
    Default,
    /// Voices the account cloned, designed or trained.
    Custom,
    /// Voices from the public library the account saved.
    Community,
    /// Voices shared with the account's workspace.
    Workspace,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct SpeechVoice {
    pub id: String,
    pub name: String,
    pub group: VoiceGroup,
    /// What its labels say of it: gender, age, accent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

pub(super) async fn list(provider: &Provider, key: &str, base: Option<&str>) -> Result<Vec<SpeechVoice>, VoiceError> {
    if provider.id != "elevenlabs" {
        return Err(VoiceError::Unknown(provider.name.into()));
    }

    let client = crate::llm::http::client();
    let url = format!("{}/v2/voices", base.unwrap_or("https://api.elevenlabs.io"));
    let mut voices = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut query = vec![
            ("page_size", "100".to_string()),
            ("include_total_count", "false".into()),
        ];
        query.extend(token.take().map(|token| ("next_page_token", token)));
        let page_url = reqwest::Url::parse_with_params(&url, &query)
            .map_err(|error| VoiceError::Invalid(format!("{url}: {error}")))?;
        let page = answer_json(provider.name, client.get(page_url).header("xi-api-key", key)).await?;

        voices.extend(page["voices"].as_array().into_iter().flatten().filter_map(voice));
        token = page["next_page_token"]
            .as_str()
            .filter(|_| page["has_more"] == true)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
    }

    Ok(voices)
}

fn voice(entry: &Value) -> Option<SpeechVoice> {
    let id = entry["voice_id"].as_str()?;
    let group = if entry["category"] == "premade" {
        VoiceGroup::Default
    } else if entry["sharing"]["status"]
        .as_str()
        .is_some_and(|status| status.starts_with("copied"))
    {
        VoiceGroup::Community
    } else if entry["is_owner"] == false {
        VoiceGroup::Workspace
    } else {
        VoiceGroup::Custom
    };
    let labels: Vec<&str> = ["gender", "age", "accent"]
        .iter()
        .filter_map(|label| entry["labels"][label].as_str())
        .filter(|value| !value.is_empty())
        .collect();

    Some(SpeechVoice {
        id: id.into(),
        name: entry["name"].as_str().unwrap_or(id).into(),
        group,
        detail: (!labels.is_empty()).then(|| labels.join(", ").replace('_', " ")),
    })
}
