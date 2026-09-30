use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

use crate::session::branch::BranchError;
use crate::session::revert::RevertError;
use crate::session::tree::TreeError;
use crate::session::turn::TurnError;

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub body: ErrorBody,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self { status, body: ErrorBody { code: code.into(), message: message.into() } }
    }

    pub fn not_found(what: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", format!("{what} not found"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(error: rusqlite::Error) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "store", error.to_string())
    }
}

impl From<TurnError> for ApiError {
    fn from(error: TurnError) -> Self {
        let (status, code) = match error {
            TurnError::NoSession | TurnError::NoWorkspace => (StatusCode::NOT_FOUND, "not_found"),
            TurnError::Busy => (StatusCode::CONFLICT, "busy"),
            TurnError::NotRetrying => (StatusCode::CONFLICT, "not_retrying"),
            TurnError::Reverted => (StatusCode::CONFLICT, "reverted"),
            TurnError::SubmissionReused => (StatusCode::CONFLICT, "submission"),
            TurnError::NoModel | TurnError::UnknownModel => (StatusCode::BAD_REQUEST, "model"),
            TurnError::NoCredentials => (StatusCode::UNAUTHORIZED, "credentials"),
            TurnError::Store(_) => (StatusCode::INTERNAL_SERVER_ERROR, "store"),
        };
        Self::new(status, code, error.to_string())
    }
}

impl From<BranchError> for ApiError {
    fn from(error: BranchError) -> Self {
        match error {
            BranchError::NoSession => Self::not_found("session"),
            BranchError::FromSubagent => Self::new(StatusCode::BAD_REQUEST, "subagent", "subagents cannot branch; branch from the conversation instead"),
            BranchError::BadCutoff => Self::new(StatusCode::BAD_REQUEST, "cutoff", "the cutoff is not a message in the source conversation"),
            BranchError::EmptyGoal => Self::new(StatusCode::BAD_REQUEST, "goal", "a branch needs a goal"),
            BranchError::Turn(error) => error.into(),
            BranchError::Draft(message) => Self::new(StatusCode::BAD_GATEWAY, "draft", message),
            BranchError::Store(error) => error.into(),
        }
    }
}

impl From<RevertError> for ApiError {
    fn from(error: RevertError) -> Self {
        match error {
            RevertError::NoSession => Self::not_found("session"),
            RevertError::NotAPrompt => Self::new(StatusCode::BAD_REQUEST, "not_a_prompt", "undo goes back to a prompt you sent"),
            RevertError::Busy => Self::new(StatusCode::CONFLICT, "busy", "stop the running turn first"),
            RevertError::Files(message) => Self::new(StatusCode::INTERNAL_SERVER_ERROR, "files", format!("the files could not be restored: {message}")),
            RevertError::Store(error) => error.into(),
        }
    }
}

impl From<TreeError> for ApiError {
    fn from(error: TreeError) -> Self {
        match error {
            TreeError::NoSession => Self::not_found("session"),
            TreeError::NoWorkspace => Self::not_found("workspace"),
            TreeError::Busy => Self::new(StatusCode::CONFLICT, "busy", "stop the running turn first; it keeps the workspace it started in"),
            TreeError::BadMessage => Self::new(StatusCode::BAD_REQUEST, "message", "fork from a finished message of this session"),
            TreeError::Empty => Self::new(StatusCode::BAD_REQUEST, "empty", "there is nothing finished to fork yet"),
            TreeError::Store(error) => error.into(),
        }
    }
}
