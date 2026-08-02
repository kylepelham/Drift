#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clipboard;
mod commands;
mod config;
mod editor;
mod engine;
mod engine_db;
mod mcp;
mod storage;
mod store;
mod updater;
mod voice;
mod watcher;

use config::ConfigRoot;
use engine::Engine;
use tauri::{Manager, RunEvent};
use voice::VoiceDownload;

/// Windows `CREATE_NO_WINDOW`: keeps spawned console processes from flashing a terminal.
#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn main() {
    // Reqwest is built without a bundled provider so the release build needs no extra C toolchain.
    let _ = rustls::crypto::ring::default_provider().install_default();
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.webview_windows().values().next() {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Engine::default())
        .manage(VoiceDownload::default())
        // Commands are named by full path: generate_handler! resolves helper macros in the
        // module that defines each command, so a plain `use` re-export is not enough.
        .invoke_handler(tauri::generate_handler![
            engine::engine_status,
            engine::restart_engine,
            updater::check_update,
            updater::install_update,
            updater::update_support,
            clipboard::clipboard_write_text,
            config::config_read,
            editor::pick_folder,
            editor::open_file,
            commands::store_workspaces,
            commands::store_removed_workspaces,
            commands::store_add_workspace,
            commands::store_save_workspace,
            commands::store_touch_workspace,
            commands::store_remove_workspace,
            commands::store_expired_removed_workspaces,
            commands::store_forget_workspace,
            commands::store_archived,
            commands::store_archive_session,
            commands::store_unarchive_session,
            commands::store_expired_archived,
            commands::store_interruptions,
            commands::store_save_interruption,
            commands::store_dismiss_interruption,
            commands::store_clear_interruptions,
            commands::mcp_snapshot,
            commands::prompt_snapshot,
            commands::prompt_save,
            commands::prompt_reset,
            commands::mcp_save,
            commands::mcp_remove,
            commands::mcp_approve,
            commands::mcp_reject,
            commands::mcp_revoke,
            commands::storage_stats,
            commands::storage_analyze,
            commands::storage_prune,
            commands::storage_compact,
            voice::voice_supported,
            voice::voice_acceleration,
            voice::voice_models,
            voice::voice_model_download,
            voice::voice_model_remove,
            voice::voice_model_cancel,
            voice::voice_transcribe
        ])
        .setup(|app| {
            let data_dir = app.path().app_data_dir().expect("no app data dir");
            let config_dir = app.path().app_config_dir().expect("no app config dir");
            std::fs::create_dir_all(&config_dir).expect("failed to create config dir");
            app.manage(ConfigRoot(config_dir));
            let shared_database = if cfg!(debug_assertions) {
                false
            } else {
                engine::engine_binary()
                    .map(|_| match engine_db::prepare_shared() {
                        Ok(imported) => {
                            if imported > 0 {
                                eprintln!("imported {imported} Drift database rows into the shared OpenCode database");
                            }
                            true
                        }
                        Err(error) => {
                            eprintln!("keeping Drift's channel database: {error}");
                            false
                        }
                    })
                    .unwrap_or(false)
            };
            let store = store::open(&data_dir).expect("failed to open drift store");
            if let Ok(database) = engine_db::database_path(shared_database) {
                if let Err(error) = store.import_opencode_workspaces(&database) {
                    eprintln!("failed to import OpenCode workspaces: {error}");
                }
            }
            let extensions = engine::engine_extensions().expect("embedded engine extensions not found");
            let mcp_runtime = mcp::McpRuntime::new(&data_dir, extensions);
            mcp_runtime
                .materialize(&store)
                .expect("failed to prepare Drift MCP policy");
            let engine_config = mcp_runtime.config_dir().to_path_buf();
            app.manage(store);
            app.manage(mcp_runtime);
            engine::spawn_engine(app.handle().clone(), shared_database, engine_config);
            watcher::watch_mcp_configs(app.handle().clone());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build drift")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                engine::stop_engine_on_exit(app);
            }
        });
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
