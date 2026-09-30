use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use utoipa::ToSchema;

use super::error::ApiError;
use crate::event::Event;
use crate::mcp::{ServerConfig, ServerStatus};
use crate::Engine;

#[derive(Deserialize, ToSchema)]
pub struct EnabledBody {
    pub enabled: bool,
}

#[utoipa::path(get, path = "/mcp", operation_id = "listMcpServers", responses((status = 200, body = Vec<ServerStatus>)))]
pub async fn list(State(engine): State<Arc<Engine>>) -> Result<Json<Vec<ServerStatus>>, ApiError> {
    Ok(Json(engine.mcp.statuses(&engine.store)?))
}

/// Saving a changed config disconnects the server and withdraws approval until the user approves it again.
#[utoipa::path(put, path = "/mcp/{name}", operation_id = "saveMcpServer", request_body = ServerConfig, responses((status = 200, body = ServerStatus)))]
pub async fn save(State(engine): State<Arc<Engine>>, Path(name): Path<String>, Json(config): Json<ServerConfig>) -> Result<Json<ServerStatus>, ApiError> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid", "server names are letters, digits, - and _"));
    }
    engine.mcp.invalidate(&name);
    engine.mcp.disconnect(&name, &engine.store, &engine.hub).await;
    engine.tools.set_dynamic(engine.mcp.tools());
    let row = engine.store.save_mcp_server(&name, &config)?;
    let status = engine.mcp.status_of(row);
    engine.hub.publish(Event::McpUpdated { server: status.clone() });
    Ok(Json(status))
}

#[utoipa::path(delete, path = "/mcp/{name}", operation_id = "removeMcpServer", responses((status = 204), (status = 404)))]
pub async fn remove(State(engine): State<Arc<Engine>>, Path(name): Path<String>) -> Result<StatusCode, ApiError> {
    engine.mcp.disconnect(&name, &engine.store, &engine.hub).await;
    engine.tools.set_dynamic(engine.mcp.tools());
    if !engine.store.remove_mcp_server(&name)? {
        return Err(ApiError::not_found("mcp server"));
    }
    engine.hub.publish(Event::McpRemoved { name });
    Ok(StatusCode::NO_CONTENT)
}

/// Approves the config exactly as stored now, then connects.
#[utoipa::path(post, path = "/mcp/{name}/approve", operation_id = "approveMcpServer", responses((status = 200, body = ServerStatus), (status = 404)))]
pub async fn approve(State(engine): State<Arc<Engine>>, Path(name): Path<String>) -> Result<Json<ServerStatus>, ApiError> {
    let row = engine.store.mcp_server(&name)?.ok_or_else(|| ApiError::not_found("mcp server"))?;
    engine.store.approve_mcp_server(&name, &row.hash())?;
    connect(&engine, &name).await
}

#[utoipa::path(post, path = "/mcp/{name}/connect", operation_id = "connectMcpServer", responses((status = 200, body = ServerStatus), (status = 404)))]
pub async fn connect_route(State(engine): State<Arc<Engine>>, Path(name): Path<String>) -> Result<Json<ServerStatus>, ApiError> {
    connect(&engine, &name).await
}

async fn connect(engine: &Arc<Engine>, name: &str) -> Result<Json<ServerStatus>, ApiError> {
    let row = engine.store.mcp_server(name)?.ok_or_else(|| ApiError::not_found("mcp server"))?;
    let _ = engine.mcp.connect(row.clone(), &engine.hub).await;
    engine.tools.set_dynamic(engine.mcp.tools());
    let row = engine.store.mcp_server(name)?.ok_or_else(|| ApiError::not_found("mcp server"))?;
    Ok(Json(engine.mcp.status_of(row)))
}

#[utoipa::path(post, path = "/mcp/{name}/disconnect", operation_id = "disconnectMcpServer", responses((status = 200, body = ServerStatus), (status = 404)))]
pub async fn disconnect(State(engine): State<Arc<Engine>>, Path(name): Path<String>) -> Result<Json<ServerStatus>, ApiError> {
    engine.mcp.disconnect(&name, &engine.store, &engine.hub).await;
    engine.tools.set_dynamic(engine.mcp.tools());
    let row = engine.store.mcp_server(&name)?.ok_or_else(|| ApiError::not_found("mcp server"))?;
    Ok(Json(engine.mcp.status_of(row)))
}

#[utoipa::path(put, path = "/mcp/{name}/enabled", operation_id = "setMcpServerEnabled", request_body = EnabledBody, responses((status = 200, body = ServerStatus), (status = 404)))]
pub async fn set_enabled(State(engine): State<Arc<Engine>>, Path(name): Path<String>, Json(body): Json<EnabledBody>) -> Result<Json<ServerStatus>, ApiError> {
    if !engine.store.set_mcp_enabled(&name, body.enabled)? {
        return Err(ApiError::not_found("mcp server"));
    }
    if body.enabled {
        return connect(&engine, &name).await;
    }
    disconnect(State(engine), Path(name)).await
}
