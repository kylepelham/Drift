use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use super::error::ApiError;
use crate::Engine;
use crate::permission::{Grant, ReplyBody, Request, Rule};

/// Every rule is checked on every call; past this the list is a mistake, not a policy.
const MAX_RULES: usize = 200;

/// The rules kept in Settings for every workspace, in the order they are checked: the first that
/// matches decides, after the rules in drift.json.
#[utoipa::path(get, path = "/permission-rules", operation_id = "listPermissionRules", responses((status = 200, body = Vec<Rule>)))]
pub(super) async fn rules(State(engine): State<Arc<Engine>>) -> Json<Vec<Rule>> {
    Json(engine.permission_rules())
}

/// Replaces the whole ordered list; calls checked from now on follow it. A rule that could never match is refused, naming it.
#[utoipa::path(put, path = "/permission-rules", operation_id = "savePermissionRules", request_body = Vec<Rule>, responses((status = 200, body = Vec<Rule>), (status = 400)))]
pub(super) async fn save_rules(
    State(engine): State<Arc<Engine>>,
    Json(rules): Json<Vec<Rule>>,
) -> Result<Json<Vec<Rule>>, ApiError> {
    if rules.len() > MAX_RULES {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "too_many",
            format!("at most {MAX_RULES} rules"),
        ));
    }
    if let Some(problem) = rules.iter().find_map(Rule::problem) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid", problem));
    }
    engine.set_permission_rules(rules)?;
    Ok(Json(engine.permission_rules()))
}

/// The workspace's "always" grants, newest last.
#[utoipa::path(get, path = "/workspaces/{id}/permission-grants", operation_id = "listPermissionGrants", responses((status = 200, body = Vec<Grant>), (status = 404)))]
pub(super) async fn grants(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<Grant>>, ApiError> {
    known(&engine, &id)?;
    Ok(Json(engine.permission_grants(&id)))
}

/// Takes back every "always" grant of the workspace; calls ask again from the next one.
#[utoipa::path(delete, path = "/workspaces/{id}/permission-grants", operation_id = "revokePermissionGrants", responses((status = 204), (status = 404)))]
pub(super) async fn revoke_all(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    known(&engine, &id)?;
    engine.revoke_permission_grant(&id, None);
    Ok(StatusCode::NO_CONTENT)
}

/// Takes back one "always" grant, as listed; 404 when the workspace is unknown or holds no such grant.
#[utoipa::path(post, path = "/workspaces/{id}/permission-grants/revoke", operation_id = "revokePermissionGrant", request_body = Grant, responses((status = 204), (status = 404)))]
pub(super) async fn revoke(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(grant): Json<Grant>,
) -> Result<StatusCode, ApiError> {
    known(&engine, &id)?;
    if !engine.revoke_permission_grant(&id, Some(&grant)) {
        return Err(ApiError::not_found("permission grant"));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// 404 for a workspace the engine does not have, before anything is loaded or cached for it.
fn known(engine: &Engine, id: &str) -> Result<(), ApiError> {
    match engine.store.workspace(id)? {
        Some(_) => Ok(()),
        None => Err(ApiError::not_found("workspace")),
    }
}

/// Everything still waiting on the user; clients read this when they hydrate.
#[utoipa::path(get, path = "/permissions", operation_id = "listPermissions", responses((status = 200, body = Vec<Request>)))]
pub(super) async fn list(State(engine): State<Arc<Engine>>) -> Json<Vec<Request>> {
    Json(engine.permissions.pending())
}

#[utoipa::path(post, path = "/permissions/{id}/reply", operation_id = "replyPermission", request_body = ReplyBody, responses((status = 204), (status = 404)))]
pub(super) async fn reply(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(body): Json<ReplyBody>,
) -> Result<StatusCode, ApiError> {
    engine
        .permissions
        .reply(&engine.hub, &id, body)
        .map_err(|_| ApiError::not_found("pending permission"))?;
    Ok(StatusCode::NO_CONTENT)
}
