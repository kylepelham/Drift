use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::error::ApiError;
use crate::Engine;
use crate::session::compaction::AUTO_COMPACT_KEY;
use crate::session::tasks::{BACKGROUND_TASKS_KEY, LimitError};

/// Engine-wide preferences the user changes in Settings.
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct EngineSettings {
    /// Compact a conversation automatically when it nears its model's context window. Left out of a PUT, it stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_compact: Option<bool>,
    /// Let `task` run subagents in the background. Left out of a PUT, it stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_tasks: Option<bool>,
    /// How many background subagents run at once, 1 to 16; more wait for a slot. Left out of a PUT, it stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_task_limit: Option<usize>,
    /// Every session answers its own asks; only a deny rule still refuses. Left out of a PUT, it stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_accept_all: Option<bool>,
    /// Registries besides the built-in ones, for a team's own plugins, skills and MCP servers. Left out of a PUT, they stay as they are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_sources: Option<Vec<RegistrySource>>,
}

use crate::config::sources::{RegistrySource, SourceInput};

/// The same settings, with each source's token to store or clear, for a PUT.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct EngineSettingsInput {
    #[serde(default)]
    pub auto_compact: Option<bool>,
    #[serde(default)]
    pub background_tasks: Option<bool>,
    #[serde(default)]
    pub background_task_limit: Option<usize>,
    #[serde(default)]
    pub auto_accept_all: Option<bool>,
    #[serde(default)]
    pub registry_sources: Option<Vec<SourceInput>>,
}

fn current(engine: &Engine) -> EngineSettings {
    EngineSettings {
        auto_compact: Some(engine.auto_compact()),
        background_tasks: Some(engine.background_enabled()),
        background_task_limit: Some(engine.background_limit()),
        auto_accept_all: Some(engine.auto_accept_all()),
        registry_sources: Some(engine.registry_sources()),
    }
}

#[utoipa::path(get, path = "/settings", operation_id = "getSettings", responses((status = 200, body = EngineSettings)))]
pub(super) async fn get(State(engine): State<Arc<Engine>>) -> Json<EngineSettings> {
    Json(current(&engine))
}

#[utoipa::path(put, path = "/settings", operation_id = "putSettings", request_body = EngineSettingsInput, responses((status = 200, body = EngineSettings)))]
pub(super) async fn put(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<EngineSettingsInput>,
) -> Result<Json<EngineSettings>, ApiError> {
    if let Some(enabled) = body.auto_compact {
        engine.store.set_setting(AUTO_COMPACT_KEY, &enabled)?;
    }
    if let Some(enabled) = body.background_tasks {
        engine.store.set_setting(BACKGROUND_TASKS_KEY, &enabled)?;
    }
    if let Some(limit) = body.background_task_limit {
        engine.set_background_limit(limit).map_err(|error| match error {
            LimitError::OutOfRange => {
                ApiError::new(StatusCode::BAD_REQUEST, "background_task_limit", error.to_string())
            }
            LimitError::Store(error) => error.into(),
        })?;
    }
    if let Some(on) = body.auto_accept_all {
        engine.set_auto_accept_all(on)?;
    }
    if let Some(sources) = body.registry_sources {
        engine
            .set_registry_sources(sources)
            .map_err(|error| ApiError::new(axum::http::StatusCode::BAD_REQUEST, "source", error.to_string()))?;
    }
    Ok(Json(current(&engine)))
}

#[derive(Deserialize, utoipa::IntoParams)]
pub(super) struct ToolsQuery {
    /// The workspace whose MCP servers' tools to include besides the built-ins.
    pub workspace: Option<String>,
}

/// A tool an agent's `tools` list can name, as Settings offers it.
#[derive(Serialize, ToSchema)]
pub(super) struct ToolName {
    pub name: String,
    /// The MCP server it comes from; none for a built-in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
}

/// Every tool an agent could be offered: the built-ins of both tool profiles, then the workspace's MCP tools.
#[utoipa::path(get, path = "/tools", operation_id = "listTools", params(ToolsQuery), responses((status = 200, body = Vec<ToolName>)))]
pub(super) async fn tools(
    State(engine): State<Arc<Engine>>,
    axum::extract::Query(query): axum::extract::Query<ToolsQuery>,
) -> Json<Vec<ToolName>> {
    use crate::llm::catalog::ToolProfile;
    let workspace = query
        .workspace
        .as_deref()
        .and_then(|id| engine.store.workspace(id).ok().flatten())
        .map(|w| crate::tool::canonical(std::path::Path::new(&w.path)));
    let builtin = engine
        .tools
        .offered(ToolProfile::Edit)
        .into_iter()
        .chain(engine.tools.offered(ToolProfile::ApplyPatch));
    let mcp = engine.mcp.tools(&engine.store, workspace.as_deref());
    let mut names: Vec<ToolName> = Vec::new();
    for tool in builtin.chain(mcp) {
        let name = tool.spec().name;
        if !names.iter().any(|known| known.name == name) {
            names.push(ToolName {
                name,
                server: tool.server().map(str::to_string),
            });
        }
    }
    Json(names)
}

#[derive(Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub(super) struct SourceQuery {
    /// The source's id.
    pub source: String,
}

/// A source's registry document, read by the engine with the source's token and trust settings.
#[utoipa::path(get, path = "/registries/fetch", operation_id = "fetchRegistry", params(SourceQuery), responses((status = 200, body = serde_json::Value)))]
pub(super) async fn fetch_registry(
    State(engine): State<Arc<Engine>>,
    axum::extract::Query(query): axum::extract::Query<SourceQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let source = engine
        .registry_source(&query.source)
        .ok_or_else(|| ApiError::not_found("source"))?;
    engine
        .fetcher()
        .document(&source)
        .await
        .map(Json)
        .map_err(|error| ApiError::new(axum::http::StatusCode::BAD_GATEWAY, "source", error))
}

/// The plugins the user's drift.json lists, loaded or with why they are not.
#[utoipa::path(get, path = "/plugins", operation_id = "listPlugins", responses((status = 200, body = Vec<crate::hook::PluginInfo>)))]
pub(super) async fn plugins(State(engine): State<Arc<Engine>>) -> Json<Vec<crate::hook::PluginInfo>> {
    Json(engine.hooks.loaded())
}

/// Reads drift.json again and loads every plugin afresh, so an edited one runs without a restart.
#[utoipa::path(post, path = "/plugins/reload", operation_id = "reloadPlugins", responses((status = 200, body = Vec<crate::hook::PluginInfo>)))]
pub(super) async fn reload_plugins(State(engine): State<Arc<Engine>>) -> Json<Vec<crate::hook::PluginInfo>> {
    Json(engine.reload_plugins().await)
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct PluginEnabled {
    /// The plugin's entry in drift.json.
    pub path: String,
    pub enabled: bool,
}

/// Installs a plugin from a registry: fetched over https, checked against the hash, listed in drift.json.
#[utoipa::path(post, path = "/plugins/install", operation_id = "installPlugin", request_body = crate::config::plugins::Install, responses((status = 200, body = Vec<crate::hook::PluginInfo>)))]
pub(super) async fn install_plugin(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<crate::config::plugins::Install>,
) -> Result<Json<Vec<crate::hook::PluginInfo>>, ApiError> {
    engine
        .install_plugin(body)
        .await
        .map(Json)
        .map_err(|error| ApiError::new(axum::http::StatusCode::BAD_REQUEST, "plugin", error.to_string()))
}

#[derive(Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub(super) struct PluginPath {
    /// The plugin's entry in drift.json.
    pub path: String,
}

/// Removes a plugin: its drift.json entry and, for one under the plugins directory, its component.
#[utoipa::path(delete, path = "/plugins", operation_id = "removePlugin", params(PluginPath), responses((status = 200, body = Vec<crate::hook::PluginInfo>)))]
pub(super) async fn remove_plugin(
    State(engine): State<Arc<Engine>>,
    axum::extract::Query(query): axum::extract::Query<PluginPath>,
) -> Result<Json<Vec<crate::hook::PluginInfo>>, ApiError> {
    engine
        .remove_plugin(&query.path)
        .await
        .map(Json)
        .map_err(|error| ApiError::new(axum::http::StatusCode::BAD_REQUEST, "plugin", error.to_string()))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct PluginConfig {
    pub path: String,
    pub config: serde_json::Value,
}

/// Replaces a plugin's config object in drift.json.
#[utoipa::path(put, path = "/plugins/config", operation_id = "configurePlugin", request_body = PluginConfig, responses((status = 200, body = Vec<crate::hook::PluginInfo>)))]
pub(super) async fn configure_plugin(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<PluginConfig>,
) -> Result<Json<Vec<crate::hook::PluginInfo>>, ApiError> {
    engine
        .configure_plugin(&body.path, body.config)
        .await
        .map(Json)
        .map_err(|error| ApiError::new(axum::http::StatusCode::BAD_REQUEST, "plugin", error.to_string()))
}

/// The skill packs installed from a registry.
#[utoipa::path(get, path = "/skills/packs", operation_id = "listSkillPacks", responses((status = 200, body = Vec<crate::config::skills::Pack>)))]
pub(super) async fn skill_packs() -> Json<Vec<crate::config::skills::Pack>> {
    Json(crate::config::skills::list())
}

/// Installs a skill pack: its archive is fetched over https and the asked folders are unpacked under the user's skills.
#[utoipa::path(post, path = "/skills/packs", operation_id = "installSkillPack", request_body = crate::config::skills::InstallPack, responses((status = 200, body = Vec<crate::config::skills::Pack>)))]
pub(super) async fn install_skill_pack(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<crate::config::skills::InstallPack>,
) -> Result<Json<Vec<crate::config::skills::Pack>>, ApiError> {
    let source = body.registry.as_deref().and_then(|id| engine.registry_source(id));
    crate::config::skills::install(&engine.fetcher(), source.as_ref(), body)
        .await
        .map_err(|error| ApiError::new(axum::http::StatusCode::BAD_REQUEST, "pack", error))?;
    Ok(Json(crate::config::skills::list()))
}

#[derive(Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub(super) struct PackId {
    pub id: String,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub(super) struct SkillsQuery {
    /// The workspace whose own skills to include besides the user's.
    pub workspace: Option<String>,
}

fn workspace_path(engine: &Engine, id: Option<&str>) -> Option<std::path::PathBuf> {
    id.and_then(|id| engine.store.workspace(id).ok().flatten())
        .map(|workspace| std::path::PathBuf::from(workspace.path))
}

/// Every skill the engine offers, packs and the workspace's included, and every one switched off.
#[utoipa::path(get, path = "/skills", operation_id = "listSkills", params(SkillsQuery), responses((status = 200, body = Vec<crate::config::skills::UserSkill>)))]
pub(super) async fn skills(
    State(engine): State<Arc<Engine>>,
    axum::extract::Query(query): axum::extract::Query<SkillsQuery>,
) -> Json<Vec<crate::config::skills::UserSkill>> {
    Json(crate::config::skills::list_skills(
        workspace_path(&engine, query.workspace.as_deref()).as_deref(),
        &engine.disabled_skills(),
    ))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SkillEnabled {
    /// The skill's folder, as listed.
    pub path: String,
    pub enabled: bool,
    /// The workspace the skill belongs to, for one of its own.
    #[serde(default)]
    pub workspace: Option<String>,
}

/// Turns a skill on or off; off, the model is never offered it.
#[utoipa::path(put, path = "/skills/enabled", operation_id = "setSkillEnabled", request_body = SkillEnabled, responses((status = 200, body = Vec<crate::config::skills::UserSkill>)))]
pub(super) async fn set_skill_enabled(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<SkillEnabled>,
) -> Result<Json<Vec<crate::config::skills::UserSkill>>, ApiError> {
    let workspace = workspace_path(&engine, body.workspace.as_deref());
    let folder = crate::config::skills::skill_folder(&body.path, workspace.as_deref())
        .map_err(|error| ApiError::new(axum::http::StatusCode::BAD_REQUEST, "skill", error))?;
    engine.set_skill_enabled(&folder, body.enabled)?;
    Ok(Json(crate::config::skills::list_skills(
        workspace.as_deref(),
        &engine.disabled_skills(),
    )))
}

/// Removes a skill pack and every skill it brought.
#[utoipa::path(delete, path = "/skills/packs", operation_id = "removeSkillPack", params(PackId), responses((status = 200, body = Vec<crate::config::skills::Pack>)))]
pub(super) async fn remove_skill_pack(
    axum::extract::Query(query): axum::extract::Query<PackId>,
) -> Result<Json<Vec<crate::config::skills::Pack>>, ApiError> {
    crate::config::skills::remove(&query.id)
        .map_err(|error| ApiError::new(axum::http::StatusCode::BAD_REQUEST, "pack", error))?;
    Ok(Json(crate::config::skills::list()))
}

/// Switches one plugin on or off; off, it stays listed and runs nothing.
#[utoipa::path(put, path = "/plugins/enabled", operation_id = "setPluginEnabled", request_body = PluginEnabled, responses((status = 200, body = Vec<crate::hook::PluginInfo>)))]
pub(super) async fn set_plugin_enabled(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<PluginEnabled>,
) -> Result<Json<Vec<crate::hook::PluginInfo>>, ApiError> {
    Ok(Json(engine.set_plugin_enabled(&body.path, body.enabled).await?))
}
