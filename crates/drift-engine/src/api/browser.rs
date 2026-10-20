//! A conversation's browser, the engines and credentials Settings > Browser manages, and the view socket the pane
//! watches and drives a browser through.

use std::sync::Arc;

use axum::Json;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;

use super::Lease;
use super::error::ApiError;
use crate::Engine;
use crate::browser::driver::{Driver, watch};
use crate::browser::engines::EngineStatus;
use crate::browser::project::Credential;
use crate::browser::{BrowserProject, BrowserRecord, Kind, ViewInput};

/// A workspace's browser setup as Settings shows it: its drift.json section and its stored sign-ins.
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorkspaceBrowser {
    pub project: BrowserProject,
    pub credentials: Vec<Credential>,
}

/// A sign-in to store; a password left out keeps the stored one.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct CredentialInput {
    pub user: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub password: Option<String>,
}

/// Where an engine's program is, set by hand; empty goes back to finding it.
#[derive(Deserialize, ToSchema)]
pub(super) struct EnginePath {
    pub path: Option<String>,
}

/// The conversation's browser, or null when it has none running.
#[utoipa::path(get, path = "/sessions/{id}/browser", operation_id = "getBrowser", responses((status = 200, body = Option<BrowserRecord>)))]
pub(super) async fn get(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> Json<Option<BrowserRecord>> {
    Json(engine.browsers.record(&id))
}

/// Ends the conversation's browser; `browser.updated` says when it has gone.
#[utoipa::path(post, path = "/sessions/{id}/browser/stop", operation_id = "stopBrowser", responses((status = 200, body = BrowserRecord), (status = 404)))]
pub(super) async fn stop(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
) -> Result<Json<BrowserRecord>, ApiError> {
    let driver = engine.browsers.get(&id).ok_or_else(|| ApiError::not_found("browser"))?;
    driver.stop();

    Ok(Json(driver.record()))
}

/// Every engine: whether it can run here, where its program is, and any download in progress.
#[utoipa::path(get, path = "/browser/engines", operation_id = "browserEngines", responses((status = 200, body = Vec<EngineStatus>)))]
pub(super) async fn engines(State(engine): State<Arc<Engine>>) -> Json<Vec<EngineStatus>> {
    Json(engine.browser_engines())
}

/// Starts fetching an engine Drift can download (Camoufox, IEDriverServer); `browserEngines` shows progress.
#[utoipa::path(post, path = "/browser/engines/{engine}/download", operation_id = "downloadBrowserEngine", responses((status = 202), (status = 400)))]
pub(super) async fn download(
    State(engine): State<Arc<Engine>>,
    Path(kind): Path<String>,
) -> Result<StatusCode, ApiError> {
    let kind = parse_kind(&kind)?;
    engine
        .download_browser_engine(kind)
        .map_err(|message| ApiError::new(StatusCode::BAD_REQUEST, "not_downloadable", message))?;

    Ok(StatusCode::ACCEPTED)
}

/// Sets or clears the program an engine runs (for IE mode, IEDriverServer).
#[utoipa::path(put, path = "/browser/engines/{engine}/path", operation_id = "setBrowserEnginePath", request_body = EnginePath, responses((status = 200, body = Vec<EngineStatus>)))]
pub(super) async fn set_path(
    State(engine): State<Arc<Engine>>,
    Path(kind): Path<String>,
    Json(body): Json<EnginePath>,
) -> Result<Json<Vec<EngineStatus>>, ApiError> {
    engine.set_browser_path(parse_kind(&kind)?, body.path.as_deref())?;

    Ok(Json(engine.browser_engines()))
}

fn parse_kind(text: &str) -> Result<Kind, ApiError> {
    Kind::parse(text).ok_or_else(|| ApiError::not_found("engine"))
}

/// A workspace's drift.json `browser` section and its stored sign-ins, without passwords. A workspace with no folder
/// on record has no section, but its sign-ins are still listed.
#[utoipa::path(get, path = "/workspaces/{id}/browser", operation_id = "workspaceBrowser", responses((status = 200, body = WorkspaceBrowser)))]
pub(super) async fn workspace(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
) -> Result<Json<WorkspaceBrowser>, ApiError> {
    let project = engine
        .store
        .workspace(&id)?
        .map(|workspace| engine.workspace_config(std::path::Path::new(&workspace.path)).browser)
        .unwrap_or_default();

    Ok(Json(WorkspaceBrowser {
        project,
        credentials: engine.browser_credentials(&id),
    }))
}

/// Stores a sign-in for a workspace's browser; its password goes to the keychain.
#[utoipa::path(put, path = "/workspaces/{id}/browser/credentials/{name}", operation_id = "saveBrowserCredential", request_body = CredentialInput, responses((status = 200, body = Vec<Credential>)))]
pub(super) async fn save_credential(
    State(engine): State<Arc<Engine>>,
    Path((id, name)): Path<(String, String)>,
    Json(body): Json<CredentialInput>,
) -> Result<Json<Vec<Credential>>, ApiError> {
    let credential = Credential {
        name,
        user: body.user,
        domain: body.domain,
        has_password: false,
    };
    engine
        .save_browser_credential(&id, &credential, body.password.as_deref())
        .map_err(|message| ApiError::new(StatusCode::BAD_REQUEST, "credential", message))?;

    Ok(Json(engine.browser_credentials(&id)))
}

#[utoipa::path(delete, path = "/workspaces/{id}/browser/credentials/{name}", operation_id = "removeBrowserCredential", responses((status = 200, body = Vec<Credential>)))]
pub(super) async fn remove_credential(
    State(engine): State<Arc<Engine>>,
    Path((id, name)): Path<(String, String)>,
) -> Result<Json<Vec<Credential>>, ApiError> {
    engine
        .remove_browser_credential(&id, &name)
        .map_err(|message| ApiError::new(StatusCode::BAD_REQUEST, "credential", message))?;

    Ok(Json(engine.browser_credentials(&id)))
}

/// Frames of the active page while the socket is open, as binary messages (a 4-byte little-endian header length,
/// a JSON header, a JPEG); the client sends `ViewInput` as JSON text.
#[utoipa::path(
    get,
    path = "/sessions/{id}/browser/view",
    operation_id = "browserView",
    responses((status = 101, description = "WebSocket; binary frames out, ViewInput in"), (status = 404))
)]
pub(super) async fn view(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    lease: Option<axum::Extension<Lease>>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let driver = engine.browsers.get(&id).ok_or_else(|| ApiError::not_found("browser"))?;
    let lease = lease.map(|axum::Extension(Lease(token))| token).unwrap_or_default();

    Ok(ws.on_upgrade(move |socket| watching(driver, socket, lease)))
}

async fn watching(driver: Arc<dyn Driver>, mut socket: WebSocket, lease: CancellationToken) {
    let mut viewer = watch(driver.clone());

    // Input is applied in the order it was sent: a click's press must reach the page before its release.
    let (inputs, mut queued) = mpsc::unbounded_channel::<ViewInput>();
    let driving = driver.clone();
    let applier = tokio::spawn(async move {
        while let Some(input) = queued.recv().await {
            let _ = driving.input(input).await;
        }
    });

    loop {
        tokio::select! {
            () = lease.cancelled() => break,
            () = driver.common().stopped() => break,
            frame = viewer.frames.recv() => match frame {
                Ok(frame) => {
                    if socket.send(Message::Binary(frame.encode().into())).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(input) = serde_json::from_str::<ViewInput>(&text) {
                        let _ = inputs.send(input);
                    }
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }

    applier.abort();
}
