use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use super::error::ApiError;
use crate::Engine;
use crate::session::turn::{Prompt, Receipt};
use crate::store::QueuedPrompt;

fn text_and_files(prompt: &Prompt) -> Result<(), ApiError> {
    // Worker results, answers and tool calls are the engine's to write, never a client's to claim.
    if prompt.parts.iter().any(crate::session::types::Part::is_engine_origin) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid",
            "a prompt carries text and files only",
        ));
    }

    Ok(())
}

/// The session's queue: prompts waiting for its turn to end, first to start first.
#[utoipa::path(get, path = "/sessions/{id}/queue", operation_id = "listQueue", responses((status = 200, body = Vec<QueuedPrompt>)))]
pub(super) async fn list(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<QueuedPrompt>>, ApiError> {
    Ok(Json(engine.queued_prompts(&id)?))
}

/// Queues a prompt to start a turn of its own once the running one ends; with none running it starts now.
#[utoipa::path(post, path = "/sessions/{id}/queue", operation_id = "queuePrompt", request_body = Prompt, responses((status = 201, body = QueuedPrompt), (status = 404)))]
pub(super) async fn add(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(prompt): Json<Prompt>,
) -> Result<(StatusCode, Json<QueuedPrompt>), ApiError> {
    text_and_files(&prompt)?;

    Ok((StatusCode::CREATED, Json(engine.queue_prompt(&id, prompt)?)))
}

/// Replaces a queued prompt's text and files, clearing why it could not start.
#[utoipa::path(put, path = "/queue/{id}", operation_id = "editQueued", request_body = Prompt, responses((status = 204), (status = 404)))]
pub(super) async fn edit(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(prompt): Json<Prompt>,
) -> Result<StatusCode, ApiError> {
    text_and_files(&prompt)?;
    if engine.edit_queued(&id, &prompt)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("queued prompt"))
    }
}

#[utoipa::path(delete, path = "/queue/{id}", operation_id = "discardQueued", responses((status = 204), (status = 404)))]
pub(super) async fn discard(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    if engine.discard_queued(&id)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("queued prompt"))
    }
}

/// Sends a queued prompt now, as a steer into the running turn or as a new turn when none runs.
#[utoipa::path(post, path = "/queue/{id}/send", operation_id = "sendQueuedNow", responses((status = 202, body = Receipt), (status = 404)))]
pub(super) async fn send_now(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Receipt>), ApiError> {
    match engine.send_queued_now(&id).await? {
        Some(receipt) => Ok((StatusCode::ACCEPTED, Json(receipt))),
        None => Err(ApiError::not_found("queued prompt")),
    }
}
