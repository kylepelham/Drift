use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::error::ApiError;
use crate::Engine;
use crate::voice::{Speech, SpokenAudio, Transcription, VoiceError, VoiceProvider};

/// Settings > Voice: the cloud providers and which have a key, shown by its last characters.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct VoiceState {
    pub providers: Vec<VoiceProvider>,
}

#[derive(Serialize, ToSchema)]
pub(super) struct Transcript {
    pub text: String,
}

impl From<VoiceError> for ApiError {
    fn from(error: VoiceError) -> Self {
        let status = if error.is_request() {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::BAD_GATEWAY
        };
        Self::new(status, "voice", error.to_string())
    }
}

fn state(engine: &Engine) -> Json<VoiceState> {
    Json(VoiceState {
        providers: engine.voice_providers(),
    })
}

#[utoipa::path(get, path = "/voice", operation_id = "getVoice", responses((status = 200, body = VoiceState)))]
pub(super) async fn get(State(engine): State<Arc<Engine>>) -> Json<VoiceState> {
    state(&engine)
}

#[derive(Deserialize, ToSchema)]
pub(super) struct VoiceKey {
    pub provider: String,
    pub key: String,
}

/// Saves a provider's key in the credential store, replacing the one it had.
#[utoipa::path(put, path = "/voice/keys", operation_id = "setVoiceKey", request_body = VoiceKey, responses((status = 200, body = VoiceState)))]
pub(super) async fn set_key(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<VoiceKey>,
) -> Result<Json<VoiceState>, ApiError> {
    engine.set_voice_key(&body.provider, &body.key)?;

    Ok(state(&engine))
}

#[derive(Deserialize, utoipa::IntoParams)]
pub(super) struct ProviderParam {
    pub provider: String,
}

#[utoipa::path(delete, path = "/voice/keys", operation_id = "removeVoiceKey", params(ProviderParam), responses((status = 200, body = VoiceState)))]
pub(super) async fn remove_key(
    State(engine): State<Arc<Engine>>,
    Query(query): Query<ProviderParam>,
) -> Result<Json<VoiceState>, ApiError> {
    engine.remove_voice_key(&query.provider)?;

    Ok(state(&engine))
}

/// Transcribes one dictated phrase with the cloud provider named; a failure names why and tries no other.
#[utoipa::path(post, path = "/voice/transcribe", operation_id = "transcribeSpeech", request_body = Transcription, responses((status = 200, body = Transcript)))]
pub(super) async fn transcribe(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<Transcription>,
) -> Result<Json<Transcript>, ApiError> {
    let text = engine.voice_transcribe(&body).await?;

    Ok(Json(Transcript { text }))
}

/// Speaks one piece of a reply with the cloud provider named.
#[utoipa::path(post, path = "/voice/speak", operation_id = "speakText", request_body = Speech, responses((status = 200, body = SpokenAudio)))]
pub(super) async fn speak(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<Speech>,
) -> Result<Json<SpokenAudio>, ApiError> {
    Ok(Json(engine.voice_speak(&body).await?))
}
