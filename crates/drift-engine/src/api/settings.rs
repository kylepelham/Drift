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
    /// Every session answers its own asks; only a deny rule still refuses. Left out of a PUT, it stays as it is.
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

#[derive(Deserialize, utoipa::IntoParams)]
pub struct ToolsQuery {
    /// The workspace whose MCP servers' tools to include besides the built-ins.
    pub workspace: Option<String>,
}

/// A tool an agent's `tools` list can name, as Settings offers it.
#[derive(Serialize, ToSchema)]
pub struct ToolName {
    pub name: String,
    /// The MCP server it comes from; none for a built-in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
}

/// Every tool an agent could be offered: the built-ins of both tool profiles, then the workspace's MCP tools.
#[utoipa::path(get, path = "/tools", operation_id = "listTools", params(ToolsQuery), responses((status = 200, body = Vec<ToolName>)))]
pub async fn tools(State(engine): State<Arc<Engine>>, axum::extract::Query(query): axum::extract::Query<ToolsQuery>) -> Json<Vec<ToolName>> {
    use crate::llm::catalog::ToolProfile;
    let workspace = query.workspace.as_deref().and_then(|id| engine.store.workspace(id).ok().flatten()).map(|w| crate::tool::canonical(std::path::Path::new(&w.path)));
    let builtin = engine.tools.offered(ToolProfile::Edit).into_iter().chain(engine.tools.offered(ToolProfile::ApplyPatch));
    let mcp = engine.mcp.tools(&engine.store, workspace.as_deref());
    let mut names: Vec<ToolName> = Vec::new();
    for tool in builtin.chain(mcp) {
        let name = tool.spec().name;
        if !names.iter().any(|known| known.name == name) {
            names.push(ToolName { name, server: tool.server().map(str::to_string) });
        }
    }
    Json(names)
}
