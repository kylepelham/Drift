//! Thin Tauri command wrappers over Store.
//!
//! Each exists only to adapt an error type into the String the frontend receives.

use crate::session_search::{self, SessionMatch};
use crate::storage::{self, PruneResult, StorageStats};
use crate::store::{ArchivedSession, Store, Workspace};
use tauri::State;

/// Sessions whose transcript contains `query`. Runs off the UI thread: the scan touches the
/// engine database, which the engine may be writing to at the same time.
#[tauri::command]
pub(crate) async fn session_search(native: State<'_, crate::native::Native>, query: String, directory: String) -> Result<Vec<SessionMatch>, String> {
    let database = native.engine().data_dir.join("drift.db");
    tauri::async_runtime::spawn_blocking(move || session_search::search(&database, &query, &directory))
        .await
        .map_err(|error| error.to_string())?
}

fn storage_location(native: &crate::native::Native) -> storage::Location {
    storage::Location { data_dir: native.engine().data_dir.clone() }
}

/// Fast, sampled overview of what is using space: the database and the engine's folders.
#[tauri::command]
pub(crate) async fn storage_stats(store: State<'_, Store>, native: State<'_, crate::native::Native>) -> Result<StorageStats, String> {
    let archived = storage::archived_ids(&store);
    let location = storage_location(&native);
    tauri::async_runtime::spawn_blocking(move || storage::stats(&location, &archived))
        .await
        .map_err(|error| error.to_string())?
}

/// The engine's housekeeping now: undo history and images nothing refers to, shell output past its week.
#[tauri::command]
pub(crate) async fn storage_prune(native: State<'_, crate::native::Native>) -> Result<PruneResult, String> {
    let location = storage_location(&native);
    let before = storage::total_bytes(&location);
    let images = native.engine().clean_up().await;
    tauri::async_runtime::spawn_blocking(move || storage::cleaned(&location, before, images))
        .await
        .map_err(|error| error.to_string())?
}

/// Gives the database's free pages back to the disk; refused while a conversation runs.
#[tauri::command]
pub(crate) async fn storage_compact(native: State<'_, crate::native::Native>) -> Result<PruneResult, String> {
    if native.engine().turns.any_running() {
        return Err("a conversation is running; compact once it finishes".into());
    }
    let location = storage_location(&native);
    tauri::async_runtime::spawn_blocking(move || storage::compact(&location))
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
    importer: State<crate::opencode_import::Importer>,
    id: String,
    path: String,
    name: String,
    icon: String,
) -> Result<Workspace, String> {
    let workspace = store
        .add_workspace(&id, &path, &name, &icon)
        .map_err(|e| e.to_string())?;
    importer.request();
    Ok(workspace)
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
pub(crate) fn store_remove_workspace(store: State<Store>, native: State<crate::native::Native>, id: String) -> Result<(), String> {
    native.engine().stop_workspace_mcp(&id);
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

/// The engine's records of a removed workspace (its kept permission grants and trusted commands),
/// then the shell's. Engine first, so a failure in between leaves the row to retry from.
#[tauri::command]
pub(crate) fn store_forget_workspace(store: State<Store>, native: State<crate::native::Native>, id: String) -> Result<(), String> {
    let removed = store.removed_workspaces().map_err(|e| e.to_string())?.iter().any(|workspace| workspace.id == id);
    if !removed {
        return Ok(());
    }
    native.engine().forget_workspace(&id).map_err(|e| e.to_string())?;
    store.forget_workspace(&id).map_err(|e| e.to_string())?;
    Ok(())
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
