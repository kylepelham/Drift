use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

use super::error::ApiError;
use crate::event::Event;
use crate::mcp::{ServerConfigInput, ServerRow, ServerStatus};
use crate::store::Renamed;
use crate::Engine;

#[derive(Deserialize, ToSchema)]
pub struct EnabledBody {
    pub enabled: bool,
}

#[derive(Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(rename_all = "camelCase")]
pub struct SaveQuery {
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
pub struct ConnectQuery {
    /// The active workspace, where a stdio server connects (besides every workspace it already ran in).
    #[serde(default)]
    pub workspace: Option<String>,
}

/// A workspace id as the folder a stdio server runs in; an unknown one is none.
fn workspace_path(engine: &Engine, id: Option<&str>) -> Option<std::path::PathBuf> {
    let workspace = engine.store.workspace(id?).ok().flatten()?;
    Some(crate::tool::canonical(std::path::Path::new(&workspace.path)))
}

#[derive(Deserialize, ToSchema)]
pub struct RenameBody {
    pub to: String,
}

#[utoipa::path(get, path = "/mcp", operation_id = "listMcpServers", responses((status = 200, body = Vec<ServerStatus>)))]
pub async fn list(State(engine): State<Arc<Engine>>) -> Result<Json<Vec<ServerStatus>>, ApiError> {
    Ok(Json(engine.mcp.statuses(&engine.store)?))
}

/// Saving a changed config reconnects the server on it; env and header values sent as null keep the saved ones.
#[utoipa::path(put, path = "/mcp/{name}", operation_id = "saveMcpServer", params(SaveQuery), request_body = ServerConfigInput, responses((status = 200, body = ServerStatus), (status = 400), (status = 409)))]
pub async fn save(State(engine): State<Arc<Engine>>, Path(name): Path<String>, Query(query): Query<SaveQuery>, Json(input): Json<ServerConfigInput>) -> Result<Json<ServerStatus>, ApiError> {
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
            let config = input.resolve(saved.as_ref().map(|row| &row.config)).map_err(|why| ApiError::new(StatusCode::BAD_REQUEST, "secret", why))?;
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
pub async fn rename(State(engine): State<Arc<Engine>>, Path(name): Path<String>, Query(query): Query<ConnectQuery>, Json(body): Json<RenameBody>) -> Result<Json<ServerStatus>, ApiError> {
    valid_name(&body.to)?;
    // Its tools are named after it, so the old name's tools end as they would on a remove.
    let row = engine
        .mcp
        .close(&name, &engine.store, &engine.hub, |store| match store.rename_mcp_server(&name, &body.to)? {
            Some(Renamed::To(row)) => Ok(*row),
            Some(Renamed::Taken) => Err(taken(&body.to)),
            None => Err(ApiError::not_found("mcp server")),
        })
        .await?;
    // A sign-in belongs to the server, not to its old name.
    crate::mcp::move_sign_in(&engine.credentials, &name, &row.name);
    engine.hub.publish(Event::McpRemoved { name });
    reconnected(&engine, row, query.workspace.as_deref()).await
}

#[derive(serde::Serialize, ToSchema)]
pub struct SignInPage {
    /// Open this in the browser; when the browser comes back, the server connects signed in.
    pub url: String,
}

/// Starts signing in to a remote server that requires OAuth.
#[utoipa::path(post, path = "/mcp/{name}/signin", operation_id = "signInMcpServer", responses((status = 200, body = SignInPage), (status = 400), (status = 404)))]
pub async fn sign_in(State(engine): State<Arc<Engine>>, Path(name): Path<String>) -> Result<Json<SignInPage>, ApiError> {
    engine.store.mcp_server(&name)?.ok_or_else(|| ApiError::not_found("mcp server"))?;
    let url = engine.sign_in_mcp(&name).await.map_err(|why| ApiError::new(StatusCode::BAD_REQUEST, "signin", why))?;
    Ok(Json(SignInPage { url }))
}

/// Forgets a server's sign-in and reconnects it without one.
#[utoipa::path(delete, path = "/mcp/{name}/signin", operation_id = "signOutMcpServer", responses((status = 200, body = ServerStatus), (status = 404)))]
pub async fn sign_out(State(engine): State<Arc<Engine>>, Path(name): Path<String>) -> Result<Json<ServerStatus>, ApiError> {
    engine.store.mcp_server(&name)?.ok_or_else(|| ApiError::not_found("mcp server"))?;
    engine.sign_out_mcp(&name).await.map_err(|why| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "credentials", why))?;
    let row = engine.store.mcp_server(&name)?.ok_or_else(|| ApiError::not_found("mcp server"))?;
    Ok(Json(engine.mcp.status_of(row)))
}

/// An enabled server connects under the row just written; a disabled one is reported as it stands.
async fn reconnected(engine: &Arc<Engine>, row: ServerRow, workspace: Option<&str>) -> Result<Json<ServerStatus>, ApiError> {
    if row.enabled {
        return connect(engine, &row.name, workspace).await;
    }
    let status = engine.mcp.status_of(row);
    engine.hub.publish(Event::McpUpdated { server: status.clone() });
    Ok(Json(status))
}

fn valid_name(name: &str) -> Result<(), ApiError> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid", "server names are letters, digits, - and _"));
    }
    Ok(())
}

fn taken(name: &str) -> ApiError {
    ApiError::new(StatusCode::CONFLICT, "taken", format!("a server named {name} already exists"))
}

#[utoipa::path(delete, path = "/mcp/{name}", operation_id = "removeMcpServer", responses((status = 204), (status = 404)))]
pub async fn remove(State(engine): State<Arc<Engine>>, Path(name): Path<String>) -> Result<StatusCode, ApiError> {
    if !engine.mcp.close(&name, &engine.store, &engine.hub, |store| store.remove_mcp_server(&name)).await? {
        return Err(ApiError::not_found("mcp server"));
    }
    let _ = crate::mcp::forget_sign_in(&engine.credentials, &name);
    engine.hub.publish(Event::McpRemoved { name });
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/mcp/{name}/connect", operation_id = "connectMcpServer", params(ConnectQuery), responses((status = 200, body = ServerStatus), (status = 404)))]
pub async fn connect_route(State(engine): State<Arc<Engine>>, Path(name): Path<String>, Query(query): Query<ConnectQuery>) -> Result<Json<ServerStatus>, ApiError> {
    connect(&engine, &name, query.workspace.as_deref()).await
}

async fn connect(engine: &Arc<Engine>, name: &str, workspace: Option<&str>) -> Result<Json<ServerStatus>, ApiError> {
    engine.store.mcp_server(name)?.ok_or_else(|| ApiError::not_found("mcp server"))?;
    let _ = engine.connect_mcp_in(name, workspace_path(engine, workspace).as_deref()).await;
    let row = engine.store.mcp_server(name)?.ok_or_else(|| ApiError::not_found("mcp server"))?;
    Ok(Json(engine.mcp.status_of(row)))
}

#[utoipa::path(post, path = "/mcp/{name}/disconnect", operation_id = "disconnectMcpServer", responses((status = 200, body = ServerStatus), (status = 404)))]
pub async fn disconnect(State(engine): State<Arc<Engine>>, Path(name): Path<String>) -> Result<Json<ServerStatus>, ApiError> {
    engine.mcp.disconnect(&name, &engine.store, &engine.hub).await;
    let row = engine.store.mcp_server(&name)?.ok_or_else(|| ApiError::not_found("mcp server"))?;
    Ok(Json(engine.mcp.status_of(row)))
}

/// A switch changes only a server this build can read; one it cannot is saved again or removed, so nothing is written for it.
fn readable(engine: &Engine, name: &str) -> Result<(), ApiError> {
    engine.store.mcp_server(name)?.map(|_| ()).ok_or_else(|| ApiError::not_found("mcp server"))
}

#[utoipa::path(put, path = "/mcp/{name}/enabled", operation_id = "setMcpServerEnabled", params(ConnectQuery), request_body = EnabledBody, responses((status = 200, body = ServerStatus), (status = 404)))]
pub async fn set_enabled(State(engine): State<Arc<Engine>>, Path(name): Path<String>, Query(query): Query<ConnectQuery>, Json(body): Json<EnabledBody>) -> Result<Json<ServerStatus>, ApiError> {
    readable(&engine, &name)?;
    if !body.enabled {
        if !engine.mcp.close(&name, &engine.store, &engine.hub, |store| store.set_mcp_enabled(&name, false)).await? {
            return Err(ApiError::not_found("mcp server"));
        }
        let row = engine.store.mcp_server(&name)?.ok_or_else(|| ApiError::not_found("mcp server"))?;
        return Ok(Json(engine.mcp.status_of(row)));
    }
    if !engine.store.set_mcp_enabled(&name, true)? {
        return Err(ApiError::not_found("mcp server"));
    }
    connect(&engine, &name, query.workspace.as_deref()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn switches_on_a_server_that_does_not_parse_write_nothing() {
        let dir = std::env::temp_dir().join(format!("drift-api-mcp-{}", crate::random_hex(4)));
        let engine = Engine::open_with(&dir, crate::Options { file_credentials: true, ..Default::default() }).unwrap();
        let config = crate::mcp::ServerConfig::Stdio { command: "npx".into(), args: vec![], env: Default::default(), cwd: None, timeout_seconds: None };
        engine.store.save_mcp_server("newer", &config).unwrap();
        engine.store.set_mcp_enabled("newer", false).unwrap();
        engine.store.lock().execute("UPDATE mcp_config SET config_json = '{\"type\":\"future\"}'", []).unwrap();
        let path = || Path("newer".to_string());
        assert!(set_enabled(State(engine.clone()), path(), Query(ConnectQuery { workspace: None }), Json(EnabledBody { enabled: true })).await.is_err());
        let enabled: bool = engine.store.lock().query_row("SELECT enabled FROM mcp_config", [], |row| row.get(0)).unwrap();
        assert!(!enabled, "refused before anything was written");
        std::fs::remove_dir_all(dir).ok();
    }
}
