use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

use super::error::ApiError;
use crate::Engine;
use crate::lsp::LspStatus;

#[derive(Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub(super) struct LspQuery {
    /// The workspace whose drift.json and `node_modules/.bin` count too.
    pub workspace: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LspServer {
    pub name: String,
    pub workspace: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LspEnabled {
    pub name: String,
    pub enabled: bool,
    pub workspace: Option<String>,
}

/// The workspace's folder, its drift.json's `lsp` (else the user's own), and the plugins' servers.
struct Scope {
    folder: Option<std::path::PathBuf>,
    config: std::collections::BTreeMap<String, crate::config::LspConfig>,
    plugins: Vec<(String, crate::hook::LanguageServer)>,
}

fn scope(engine: &Engine, workspace: Option<&str>) -> Scope {
    let folder = workspace
        .and_then(|id| engine.store.workspace(id).ok().flatten())
        .map(|workspace| crate::tool::canonical(std::path::Path::new(&workspace.path)));
    let config = match &folder {
        Some(folder) => engine.workspace_config(folder).lsp,
        None => crate::config::user_lsp(),
    };

    Scope {
        folder,
        config,
        plugins: engine.hooks.language_servers_by_plugin(),
    }
}

fn statuses(engine: &Engine, workspace: Option<&str>) -> Json<Vec<LspStatus>> {
    let scope = scope(engine, workspace);

    Json(
        engine
            .lsp
            .statuses(&scope.config, &scope.plugins, scope.folder.as_deref()),
    )
}

/// Every language server Drift could use, where it comes from, and whether it is installed and answers.
#[utoipa::path(get, path = "/lsp", operation_id = "listLanguageServers", params(LspQuery), responses((status = 200, body = Vec<LspStatus>)))]
pub(super) async fn list(State(engine): State<Arc<Engine>>, Query(query): Query<LspQuery>) -> Json<Vec<LspStatus>> {
    statuses(&engine, query.workspace.as_deref())
}

/// Starts a server, waits up to 30 seconds for it to answer and stops it; the list says what it found.
#[utoipa::path(post, path = "/lsp/check", operation_id = "checkLanguageServer", request_body = LspServer, responses((status = 200, body = Vec<LspStatus>)))]
pub(super) async fn check(State(engine): State<Arc<Engine>>, Json(body): Json<LspServer>) -> Json<Vec<LspStatus>> {
    let scope = scope(&engine, body.workspace.as_deref());
    let _ = engine
        .lsp
        .check(&body.name, &scope.config, &scope.plugins, scope.folder.as_deref())
        .await;

    statuses(&engine, body.workspace.as_deref())
}

/// Installs a missing server with the command the list offered: Drift's own for a built-in, its plugin's otherwise.
#[utoipa::path(post, path = "/lsp/install", operation_id = "installLanguageServer", request_body = LspServer, responses((status = 200, body = Vec<LspStatus>), (status = 422)))]
pub(super) async fn install(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<LspServer>,
) -> Result<Json<Vec<LspStatus>>, ApiError> {
    let scope = scope(&engine, body.workspace.as_deref());
    engine
        .lsp
        .install(&body.name, &scope.config, &scope.plugins)
        .await
        .map_err(|why| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "install", why))?;

    Ok(statuses(&engine, body.workspace.as_deref()))
}

/// Turns a server off in the user's drift.json, or on again.
#[utoipa::path(put, path = "/lsp/enabled", operation_id = "setLanguageServerEnabled", request_body = LspEnabled, responses((status = 200, body = Vec<LspStatus>)))]
pub(super) async fn set_enabled(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<LspEnabled>,
) -> Result<Json<Vec<LspStatus>>, ApiError> {
    crate::config::set_lsp_enabled(&body.name, body.enabled)
        .map_err(|error| ApiError::new(StatusCode::BAD_REQUEST, "lsp", error.to_string()))?;

    Ok(statuses(&engine, body.workspace.as_deref()))
}
