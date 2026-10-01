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
pub struct SaveQuery {
    /// Adding a server: refused with 409 if one has the name, rather than replacing it.
    #[serde(default)]
    pub create: bool,
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
    let row = engine
        .mcp
        .change(&name, &engine.store, &engine.hub, |store| {
            let saved = store.mcp_server(&name)?;
            if query.create && saved.is_some() {
                return Err(taken(&name));
            }
            let config = input.resolve(saved.as_ref().map(|row| &row.config)).map_err(|why| ApiError::new(StatusCode::BAD_REQUEST, "secret", why))?;
            Ok(store.save_mcp_server(&name, &config)?)
        })
        .await?;
    reconnected(&engine, row).await
}

/// Renames a server, saved secrets included. 409 if the new name is taken: nothing is replaced.
#[utoipa::path(post, path = "/mcp/{name}/rename", operation_id = "renameMcpServer", request_body = RenameBody, responses((status = 200, body = ServerStatus), (status = 404), (status = 409)))]
pub async fn rename(State(engine): State<Arc<Engine>>, Path(name): Path<String>, Json(body): Json<RenameBody>) -> Result<Json<ServerStatus>, ApiError> {
    valid_name(&body.to)?;
    // Its tools are named after it, so the old name's tools end as they would on a remove.
    let row = engine
        .mcp
        .close(&name, &engine.store, &engine.hub, |store| match store.rename_mcp_server(&name, &body.to)? {
            Some(Renamed::To(row)) => Ok(row),
            Some(Renamed::Taken) => Err(taken(&body.to)),
            None => Err(ApiError::not_found("mcp server")),
        })
        .await?;
    engine.hub.publish(Event::McpRemoved { name });
    reconnected(&engine, row).await
}

/// An enabled server connects under the row just written; a disabled one is reported as it stands.
async fn reconnected(engine: &Arc<Engine>, row: ServerRow) -> Result<Json<ServerStatus>, ApiError> {
    if row.enabled {
        return connect(engine, &row.name).await;
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
    engine.hub.publish(Event::McpRemoved { name });
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/mcp/{name}/connect", operation_id = "connectMcpServer", responses((status = 200, body = ServerStatus), (status = 404)))]
pub async fn connect_route(State(engine): State<Arc<Engine>>, Path(name): Path<String>) -> Result<Json<ServerStatus>, ApiError> {
    connect(&engine, &name).await
}

async fn connect(engine: &Arc<Engine>, name: &str) -> Result<Json<ServerStatus>, ApiError> {
    engine.store.mcp_server(name)?.ok_or_else(|| ApiError::not_found("mcp server"))?;
    let _ = engine.connect_mcp(name).await;
    let row = engine.store.mcp_server(name)?.ok_or_else(|| ApiError::not_found("mcp server"))?;
    Ok(Json(engine.mcp.status_of(row)))
}

#[utoipa::path(post, path = "/mcp/{name}/disconnect", operation_id = "disconnectMcpServer", responses((status = 200, body = ServerStatus), (status = 404)))]
pub async fn disconnect(State(engine): State<Arc<Engine>>, Path(name): Path<String>) -> Result<Json<ServerStatus>, ApiError> {
    engine.mcp.disconnect(&name, &engine.store, &engine.hub).await;
    let row = engine.store.mcp_server(&name)?.ok_or_else(|| ApiError::not_found("mcp server"))?;
    Ok(Json(engine.mcp.status_of(row)))
}

#[utoipa::path(put, path = "/mcp/{name}/enabled", operation_id = "setMcpServerEnabled", request_body = EnabledBody, responses((status = 200, body = ServerStatus), (status = 404)))]
pub async fn set_enabled(State(engine): State<Arc<Engine>>, Path(name): Path<String>, Json(body): Json<EnabledBody>) -> Result<Json<ServerStatus>, ApiError> {
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
    connect(&engine, &name).await
}
