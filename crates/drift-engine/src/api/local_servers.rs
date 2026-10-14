//! Where a local server (Ollama, LM Studio) listens and the largest window its models run with, set from Settings.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::error::ApiError;
use crate::Engine;
use crate::config;
use crate::llm::local::LOCAL;

/// A local server as the engine reaches it.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LocalServer {
    /// Where the engine asks for its models.
    pub address: String,
    /// Where it listens unless the user says otherwise.
    pub default_address: String,
    /// The largest window its models run with, in tokens; absent for each model's full window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_limit: Option<u64>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LocalServerInput {
    /// An http or https address; absent or empty for the default.
    #[serde(default)]
    pub address: Option<String>,
    /// Absent or 0 for each model's full window.
    #[serde(default)]
    pub context_limit: Option<u64>,
}

#[utoipa::path(get, path = "/providers/{id}/server", operation_id = "getLocalServer", responses((status = 200, body = LocalServer), (status = 404)))]
pub(super) async fn get(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
) -> Result<Json<LocalServer>, ApiError> {
    let default = default_address(&id)?;

    Ok(Json(current(&engine, &id, default)))
}

/// Saves the address and limit in the user's drift.json and asks the server again at once.
#[utoipa::path(put, path = "/providers/{id}/server", operation_id = "setLocalServer", request_body = LocalServerInput, responses((status = 200, body = LocalServer), (status = 400), (status = 404)))]
pub(super) async fn set(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(body): Json<LocalServerInput>,
) -> Result<Json<LocalServer>, ApiError> {
    let default = default_address(&id)?;
    let address = address(body.address.as_deref())?.filter(|address| address != default);
    let limit = body.context_limit.filter(|limit| *limit > 0);

    config::set_local_server(&id, address.as_deref(), limit)
        .map_err(|error| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "config", error.to_string()))?;
    if let Some(provider) = engine.catalog.write().unwrap().providers.get_mut(&id) {
        provider.api = Some(address.unwrap_or_else(|| default.into()));
    }
    engine.ask_local().await;

    Ok(Json(current(&engine, &id, default)))
}

fn default_address(id: &str) -> Result<&'static str, ApiError> {
    LOCAL
        .iter()
        .find(|(local, _, _)| *local == id)
        .map(|(_, _, address)| *address)
        .ok_or_else(|| ApiError::not_found("local server"))
}

/// The address as typed, without a trailing slash; `None` when left empty.
fn address(typed: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(typed) = typed.map(str::trim).filter(|typed| !typed.is_empty()) else {
        return Ok(None);
    };

    let parsed = reqwest::Url::parse(typed).ok();
    let web = parsed.is_some_and(|url| matches!(url.scheme(), "http" | "https") && url.host().is_some());
    if !web {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid",
            "the address is an http or https URL",
        ));
    }

    Ok(Some(typed.trim_end_matches('/').to_string()))
}

fn current(engine: &Engine, id: &str, default: &str) -> LocalServer {
    let address = engine
        .catalog
        .read()
        .unwrap()
        .providers
        .get(id)
        .and_then(|provider| provider.api.clone())
        .unwrap_or_else(|| default.into());
    let context_limit = config::user_providers()
        .get(id)
        .and_then(|provider| provider.context_limit)
        .filter(|limit| *limit > 0);

    LocalServer {
        address,
        default_address: default.into(),
        context_limit,
    }
}
