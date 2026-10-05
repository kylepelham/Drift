use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::error::ApiError;
use crate::session::compaction::AUTO_COMPACT_KEY;
use crate::session::tasks::BACKGROUND_TASKS_KEY;
use crate::Engine;

/// Engine-wide preferences the user changes in Settings.
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EngineSettings {
    /// Compact a conversation automatically when it nears its model's context window. Left out of a PUT, it stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_compact: Option<bool>,
    /// Let `task` run subagents in the background. Left out of a PUT, it stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_tasks: Option<bool>,
    /// Every session answers its own asks, except secrets and anything outside the workspace. Left out of a PUT, it stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_accept_all: Option<bool>,
}

fn current(engine: &Engine) -> EngineSettings {
    EngineSettings { auto_compact: Some(engine.auto_compact()), background_tasks: Some(engine.background_enabled()), auto_accept_all: Some(engine.auto_accept_all()) }
}

#[utoipa::path(get, path = "/settings", operation_id = "getSettings", responses((status = 200, body = EngineSettings)))]
pub async fn get(State(engine): State<Arc<Engine>>) -> Json<EngineSettings> {
    Json(current(&engine))
}

#[utoipa::path(put, path = "/settings", operation_id = "putSettings", request_body = EngineSettings, responses((status = 200, body = EngineSettings)))]
pub async fn put(State(engine): State<Arc<Engine>>, Json(body): Json<EngineSettings>) -> Result<Json<EngineSettings>, ApiError> {
    if let Some(enabled) = body.auto_compact {
        engine.store.set_setting(AUTO_COMPACT_KEY, &enabled)?;
    }
    if let Some(enabled) = body.background_tasks {
        engine.store.set_setting(BACKGROUND_TASKS_KEY, &enabled)?;
    }
    if let Some(on) = body.auto_accept_all {
        engine.set_auto_accept_all(on)?;
    }
    Ok(Json(current(&engine)))
}
