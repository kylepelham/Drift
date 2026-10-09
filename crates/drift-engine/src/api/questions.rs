use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use utoipa::ToSchema;

use super::error::ApiError;
use crate::Engine;
use crate::question::Request;
use crate::session::clarify::AnswerError;

#[derive(Deserialize, ToSchema)]
pub(super) struct AnswerBody {
    /// One list of chosen labels per question, in order.
    pub answers: Vec<Vec<String>>,
}

#[utoipa::path(get, path = "/questions", operation_id = "listQuestions", responses((status = 200, body = Vec<Request>)))]
pub(super) async fn list(State(engine): State<Arc<Engine>>) -> Json<Vec<Request>> {
    Json(engine.questions.pending())
}

/// An async question's answer is saved before this returns; resending the same answer is accepted, a different one is 409.
#[utoipa::path(post, path = "/questions/{id}/reply", operation_id = "answerQuestion", request_body = AnswerBody, responses((status = 204), (status = 404), (status = 409)))]
pub(super) async fn reply(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(body): Json<AnswerBody>,
) -> Result<StatusCode, ApiError> {
    engine
        .answer_question(&id, Some(body.answers))
        .await
        .map_err(answer_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Dismissing a question whose answer was already saved is 409: the answer stands.
#[utoipa::path(post, path = "/questions/{id}/reject", operation_id = "rejectQuestion", responses((status = 204), (status = 404), (status = 409)))]
pub(super) async fn reject(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    engine.answer_question(&id, None).await.map_err(answer_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) fn answer_error(error: AnswerError) -> ApiError {
    match error {
        AnswerError::NotPending => ApiError::not_found("pending question"),
        AnswerError::Conflict => ApiError::new(
            StatusCode::CONFLICT,
            "answered",
            "this question was already answered differently",
        ),
        AnswerError::Turn(error) => error.into(),
    }
}
