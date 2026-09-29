use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use utoipa::ToSchema;

use super::error::ApiError;
use crate::question::Request;
use crate::Engine;

#[derive(Deserialize, ToSchema)]
pub struct AnswerBody {
    /// One list of chosen labels per question, in order.
    pub answers: Vec<Vec<String>>,
}

#[utoipa::path(get, path = "/questions", operation_id = "listQuestions", responses((status = 200, body = Vec<Request>)))]
pub async fn list(State(engine): State<Arc<Engine>>) -> Json<Vec<Request>> {
    Json(engine.questions.pending())
}

#[utoipa::path(post, path = "/questions/{id}/reply", operation_id = "answerQuestion", request_body = AnswerBody, responses((status = 204), (status = 404)))]
pub async fn reply(State(engine): State<Arc<Engine>>, Path(id): Path<String>, Json(body): Json<AnswerBody>) -> Result<StatusCode, ApiError> {
    engine.questions.reply(&engine.hub, &id, Some(body.answers)).map_err(|_| ApiError::not_found("pending question"))?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/questions/{id}/reject", operation_id = "rejectQuestion", responses((status = 204), (status = 404)))]
pub async fn reject(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    engine.questions.reply(&engine.hub, &id, None).map_err(|_| ApiError::not_found("pending question"))?;
    Ok(StatusCode::NO_CONTENT)
}
