use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::error::ApiError;
use super::provider_accounts::ProviderAccount;
use crate::Engine;
use crate::llm::Credential;
use crate::llm::anthropic::oauth;
use crate::llm::catalog::{Model, ProviderInfo};
use crate::llm::credentials::Profile;
use crate::llm::openai::oauth as codex;

/// A catalog provider plus whether the engine can currently talk to it.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProviderStatus {
    pub id: String,
    pub name: String,
    pub connected: bool,
    /// `keychain`, `env` or absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
    /// Stored credentials in the order they are used; sign-ins take turns as each reaches its limit.
    pub accounts: Vec<ProviderAccount>,
    pub models: BTreeMap<String, Model>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ApiKeyBody {
    pub key: String,
}

#[utoipa::path(get, path = "/providers", operation_id = "listProviders", responses((status = 200, body = Vec<ProviderStatus>)))]
pub(super) async fn list(State(engine): State<Arc<Engine>>) -> Json<Vec<ProviderStatus>> {
    let catalog = engine.catalog_view();
    let statuses = catalog.providers.values().map(|info| status(&engine, info)).collect();

    Json(statuses)
}

fn status(engine: &Engine, info: &ProviderInfo) -> ProviderStatus {
    let accounts = engine.credentials.accounts(&info.id);
    let credential = if !accounts.is_empty() {
        Some("keychain".to_string())
    } else if engine.credentials.found_elsewhere(&info.id, &info.env) {
        Some("env".to_string())
    } else {
        None
    };

    ProviderStatus {
        id: info.id.clone(),
        name: info.name.clone(),
        connected: credential.is_some(),
        credential,
        accounts: accounts
            .into_iter()
            .map(|account| ProviderAccount::of(engine, account))
            .collect(),
        models: info.models.clone(),
    }
}

#[utoipa::path(put, path = "/providers/{id}/key", operation_id = "setProviderKey", request_body = ApiKeyBody, responses((status = 204), (status = 404)))]
pub(super) async fn set_key(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(body): Json<ApiKeyBody>,
) -> Result<StatusCode, ApiError> {
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
        .map_err(credentials_error)?;
    credentials_changed(&engine, &id);
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/providers/{id}/credentials", operation_id = "removeProviderCredentials", responses((status = 204)))]
pub(super) async fn remove(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    engine.credentials.remove(&id).map_err(credentials_error)?;
    if id == "openai" {
        engine.codex_credentials_changed();
    }
    engine.hub.publish(crate::event::Event::CatalogUpdated {});
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum OAuthMode {
    /// Claude Pro or Max.
    Max,
    /// Anthropic Console.
    Console,
    /// ChatGPT Plus, Pro or Team through Codex.
    Chatgpt,
    /// A SuperGrok subscription, signed in with a code entered in any browser.
    Supergrok,
}

#[derive(Deserialize, ToSchema)]
pub(super) struct OAuthStartBody {
    pub mode: OAuthMode,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct OAuthStarted {
    /// Open this in a browser.
    pub url: String,
    pub state: String,
    /// `code`: the user pastes what the callback page shows. `auto`: the engine catches the callback itself.
    pub method: String,
    /// For a device sign-in, what the user enters on the page `url` opens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_code: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub(super) struct OAuthFinishBody {
    /// For `code` flows: `code#state`, the callback URL, or its query string. Empty for `auto` flows.
    #[serde(default)]
    pub input: String,
    /// The `state` from `startOAuth`; required for `auto` flows.
    #[serde(default)]
    pub state: Option<String>,
}

#[utoipa::path(post, path = "/providers/{id}/oauth", operation_id = "startOAuth", request_body = OAuthStartBody, responses((status = 200, body = OAuthStarted), (status = 404)))]
pub(super) async fn oauth_start(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(body): Json<OAuthStartBody>,
) -> Result<Json<OAuthStarted>, ApiError> {
    let (started, method) = match (id.as_str(), body.mode) {
        ("anthropic", OAuthMode::Max) => (oauth::start(oauth::Mode::Max), "code"),
        ("anthropic", OAuthMode::Console) => (oauth::start(oauth::Mode::Console), "code"),
        ("openai", OAuthMode::Chatgpt) => {
            let started = codex::start();
            (
                oauth::Started {
                    url: started.url,
                    state: started.state,
                    verifier: started.verifier,
                },
                "auto",
            )
        }
        ("xai", OAuthMode::Supergrok) => return supergrok_start(&engine).await,
        _ => return Err(ApiError::not_found("oauth provider")),
    };

    engine
        .oauth
        .lock()
        .unwrap()
        .insert(started.state.clone(), started.verifier);
    Ok(Json(OAuthStarted {
        url: started.url,
        state: started.state,
        method: method.into(),
        user_code: None,
    }))
}

/// Asks xAI for a device code; the finish call waits for the user to enter it.
async fn supergrok_start(engine: &Arc<Engine>) -> Result<Json<OAuthStarted>, ApiError> {
    let device = crate::llm::xai::start(&engine.http)
        .await
        .map_err(|error| ApiError::new(StatusCode::BAD_GATEWAY, "oauth", error.to_string()))?;

    let state = crate::random_hex(16);
    engine
        .oauth
        .lock()
        .unwrap()
        .insert(state.clone(), serde_json::to_string(&device).unwrap());
    Ok(Json(OAuthStarted {
        url: device.url,
        state,
        method: "auto".into(),
        user_code: Some(device.user_code),
    }))
}

#[utoipa::path(post, path = "/providers/{id}/oauth/callback", operation_id = "finishOAuth", request_body = OAuthFinishBody, responses((status = 204), (status = 400), (status = 404)))]
pub(super) async fn oauth_finish(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(body): Json<OAuthFinishBody>,
) -> Result<StatusCode, ApiError> {
    let signed_in = match id.as_str() {
        "anthropic" => {
            let (code, state) = oauth::parse_callback(&body.input)
                .ok_or_else(|| invalid("paste the code#state value or the callback URL"))?;
            let verifier = pending(&engine, &state)?;
            oauth::exchange(&engine.http, &code, &state, &verifier).await
        }
        "openai" => {
            let state = body.state.ok_or_else(|| invalid("state is required"))?;
            let verifier = pending(&engine, &state)?;
            let code = codex::wait_for_callback(&state)
                .await
                .map_err(|error| ApiError::new(StatusCode::BAD_GATEWAY, "oauth", error.to_string()))?;
            codex::exchange(&engine.http, &code, &verifier).await.map(with_profile)
        }
        "xai" => {
            let state = body.state.ok_or_else(|| invalid("state is required"))?;
            let started = pending(&engine, &state)?;
            let device: crate::llm::xai::Device =
                serde_json::from_str(&started).map_err(|_| invalid("unknown or expired sign-in state"))?;
            crate::llm::xai::wait(&engine.http, &device).await.map(with_profile)
        }
        _ => return Err(ApiError::not_found("oauth provider")),
    };
    let (credential, profile) =
        signed_in.map_err(|error| ApiError::new(StatusCode::BAD_GATEWAY, "oauth", error.to_string()))?;

    engine
        .credentials
        .add_account(&id, &credential, &profile)
        .map_err(credentials_error)?;
    credentials_changed(&engine, &id);
    Ok(StatusCode::NO_CONTENT)
}

/// Who a JWT sign-in belongs to, read from its own token.
fn with_profile(credential: Credential) -> (Credential, Profile) {
    let profile = match &credential {
        Credential::OAuth { access, .. } => Profile::from_jwt(access),
        _ => Profile::default(),
    };

    (credential, profile)
}

/// What `startOAuth` kept for `state`, taken so a sign-in finishes once.
fn pending(engine: &Engine, state: &str) -> Result<String, ApiError> {
    engine
        .oauth
        .lock()
        .unwrap()
        .remove(state)
        .ok_or_else(|| invalid("unknown or expired sign-in state"))
}

fn invalid(message: &str) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "invalid", message)
}

pub(super) fn credentials_error(error: impl std::fmt::Display) -> ApiError {
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "credentials", error.to_string())
}

/// A sign-in changes which models a provider offers (a ChatGPT one only what Codex takes), so the picker reloads.
pub(super) fn credentials_changed(engine: &Arc<Engine>, id: &str) {
    if id == "openai" {
        engine.codex_credentials_changed();
    }
    engine.retry_deliveries(None);
    engine.hub.publish(crate::event::Event::CatalogUpdated {});
}
