use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

use super::error::ApiError;
use crate::Engine;
use crate::event::Event;
use crate::mcp::{ServerConfigInput, ServerRow, ServerStatus};
use crate::store::Renamed;

#[derive(Deserialize, ToSchema)]
pub(super) struct EnabledBody {
    pub enabled: bool,
}

#[derive(Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(rename_all = "camelCase")]
pub(super) struct SaveQuery {
    /// Adding a server: refused with 409 if one has the name, rather than replacing it.
    #[serde(default)]
    pub create: bool,
    /// Whether read-only agents (plan, explore) may use the tools it marks read-only; left out, a new
    /// server is trusted and a saved one keeps what it had.
    #[serde(default)]
    pub read_only_trusted: Option<bool>,
    /// The active workspace, where a stdio server connects (besides every workspace it already ran in).
    #[serde(default)]
    pub workspace: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(rename_all = "camelCase")]
pub(super) struct ConnectQuery {
    /// The active workspace, where a stdio server connects (besides every workspace it already ran in).
    /// On connect and disconnect, the server is turned on or off there, and remembered for it.
    #[serde(default)]
    pub workspace: Option<String>,
}

/// A workspace id as the folder a stdio server runs in; an unknown one is none.
fn workspace_path(engine: &Engine, id: Option<&str>) -> Option<std::path::PathBuf> {
    let workspace = engine.store.workspace(id?).ok().flatten()?;
    Some(crate::tool::canonical(std::path::Path::new(&workspace.path)))
}

#[derive(Deserialize, ToSchema)]
pub(super) struct RenameBody {
    pub to: String,
}

#[utoipa::path(get, path = "/mcp", operation_id = "listMcpServers", responses((status = 200, body = Vec<ServerStatus>)))]
pub(super) async fn list(State(engine): State<Arc<Engine>>) -> Result<Json<Vec<ServerStatus>>, ApiError> {
    Ok(Json(engine.mcp.statuses(&engine.store)?))
}

/// Saving a changed config reconnects the server on it; env and header values sent as null keep the saved ones.
#[utoipa::path(put, path = "/mcp/{name}", operation_id = "saveMcpServer", params(SaveQuery), request_body = ServerConfigInput, responses((status = 200, body = ServerStatus), (status = 400), (status = 409)))]
pub(super) async fn save(
    State(engine): State<Arc<Engine>>,
    Path(name): Path<String>,
    Query(query): Query<SaveQuery>,
    Json(input): Json<ServerConfigInput>,
) -> Result<Json<ServerStatus>, ApiError> {
    valid_name(&name)?;
    let before = engine.store.mcp_server(&name)?;

    let row = engine
        .mcp
        .change(&name, &engine.store, &engine.hub, |store| {
            let saved = store.mcp_server(&name)?;
            // A row this build cannot read still holds the name: adding is refused, editing replaces it.
            if query.create && (saved.is_some() || store.unreadable_mcp_servers()?.contains(&name)) {
                return Err(taken(&name));
            }
            let config = input
                .resolve(saved.as_ref().map(|row| &row.config))
                .map_err(|why| ApiError::new(StatusCode::BAD_REQUEST, "secret", why.to_string()))?;
            let mut row = store.save_mcp_server(&name, &config)?;
            if let Some(trusted) = query.read_only_trusted {
                store.set_mcp_read_only_trusted(&name, trusted)?;
                row.read_only_trusted = trusted;
            }
            Ok(row)
        })
        .await?;
    if let Some(before) = before {
        crate::mcp::forget_if_moved(&engine.credentials, &name, &before.config, &row.config);
    }

    reconnected(&engine, row, query.workspace.as_deref()).await
}

/// Renames a server, saved secrets included. 409 if the new name is taken: nothing is replaced.
#[utoipa::path(post, path = "/mcp/{name}/rename", operation_id = "renameMcpServer", params(ConnectQuery), request_body = RenameBody, responses((status = 200, body = ServerStatus), (status = 404), (status = 409)))]
pub(super) async fn rename(
    State(engine): State<Arc<Engine>>,
    Path(name): Path<String>,
    Query(query): Query<ConnectQuery>,
    Json(body): Json<RenameBody>,
) -> Result<Json<ServerStatus>, ApiError> {
    valid_name(&body.to)?;

    // Its tools are named after it, so the old name's tools end as they would on a remove.
    let row = engine
        .mcp
        .close(&name, &engine.store, &engine.hub, |store| {
            match store.rename_mcp_server(&name, &body.to)? {
                Some(Renamed::To(row)) => Ok(*row),
                Some(Renamed::Taken) => Err(taken(&body.to)),
                None => Err(ApiError::not_found("mcp server")),
            }
        })
        .await?;
    // A sign-in belongs to the server, not to its old name.
    crate::mcp::move_sign_in(&engine.credentials, &name, &row.name);
    engine.hub.publish(Event::McpRemoved { name });

    reconnected(&engine, row, query.workspace.as_deref()).await
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct SignInPage {
    /// Open this in the browser; when the browser comes back, the server connects signed in.
    pub url: String,
}

/// Starts signing in to a remote server that requires OAuth.
#[utoipa::path(post, path = "/mcp/{name}/signin", operation_id = "signInMcpServer", responses((status = 200, body = SignInPage), (status = 400), (status = 404)))]
pub(super) async fn sign_in(
    State(engine): State<Arc<Engine>>,
    Path(name): Path<String>,
) -> Result<Json<SignInPage>, ApiError> {
    readable(&engine, &name)?;

    let url = engine
        .sign_in_mcp(&name)
        .await
        .map_err(|why| ApiError::new(StatusCode::BAD_REQUEST, "signin", why.to_string()))?;
    Ok(Json(SignInPage { url }))
}

/// Forgets a server's sign-in and reconnects it without one.
#[utoipa::path(delete, path = "/mcp/{name}/signin", operation_id = "signOutMcpServer", responses((status = 200, body = ServerStatus), (status = 404)))]
pub(super) async fn sign_out(
    State(engine): State<Arc<Engine>>,
    Path(name): Path<String>,
) -> Result<Json<ServerStatus>, ApiError> {
    readable(&engine, &name)?;

    engine
        .sign_out_mcp(&name)
        .await
        .map_err(|why| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "credentials", why.to_string()))?;

    status(&engine, &name)
}

/// An enabled server connects under the row just written; a disabled one is reported as it stands.
async fn reconnected(
    engine: &Arc<Engine>,
    row: ServerRow,
    workspace: Option<&str>,
) -> Result<Json<ServerStatus>, ApiError> {
    if row.enabled {
        return connect(engine, &row.name, workspace).await;
    }

    let status = engine.mcp.status_of(row);
    engine.hub.publish(Event::McpUpdated { server: status.clone() });
    Ok(Json(status))
}

fn valid_name(name: &str) -> Result<(), ApiError> {
    let allowed = |character: char| character.is_ascii_alphanumeric() || character == '-' || character == '_';
    if name.is_empty() || !name.chars().all(allowed) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid",
            "server names are letters, digits, - and _",
        ));
    }
    Ok(())
}

fn taken(name: &str) -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "taken",
        format!("a server named {name} already exists"),
    )
}

#[utoipa::path(delete, path = "/mcp/{name}", operation_id = "removeMcpServer", responses((status = 204), (status = 404)))]
pub(super) async fn remove(
    State(engine): State<Arc<Engine>>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    if !engine
        .mcp
        .close(&name, &engine.store, &engine.hub, |store| {
            store.remove_mcp_server(&name)
        })
        .await?
    {
        return Err(ApiError::not_found("mcp server"));
    }

    let _ = crate::mcp::forget_sign_in(&engine.credentials, &name);
    engine.hub.publish(Event::McpRemoved { name });
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/mcp/{name}/connect", operation_id = "connectMcpServer", params(ConnectQuery), responses((status = 200, body = ServerStatus), (status = 404), (status = 409)))]
pub(super) async fn connect_route(
    State(engine): State<Arc<Engine>>,
    Path(name): Path<String>,
    Query(query): Query<ConnectQuery>,
) -> Result<Json<ServerStatus>, ApiError> {
    readable(&engine, &name)?;

    if let Some(id) = query
        .workspace
        .as_deref()
        .filter(|id| workspace_path(&engine, Some(id)).is_some())
    {
        engine.store.set_mcp_choice(&name, id, true)?;
    }

    // A stdio server with no workspace to run in is refused; any other failure shows on the status.
    if let Err(why) = engine
        .connect_mcp_in(&name, workspace_path(&engine, query.workspace.as_deref()).as_deref())
        .await
        && matches!(why, crate::mcp::Error::NeedsWorkspace)
    {
        return Err(ApiError::new(StatusCode::CONFLICT, "workspace", why.to_string()));
    }

    status(&engine, &name)
}

/// A save, rename or enable connects too, where it can: a stdio server with no workspace to run in waits for one.
async fn connect(engine: &Arc<Engine>, name: &str, workspace: Option<&str>) -> Result<Json<ServerStatus>, ApiError> {
    readable(engine, name)?;

    let _ = engine
        .connect_mcp_in(name, workspace_path(engine, workspace).as_deref())
        .await;

    status(engine, name)
}

fn status(engine: &Engine, name: &str) -> Result<Json<ServerStatus>, ApiError> {
    let row = engine
        .store
        .mcp_server(name)?
        .ok_or_else(|| ApiError::not_found("mcp server"))?;
    Ok(Json(engine.mcp.status_of(row)))
}

/// With a workspace, the server goes off there only; without one, every connection ends until the user connects it again.
#[utoipa::path(post, path = "/mcp/{name}/disconnect", operation_id = "disconnectMcpServer", params(ConnectQuery), responses((status = 200, body = ServerStatus), (status = 404)))]
pub(super) async fn disconnect(
    State(engine): State<Arc<Engine>>,
    Path(name): Path<String>,
    Query(query): Query<ConnectQuery>,
) -> Result<Json<ServerStatus>, ApiError> {
    match query
        .workspace
        .as_deref()
        .and_then(|id| Some((id, workspace_path(&engine, Some(id))?)))
    {
        Some((id, path)) => {
            let off = |store: &crate::store::Store| store.set_mcp_choice(&name, id, false);
            if !engine
                .mcp
                .disconnect_in(
                    crate::mcp::WorkspaceServer {
                        name: &name,
                        workspace: &path,
                    },
                    &engine.store,
                    &engine.hub,
                    off,
                )
                .await?
            {
                return Err(ApiError::not_found("mcp server"));
            }
        }
        None => drop(engine.mcp.disconnect(&name, &engine.store, &engine.hub).await),
    }

    status(&engine, &name)
}

/// Not found unless the server's row is one this build can read, so nothing is written for one it cannot.
fn readable(engine: &Engine, name: &str) -> Result<(), ApiError> {
    engine
        .store
        .mcp_server(name)?
        .map(|_| ())
        .ok_or_else(|| ApiError::not_found("mcp server"))
}

#[utoipa::path(put, path = "/mcp/{name}/enabled", operation_id = "setMcpServerEnabled", params(ConnectQuery), request_body = EnabledBody, responses((status = 200, body = ServerStatus), (status = 404)))]
pub(super) async fn set_enabled(
    State(engine): State<Arc<Engine>>,
    Path(name): Path<String>,
    Query(query): Query<ConnectQuery>,
    Json(body): Json<EnabledBody>,
) -> Result<Json<ServerStatus>, ApiError> {
    readable(&engine, &name)?;

    if !body.enabled {
        if !engine
            .mcp
            .close(&name, &engine.store, &engine.hub, |store| {
                store.set_mcp_enabled(&name, false)
            })
            .await?
        {
            return Err(ApiError::not_found("mcp server"));
        }
        return status(&engine, &name);
    }

    if !engine.store.set_mcp_enabled(&name, true)? {
        return Err(ApiError::not_found("mcp server"));
    }
    connect(&engine, &name, query.workspace.as_deref()).await
}

#[cfg(test)]
mod tests;
