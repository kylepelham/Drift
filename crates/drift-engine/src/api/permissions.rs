use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;

use super::error::ApiError;
use crate::permission::{ReplyBody, Request};
use crate::Engine;

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
