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
