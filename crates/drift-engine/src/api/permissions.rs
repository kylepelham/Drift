use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;

use super::error::ApiError;
use crate::permission::{Grant, ReplyBody, Request};
use crate::Engine;

/// The workspace's "always" grants, newest last.
#[utoipa::path(get, path = "/workspaces/{id}/permission-grants", operation_id = "listPermissionGrants", responses((status = 200, body = Vec<Grant>), (status = 404)))]
pub async fn grants(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> Result<Json<Vec<Grant>>, ApiError> {
    if engine.store.workspace(&id)?.is_none() {
        return Err(ApiError::not_found("workspace"));
    }
    Ok(Json(engine.permission_grants(&id)))
}

/// Takes back every "always" grant of the workspace; calls ask again from the next one.
#[utoipa::path(delete, path = "/workspaces/{id}/permission-grants", operation_id = "revokePermissionGrants", responses((status = 204)))]
pub async fn revoke_all(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> StatusCode {
    engine.revoke_permission_grant(&id, None);
    StatusCode::NO_CONTENT
}

/// Takes back one "always" grant, as listed; 404 when the workspace holds no such grant.
#[utoipa::path(post, path = "/workspaces/{id}/permission-grants/revoke", operation_id = "revokePermissionGrant", request_body = Grant, responses((status = 204), (status = 404)))]
pub async fn revoke(State(engine): State<Arc<Engine>>, Path(id): Path<String>, Json(grant): Json<Grant>) -> Result<StatusCode, ApiError> {
    if !engine.revoke_permission_grant(&id, Some(&grant)) {
        return Err(ApiError::not_found("permission grant"));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Everything still waiting on the user; clients read this when they hydrate.
#[utoipa::path(get, path = "/permissions", operation_id = "listPermissions", responses((status = 200, body = Vec<Request>)))]
pub async fn list(State(engine): State<Arc<Engine>>) -> Json<Vec<Request>> {
    Json(engine.permissions.pending())
}

#[utoipa::path(post, path = "/permissions/{id}/reply", operation_id = "replyPermission", request_body = ReplyBody, responses((status = 204), (status = 404)))]
pub async fn reply(State(engine): State<Arc<Engine>>, Path(id): Path<String>, Json(body): Json<ReplyBody>) -> Result<StatusCode, ApiError> {
    engine.permissions.reply(&engine.hub, &id, body).map_err(|_| ApiError::not_found("pending permission"))?;
    Ok(StatusCode::NO_CONTENT)
}
