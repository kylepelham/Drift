use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::error::ApiError;
use crate::session::compaction::AUTO_COMPACT_KEY;
use crate::Engine;

/// Engine-wide preferences the user changes in Settings.
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EngineSettings {
    /// Compact a conversation automatically when it nears its model's context window.
    pub auto_compact: bool,
}

#[utoipa::path(get, path = "/settings", operation_id = "getSettings", responses((status = 200, body = EngineSettings)))]
pub async fn get(State(engine): State<Arc<Engine>>) -> Json<EngineSettings> {
    Json(EngineSettings { auto_compact: engine.auto_compact() })
}

#[utoipa::path(put, path = "/settings", operation_id = "putSettings", request_body = EngineSettings, responses((status = 200, body = EngineSettings)))]
pub async fn put(State(engine): State<Arc<Engine>>, Json(body): Json<EngineSettings>) -> Result<Json<EngineSettings>, ApiError> {
    engine.store.set_setting(AUTO_COMPACT_KEY, &body.auto_compact)?;
    Ok(Json(EngineSettings { auto_compact: engine.auto_compact() }))
}
