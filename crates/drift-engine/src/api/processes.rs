//! Background processes: what a conversation has running, its output, and stopping one from the dock.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::Serialize;
use utoipa::ToSchema;

use super::error::ApiError;
use crate::Engine;
use crate::background::ProcessRecord;

/// The conversation's background processes, oldest first, recently ended ones included.
#[utoipa::path(get, path = "/sessions/{id}/processes", operation_id = "listProcesses", responses((status = 200, body = Vec<ProcessRecord>)))]
pub(super) async fn list(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> Json<Vec<ProcessRecord>> {
    let records = engine.processes.of(&id).iter().map(|handle| handle.record()).collect();

    Json(records)
}

/// Stops a process and everything it started; its conversation is not told, since the user chose it.
#[utoipa::path(post, path = "/processes/{id}/stop", operation_id = "stopProcess", responses((status = 200, body = ProcessRecord), (status = 404)))]
pub(super) async fn stop(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
) -> Result<Json<ProcessRecord>, ApiError> {
    let handle = engine
        .processes
        .get(&id)
        .ok_or_else(|| ApiError::not_found("process"))?;
    handle.stop();

    Ok(Json(handle.record()))
}

/// How much of a process's newest output its row in the dock shows.
const OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Serialize, ToSchema)]
pub(super) struct ProcessOutput {
    /// The newest output, at most 64 KB, with colours and cursor moves taken out.
    output: String,
}

/// A process's newest output, for its row in the dock; reading it marks nothing read for the model.
#[utoipa::path(get, path = "/processes/{id}/output", operation_id = "processOutput", responses((status = 200, body = ProcessOutput), (status = 404)))]
pub(super) async fn output(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
) -> Result<Json<ProcessOutput>, ApiError> {
    let handle = engine
        .processes
        .get(&id)
        .ok_or_else(|| ApiError::not_found("process"))?;

    Ok(Json(ProcessOutput {
        output: handle.tail(OUTPUT_BYTES),
    }))
}
