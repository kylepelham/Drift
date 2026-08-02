//! Thin Tauri command wrappers over Store and McpRuntime.
//!
//! Each exists only to adapt an error type into the String the frontend receives.

use crate::engine::stop_engine_instances;
use crate::mcp;
use crate::storage::{self, PruneResult, PruneRules, RuleEstimate, StorageStats};
use crate::store::{ArchivedSession, Store, Workspace};
use serde_json::Value;
use tauri::State;

/// Fast, sampled overview of what is using space in the session database.
#[tauri::command]
pub(crate) async fn storage_stats(store: State<'_, Store>) -> Result<StorageStats, String> {
    let archived = storage::archived_ids(&store);
    tauri::async_runtime::spawn_blocking(move || storage::stats(&archived))
        .await
        .map_err(|error| error.to_string())?
}

/// Exact reclaimable space per rule. Scans the event table, so callers should show progress.
#[tauri::command]
pub(crate) async fn storage_analyze(store: State<'_, Store>) -> Result<Vec<RuleEstimate>, String> {
    let archived = storage::archived_ids(&store);
    tauri::async_runtime::spawn_blocking(move || storage::analyze(&archived))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn storage_prune(store: State<'_, Store>, rules: PruneRules) -> Result<PruneResult, String> {
    let archived = storage::archived_ids(&store);
    tauri::async_runtime::spawn_blocking(move || storage::prune(rules, &archived))
        .await
        .map_err(|error| error.to_string())?
}

/// Releases free pages back to the filesystem. Fails while the engine holds the database.
#[tauri::command]
pub(crate) async fn storage_compact() -> Result<PruneResult, String> {
    tauri::async_runtime::spawn_blocking(storage::compact)
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) fn store_workspaces(store: State<Store>) -> Result<Vec<Workspace>, String> {
    store.workspaces().map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn store_removed_workspaces(store: State<Store>) -> Result<Vec<Workspace>, String> {
    store.removed_workspaces().map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn store_add_workspace(
    store: State<Store>,
    id: String,
    path: String,
    name: String,
    icon: String,
) -> Result<Workspace, String> {
    store
        .add_workspace(&id, &path, &name, &icon)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn store_save_workspace(
    store: State<Store>,
    id: String,
    path: String,
    name: String,
    icon: String,
) -> Result<(), String> {
    store
        .save_workspace(&id, &path, &name, &icon)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn store_touch_workspace(store: State<Store>, id: String) -> Result<(), String> {
    store.touch_workspace(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn store_remove_workspace(store: State<Store>, id: String) -> Result<(), String> {
    store.remove_workspace(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn store_expired_removed_workspaces(
    store: State<Store>,
    before: i64,
) -> Result<Vec<Workspace>, String> {
    store
        .expired_removed_workspaces(before)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn store_forget_workspace(store: State<Store>, id: String) -> Result<(), String> {
    store.forget_workspace(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn store_archived(store: State<Store>) -> Result<Vec<ArchivedSession>, String> {
    store.archived().map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn store_archive_session(
    store: State<Store>,
    session_id: String,
    workspace_id: String,
) -> Result<(), String> {
    store
        .archive_session(&session_id, &workspace_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn store_unarchive_session(store: State<Store>, session_id: String) -> Result<(), String> {
    store
        .unarchive_session(&session_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn store_expired_archived(store: State<Store>, before: i64) -> Result<Vec<String>, String> {
    store.expired_archived(before).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn mcp_snapshot(
    runtime: State<mcp::McpRuntime>,
    store: State<Store>,
    directory: String,
) -> Result<mcp::McpSnapshot, String> {
    runtime.snapshot(&store, &directory)
}

#[tauri::command]
pub(crate) fn prompt_snapshot(
    runtime: State<mcp::McpRuntime>,
    store: State<Store>,
) -> Result<mcp::PromptSnapshot, String> {
    runtime.prompt_snapshot(&store)
}

#[tauri::command]
pub(crate) fn prompt_save(
    runtime: State<mcp::McpRuntime>,
    store: State<Store>,
    key: String,
    value: Value,
    original: Option<Value>,
) -> Result<(), String> {
    runtime.save_prompt(&store, &key, value, original)
}

#[tauri::command]
pub(crate) fn prompt_reset(
    runtime: State<mcp::McpRuntime>,
    store: State<Store>,
    key: String,
) -> Result<(), String> {
    runtime.reset_prompt(&store, &key)
}

#[tauri::command]
pub(crate) fn mcp_save(
    app: tauri::AppHandle,
    runtime: State<mcp::McpRuntime>,
    store: State<Store>,
    name: String,
    previous_name: Option<String>,
    config: Value,
    generation: i64,
) -> Result<(), String> {
    runtime.save(
        &store,
        &name,
        previous_name.as_deref(),
        config,
        generation,
        || stop_engine_instances(&app),
    )
}

#[tauri::command]
pub(crate) fn mcp_remove(
    app: tauri::AppHandle,
    runtime: State<mcp::McpRuntime>,
    store: State<Store>,
    name: String,
    generation: i64,
) -> Result<(), String> {
    runtime.remove(&store, &name, generation, || stop_engine_instances(&app))
}

#[tauri::command]
pub(crate) fn mcp_approve(
    app: tauri::AppHandle,
    runtime: State<mcp::McpRuntime>,
    store: State<Store>,
    directory: String,
    name: String,
    fingerprint: String,
    generation: i64,
) -> Result<(), String> {
    runtime.decide(
        &store,
        &directory,
        &name,
        &fingerprint,
        generation,
        mcp::McpDecision::Approved,
        || stop_engine_instances(&app),
    )
}

#[tauri::command]
pub(crate) fn mcp_reject(
    app: tauri::AppHandle,
    runtime: State<mcp::McpRuntime>,
    store: State<Store>,
    directory: String,
    name: String,
    fingerprint: String,
    generation: i64,
) -> Result<(), String> {
    runtime.decide(
        &store,
        &directory,
        &name,
        &fingerprint,
        generation,
        mcp::McpDecision::Rejected,
        || stop_engine_instances(&app),
    )
}

#[tauri::command]
pub(crate) fn mcp_revoke(
    app: tauri::AppHandle,
    runtime: State<mcp::McpRuntime>,
    store: State<Store>,
    directory: String,
    name: String,
    fingerprint: String,
    generation: i64,
) -> Result<(), String> {
    runtime.revoke(&store, &directory, &name, &fingerprint, generation, || {
        stop_engine_instances(&app)
    })
}
