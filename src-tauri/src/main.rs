#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clipboard;
mod commands;
mod config;
mod editor;
mod file_preview;
mod native;
mod opencode_import;
mod permissions;
mod prompts;
mod remote;
mod remote_auth;
mod remote_tls;
mod session_search;
mod startup;
mod storage;
mod store;
mod ui_state;
mod updater;
mod usage_limits;
mod voice;

use config::ConfigRoot;
use tauri::{Manager, RunEvent};
use voice::VoiceDownload;

/// Windows CREATE_NO_WINDOW flag; prevents spawned console processes from flashing a terminal.
#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Records the first placement of the launch window so it is centered only once.
static WINDOW_REVEALED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Places the launch window on screen before its reveal.
/// The preload reveals the hidden window after paint, independently of engine startup.
fn position_main_window(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    let Some(monitor) = window.primary_monitor()? else {
        return window.center();
    };

    let monitor_position = monitor.position();
    let monitor_size = monitor.size();
    let window_size = window.outer_size()?;
    let x = monitor_position.x as i64 + (monitor_size.width as i64 - window_size.width as i64) / 2;
    let y = monitor_position.y as i64 + (monitor_size.height as i64 - window_size.height as i64) / 2;

    window.set_position(tauri::PhysicalPosition::new(x as i32, y as i32))
}

fn reveal_main_window(window: &tauri::WebviewWindow) {
    if !WINDOW_REVEALED.load(std::sync::atomic::Ordering::SeqCst) && position_main_window(window).is_err() {
        return;
    }
    if window.show().is_err() {
        return;
    }

    if !WINDOW_REVEALED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        startup::mark("window-visible");
    }
    let _ = window.unminimize();
    let _ = window.set_focus();
}

#[tauri::command]
fn show_main_window(window: tauri::WebviewWindow) {
    reveal_main_window(&window);
}

#[tauri::command]
fn open_webview_devtools(window: tauri::WebviewWindow) {
    window.open_devtools();
}

fn main() {
    startup::mark("process-start");
    // Reqwest is built without a bundled provider so the release build needs no extra C toolchain.
    let _ = rustls::crypto::ring::default_provider().install_default();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if !WINDOW_REVEALED.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            if let Some(window) = app.webview_windows().values().next() {
                reveal_main_window(window);
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(VoiceDownload::default())
        .manage(permissions::DictationConsent::default())
        // generate_handler! resolves command helper macros in their defining module, not through ordinary re-exports.
        .invoke_handler(tauri::generate_handler![
            native::native_engine_status,
            opencode_import::opencode_import_summary,
            updater::check_update,
            updater::install_update,
            usage_limits::provider_usage,
            updater::update_support,
            clipboard::clipboard_write_text,
            config::config_read,
            editor::pick_folder,
            editor::open_file,
            editor::open_file_in_editor,
            file_preview::read_file_preview,
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
            prompts::prompt_snapshot,
            prompts::prompt_save,
            prompts::prompt_reset,
            show_main_window,
            open_webview_devtools,
            commands::session_search,
            commands::storage_stats,
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
            remote::access_commands::remote_access_status,
            remote::access_commands::remote_access_enable,
            remote::access_commands::remote_access_disable,
            remote::access_commands::remote_access_link,
            remote::access_commands::remote_access_revoke,
            remote::access_commands::remote_access_set_password,
            ui_state::ui_state_initialize,
            ui_state::ui_state_snapshot,
            ui_state::ui_state_update,
            ui_state::timeout::shell_timeout_initialize,
            ui_state::timeout::shell_timeout_snapshot,
            ui_state::timeout::shell_timeout_update
        ])
        .setup(setup)
        .build(tauri::generate_context!())
        .expect("failed to build drift")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                app.state::<remote::RemoteAccess>().stop_on_exit();
                native::stop(app);
            }
        });
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    startup::mark("setup-start");
    let launch_window = app
        .get_webview_window("main")
        .ok_or_else(|| std::io::Error::other("main window was not created"))?;
    let data_dir = app.path().app_data_dir().expect("no app data dir");
    let config_dir = app.path().app_config_dir().expect("no app config dir");
    std::fs::create_dir_all(&config_dir).expect("failed to create config dir");
    app.manage(ConfigRoot(config_dir));

    let engine = native::start(app.handle(), &data_dir).expect("failed to open the drift engine");
    let store = store::attach(engine.store.clone()).expect("failed to open drift store");
    native::push_agent_overrides(app.handle(), &store).expect("failed to load agent settings");
    let ui_state = ui_state::UiStateAuthority::load(&store).expect("failed to load UI mirror state");
    let shell_timeout = ui_state::ShellTimeoutAuthority::load(&store).expect("failed to load shell timeout policy");
    if let Some(policy) = shell_timeout.current() {
        native::push_shell_timeout(app.handle(), policy.timeout_ms);
    }

    let dictation_enabled = store.dictation_enabled().unwrap_or(false);
    app.state::<permissions::DictationConsent>().set(dictation_enabled);
    app.manage(store);
    app.manage(ui_state);
    app.manage(shell_timeout);
    app.manage(opencode_import::start(app.handle()));
    #[cfg(windows)]
    permissions::install(app)?;

    let remote_access = remote::RemoteAccess::load(&app.state::<store::Store>(), &data_dir)
        .expect("failed to load remote access settings");
    let start_remote = remote_access.should_start();
    app.manage(remote_access);
    if start_remote {
        let app = app.handle().clone();
        tauri::async_runtime::spawn(async move {
            let access = app.state::<remote::RemoteAccess>();
            if let Err(error) = access.start(app.clone()).await {
                access.set_error(error.to_string());
            }
        });
    }
    startup::mark("setup-complete");

    // Recover if the preload script never observes paint or cannot invoke the reveal.
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        if !WINDOW_REVEALED.load(std::sync::atomic::Ordering::SeqCst) {
            reveal_main_window(&launch_window);
        }
    });

    Ok(())
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
