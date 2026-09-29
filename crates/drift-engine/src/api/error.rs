use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

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
            TurnError::SubmissionReused => (StatusCode::CONFLICT, "submission"),
            TurnError::NoModel | TurnError::UnknownModel => (StatusCode::BAD_REQUEST, "model"),
            TurnError::NoCredentials => (StatusCode::UNAUTHORIZED, "credentials"),
            TurnError::Store(_) => (StatusCode::INTERNAL_SERVER_ERROR, "store"),
        };
        Self::new(status, code, error.to_string())
    }
}
