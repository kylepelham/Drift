use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::error::ApiError;
use crate::llm::anthropic::oauth;
use crate::llm::catalog::{Model, ProviderInfo};
use crate::llm::Credential;
use crate::Engine;

/// A catalog provider plus whether the engine can currently talk to it.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub id: String,
    pub name: String,
    pub connected: bool,
    /// `keychain`, `env` or absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
    pub models: BTreeMap<String, Model>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyBody {
    pub key: String,
}

#[utoipa::path(get, path = "/providers", operation_id = "listProviders", responses((status = 200, body = Vec<ProviderStatus>)))]
pub async fn list(State(engine): State<Arc<Engine>>) -> Json<Vec<ProviderStatus>> {
    let catalog = engine.catalog.read().unwrap();
    let stored = engine.credentials.providers();
    let statuses = catalog.providers.values().map(|info| status(&engine, info, &stored)).collect();
    Json(statuses)
}

fn status(engine: &Engine, info: &ProviderInfo, stored: &[String]) -> ProviderStatus {
    let credential = if stored.iter().any(|id| id == &info.id) {
        Some("keychain".to_string())
    } else if engine.credentials.resolve(&info.id, &info.env).is_some() {
        Some("env".to_string())
    } else {
        None
    };
    ProviderStatus { id: info.id.clone(), name: info.name.clone(), connected: credential.is_some(), credential, models: info.models.clone() }
}

#[utoipa::path(put, path = "/providers/{id}/key", operation_id = "setProviderKey", request_body = ApiKeyBody, responses((status = 204), (status = 404)))]
pub async fn set_key(State(engine): State<Arc<Engine>>, Path(id): Path<String>, Json(body): Json<ApiKeyBody>) -> Result<StatusCode, ApiError> {
    if !engine.catalog.read().unwrap().providers.contains_key(&id) {
        return Err(ApiError::not_found("provider"));
    }
    let key = body.key.trim();
    if key.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid", "key is empty"));
    }
    engine
        .credentials
        .set(&id, &Credential::ApiKey { key: key.into() })
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "credentials", e))?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/providers/{id}/credentials", operation_id = "removeProviderCredentials", responses((status = 204)))]
pub async fn remove(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    engine.credentials.remove(&id).map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "credentials", e))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OAuthMode {
    Max,
    Console,
}

#[derive(Deserialize, ToSchema)]
pub struct OAuthStartBody {
    pub mode: OAuthMode,
}

#[derive(Serialize, ToSchema)]
pub struct OAuthStarted {
    /// Open this in a browser; the user pastes back what the callback page shows.
    pub url: String,
    pub state: String,
}

#[derive(Deserialize, ToSchema)]
pub struct OAuthFinishBody {
    /// `code#state`, the callback URL, or its query string.
    pub input: String,
}

#[utoipa::path(post, path = "/providers/{id}/oauth", operation_id = "startOAuth", request_body = OAuthStartBody, responses((status = 200, body = OAuthStarted), (status = 404)))]
pub async fn oauth_start(State(engine): State<Arc<Engine>>, Path(id): Path<String>, Json(body): Json<OAuthStartBody>) -> Result<Json<OAuthStarted>, ApiError> {
    if id != "anthropic" {
        return Err(ApiError::not_found("oauth provider"));
    }
    let mode = match body.mode {
        OAuthMode::Max => oauth::Mode::Max,
        OAuthMode::Console => oauth::Mode::Console,
    };
    let started = oauth::start(mode);
    engine.oauth.lock().unwrap().insert(started.state.clone(), started.verifier);
    Ok(Json(OAuthStarted { url: started.url, state: started.state }))
}

#[utoipa::path(post, path = "/providers/{id}/oauth/callback", operation_id = "finishOAuth", request_body = OAuthFinishBody, responses((status = 204), (status = 400), (status = 404)))]
pub async fn oauth_finish(State(engine): State<Arc<Engine>>, Path(id): Path<String>, Json(body): Json<OAuthFinishBody>) -> Result<StatusCode, ApiError> {
    if id != "anthropic" {
        return Err(ApiError::not_found("oauth provider"));
    }
    let (code, state) = oauth::parse_callback(&body.input).ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "invalid", "paste the code#state value or the callback URL"))?;
    let verifier = engine.oauth.lock().unwrap().remove(&state).ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "invalid", "unknown or expired sign-in state"))?;
    let credential = oauth::exchange(&engine.http, &code, &state, &verifier).await.map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, "oauth", e))?;
    engine.credentials.set(&id, &credential).map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "credentials", e))?;
    Ok(StatusCode::NO_CONTENT)
}
