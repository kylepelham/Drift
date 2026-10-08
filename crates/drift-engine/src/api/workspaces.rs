use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::Engine;
use crate::event::Event;
use crate::store::Workspace;

#[derive(Serialize, Deserialize, ToSchema)]
pub(super) struct NewWorkspace {
    pub path: String,
    pub name: String,
    #[serde(default)]
    pub icon: String,
}

#[utoipa::path(get, path = "/workspaces", operation_id = "listWorkspaces", responses((status = 200, body = Vec<Workspace>)))]
pub(super) async fn list(State(engine): State<Arc<Engine>>) -> Result<Json<Vec<Workspace>>, StatusCode> {
    engine
        .store
        .workspaces()
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[utoipa::path(
    post,
    path = "/workspaces",
    operation_id = "createWorkspace",
    request_body = NewWorkspace,
    responses((status = 201, body = Workspace))
)]
pub(super) async fn create(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<NewWorkspace>,
) -> Result<(StatusCode, Json<Workspace>), StatusCode> {
    let workspace = engine
        .store
        .add_workspace(&body.path, &body.name, &body.icon)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    engine.hub.publish(Event::WorkspaceCreated {
        workspace: workspace.clone(),
    });
    Ok((StatusCode::CREATED, Json(workspace)))
}

#[derive(Deserialize, utoipa::IntoParams)]
pub(super) struct FileQuery {
    /// What the user typed after `@`.
    #[serde(default)]
    pub query: String,
    /// At most this many paths; default 20.
    pub limit: Option<usize>,
}

/// Workspace paths for an @ mention, best match first (directories end in `/`). Names only: reading a
/// mentioned file is decided when the prompt is sent.
#[utoipa::path(get, path = "/workspaces/{id}/files", operation_id = "findFiles", params(FileQuery), responses((status = 200, body = Vec<String>), (status = 404)))]
pub(super) async fn files(
    State(engine): State<Arc<Engine>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<FileQuery>,
) -> Result<Json<Vec<String>>, StatusCode> {
    let workspace = engine
        .store
        .workspace(&id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let root = crate::tool::canonical(std::path::Path::new(&workspace.path));
    let limit = query.limit.unwrap_or(20).clamp(1, 200);
    let found = tokio::task::spawn_blocking(move || crate::tool::glob::search_names(&root, &query.query, limit))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(found))
}

#[derive(Serialize, ToSchema)]
pub(super) struct Purged {
    /// Conversations deleted, subagents and archived ones included.
    pub deleted: usize,
}

/// Deletes every conversation of a workspace the user removed: the seven-day purge's last step,
/// after which the shell forgets the workspace. 409 while it is in use again or one of them runs.
#[utoipa::path(post, path = "/workspaces/{id}/purge", operation_id = "purgeWorkspace", responses((status = 200, body = Purged), (status = 404), (status = 409)))]
pub(super) async fn purge(
    State(engine): State<Arc<Engine>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<Purged>, crate::api::error::ApiError> {
    use crate::api::error::ApiError;
    match engine.purge_removed_workspace(&id)? {
        crate::WorkspacePurge::Purged(deleted) => Ok(Json(Purged { deleted })),
        crate::WorkspacePurge::InUse => Err(ApiError::new(
            StatusCode::CONFLICT,
            "in_use",
            "the workspace is in use; nothing was deleted",
        )),
        crate::WorkspacePurge::Busy => Err(ApiError::new(
            StatusCode::CONFLICT,
            "busy",
            "one of its conversations is running",
        )),
        crate::WorkspacePurge::Missing => Err(ApiError::not_found("workspace")),
    }
}

#[utoipa::path(get, path = "/workspaces/{id}/config", operation_id = "workspaceConfig", responses((status = 200, body = crate::config::Config), (status = 404)))]
pub(super) async fn config(
    State(engine): State<Arc<Engine>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<crate::config::Config>, StatusCode> {
    let workspace = engine
        .store
        .workspace(&id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let path = crate::tool::canonical(std::path::Path::new(&workspace.path));
    let mut config = engine.workspace_config(&path);
    // As opencode starts a project's MCP servers when it opens, a workspace's stdio servers start when its config is first asked for.
    engine.start_workspace_mcp(&path);
    config.commands.extend(engine.mcp.prompt_commands(Some(&path)));
    Ok(Json(config))
}
