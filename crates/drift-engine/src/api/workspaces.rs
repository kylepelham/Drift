use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::event::Event;
use crate::store::Workspace;
use crate::Engine;

#[derive(Serialize, Deserialize, ToSchema)]
pub struct NewWorkspace {
    pub path: String,
    pub name: String,
    #[serde(default)]
    pub icon: String,
}

#[utoipa::path(get, path = "/workspaces", operation_id = "listWorkspaces", responses((status = 200, body = Vec<Workspace>)))]
pub async fn list(State(engine): State<Arc<Engine>>) -> Result<Json<Vec<Workspace>>, StatusCode> {
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
pub async fn create(
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
pub struct FileQuery {
    /// What the user typed after `@`.
    #[serde(default)]
    pub query: String,
    /// At most this many paths; default 20.
    pub limit: Option<usize>,
}

/// Workspace paths for an @ mention, best match first (directories end in `/`). Names only: reading a
/// mentioned file is decided when the prompt is sent.
#[utoipa::path(get, path = "/workspaces/{id}/files", operation_id = "findFiles", params(FileQuery), responses((status = 200, body = Vec<String>), (status = 404)))]
pub async fn files(
    State(engine): State<Arc<Engine>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<FileQuery>,
) -> Result<Json<Vec<String>>, StatusCode> {
    let workspace = engine.store.workspace(&id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?.ok_or(StatusCode::NOT_FOUND)?;
    let root = crate::tool::canonical(std::path::Path::new(&workspace.path));
    let limit = query.limit.unwrap_or(20).clamp(1, 200);
    let found = tokio::task::spawn_blocking(move || crate::tool::glob::search_names(&root, &query.query, limit)).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(found))
}

#[utoipa::path(get, path = "/workspaces/{id}/config", operation_id = "workspaceConfig", responses((status = 200, body = crate::config::Config), (status = 404)))]
pub async fn config(State(engine): State<Arc<Engine>>, axum::extract::Path(id): axum::extract::Path<String>) -> Result<Json<crate::config::Config>, StatusCode> {
    let workspace = engine.store.workspace(&id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?.ok_or(StatusCode::NOT_FOUND)?;
    let path = crate::tool::canonical(std::path::Path::new(&workspace.path));
    Ok(Json(engine.workspace_config(&path)))
}
