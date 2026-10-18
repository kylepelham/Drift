use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;

use super::error::ApiError;
use crate::Engine;
use crate::session::review::{GitSource, Review, ReviewError, ReviewRefs, git_refs, git_review};

impl From<ReviewError> for ApiError {
    fn from(error: ReviewError) -> Self {
        let message = error.to_string();
        match error {
            ReviewError::NoSession => Self::not_found("session"),
            ReviewError::NotAPrompt => Self::new(StatusCode::BAD_REQUEST, "not_a_prompt", message),
            ReviewError::NotARepo => Self::new(StatusCode::BAD_REQUEST, "not_a_repo", message),
            ReviewError::UnknownTarget(_) => Self::new(StatusCode::BAD_REQUEST, "target", message),
            ReviewError::Git(_) => Self::new(StatusCode::INTERNAL_SERVER_ERROR, "git", message),
            ReviewError::Store(error) => error.into(),
        }
    }
}

/// What the turn started by prompt `message` and its subagents changed, each file from before its first change to after its last.
#[utoipa::path(get, path = "/sessions/{id}/review/{message}", operation_id = "reviewTurn", responses((status = 200, body = Review), (status = 400), (status = 404)))]
pub(super) async fn turn(
    State(engine): State<Arc<Engine>>,
    Path((id, message)): Path<(String, String)>,
) -> Result<Json<Review>, ApiError> {
    Ok(Json(engine.turn_review(&id, &message).await?))
}

#[derive(Deserialize, utoipa::IntoParams)]
pub(super) struct ReviewQuery {
    #[param(inline)]
    pub source: GitSource,
    /// The branch for `branch`, the commit for `commit`.
    pub target: Option<String>,
}

/// The workspace's files against `HEAD`, where the current branch left another, or one commit's own changes.
#[utoipa::path(get, path = "/workspaces/{id}/review", operation_id = "reviewWorkspace", params(ReviewQuery), responses((status = 200, body = Review), (status = 400), (status = 404)))]
pub(super) async fn workspace(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Query(query): Query<ReviewQuery>,
) -> Result<Json<Review>, ApiError> {
    let root = root(&engine, &id)?;
    Ok(Json(git_review(&root, query.source, query.target.as_deref()).await?))
}

/// Whether the workspace is a git repository, with its branches and recent commits to review against.
#[utoipa::path(get, path = "/workspaces/{id}/review/refs", operation_id = "reviewRefs", responses((status = 200, body = ReviewRefs), (status = 404)))]
pub(super) async fn refs(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
) -> Result<Json<ReviewRefs>, ApiError> {
    let root = root(&engine, &id)?;
    Ok(Json(git_refs(&root).await))
}

fn root(engine: &Engine, id: &str) -> Result<std::path::PathBuf, ApiError> {
    let workspace = engine
        .store
        .workspace(id)?
        .ok_or_else(|| ApiError::not_found("workspace"))?;
    Ok(crate::tool::canonical(std::path::Path::new(&workspace.path)))
}
