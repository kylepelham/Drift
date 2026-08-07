#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clipboard;
mod commands;
mod config;
mod editor;
mod engine;
mod engine_db;
mod mcp;
mod permissions;
mod remote;
mod storage;
mod store;
mod ui_state;
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

/// Set once the launch window has been placed on screen, so it is only centered once.
static WINDOW_REVEALED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Brings the launch window on screen.
///
/// Tauri creates configured windows before `setup` runs, so hiding there still flashes an
/// unpainted rectangle. The window is instead configured off-screen and hidden, then centered
/// here on its first reveal. Creating it with `visible: false` would be the obvious alternative,
/// but that path can break Tauri's outbound event channel on Windows.
fn position_main_window(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    let Some(monitor) = window.primary_monitor()? else {
        return window.center();
    };
    let monitor_position = monitor.position();
    let monitor_size = monitor.size();
    let window_size = window.outer_size()?;
    let x = monitor_position.x as i64 + (monitor_size.width as i64 - window_size.width as i64) / 2;
    let y =
        monitor_position.y as i64 + (monitor_size.height as i64 - window_size.height as i64) / 2;
    window.set_position(tauri::PhysicalPosition::new(x as i32, y as i32))
}

fn reveal_main_window(window: &tauri::WebviewWindow) {
    if !WINDOW_REVEALED.load(std::sync::atomic::Ordering::SeqCst)
        && position_main_window(window).is_err()
    {
        return;
    }
    if window.show().is_err() {
        return;
    }
    WINDOW_REVEALED.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = window.unminimize();
    let _ = window.set_focus();
}

#[tauri::command]
fn show_main_window(window: tauri::WebviewWindow) {
    reveal_main_window(&window);
}

fn main() {
    // Reqwest is built without a bundled provider so the release build needs no extra C toolchain.
    let _ = rustls::crypto::ring::default_provider().install_default();
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.webview_windows().values().next() {
                reveal_main_window(window);
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Engine::default())
        .manage(VoiceDownload::default())
        .manage(permissions::DictationConsent::default())
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
            show_main_window,
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
            voice::voice_transcribe,
            permissions::voice_dictation_set_enabled,
            remote::remote_access_status,
            remote::remote_access_enable,
            remote::remote_access_disable,
            remote::remote_access_rotate_token,
            remote::remote_access_urls,
            ui_state::ui_state_initialize,
            ui_state::ui_state_snapshot,
            ui_state::ui_state_update,
            ui_state::shell_timeout_initialize,
            ui_state::shell_timeout_snapshot,
            ui_state::shell_timeout_update
        ])
        .setup(|app| {
            let launch_window = app
                .get_webview_window("main")
                .ok_or_else(|| std::io::Error::other("main window was not created"))?;
            let _ = launch_window.hide();
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
            let ui_state = ui_state::UiStateAuthority::load(&store)
                .expect("failed to load UI mirror state");
            let shell_timeout = ui_state::ShellTimeoutAuthority::load(&store)
                .expect("failed to load shell timeout policy");
            let dictation_enabled = store.dictation_enabled().unwrap_or(false);
            app.state::<permissions::DictationConsent>()
                .set(dictation_enabled);
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
            app.manage(ui_state);
            app.manage(shell_timeout);
            app.manage(mcp_runtime);
            #[cfg(windows)]
            permissions::install(app)?;
            let remote_access = remote::RemoteAccess::load(&app.state::<store::Store>())
                .expect("failed to load remote access settings");
            let start_remote = remote_access.should_start();
            app.manage(remote_access);
            engine::spawn_engine(app.handle().clone(), shared_database, engine_config);
            watcher::watch_mcp_configs(app.handle().clone());
            if start_remote {
                let app = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    for _ in 0..150 {
                        if app.state::<Engine>().current_url().is_some() {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    }
                    let access = app.state::<remote::RemoteAccess>();
                    if let Err(error) = access.start(app.clone()).await {
                        access.set_error(error);
                    }
                });
            }
            // Once setup releases the event loop, recover from a preload script that failed to
            // invoke `show_main_window`. Normal startup reveals much earlier, after its first paint.
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                if !WINDOW_REVEALED.load(std::sync::atomic::Ordering::SeqCst) {
                    reveal_main_window(&launch_window);
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build drift")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                app.state::<remote::RemoteAccess>().stop_on_exit();
                engine::stop_engine_on_exit(app);
            }
        });
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
