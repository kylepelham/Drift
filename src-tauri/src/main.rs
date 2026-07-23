#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engine_db;
mod store;

use serde::Serialize;
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use store::{ArchivedSession, Store, Workspace};
use tauri::{Manager, RunEvent, State};

#[derive(Default)]
struct Engine {
    url: Mutex<Option<String>>,
    child: Mutex<Option<Child>>,
    diagnostic: Mutex<String>,
}

#[derive(Serialize)]
struct EngineStatus {
    url: Option<String>,
    error: Option<String>,
}

struct ConfigRoot(PathBuf);

fn config_path(root: &Path, path: &str) -> Result<PathBuf, String> {
    let relative = Path::new(path);
    if relative
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("config path must be relative".into());
    }
    Ok(root.join(relative))
}

#[tauri::command]
fn config_read(config: State<ConfigRoot>, path: String) -> Result<Option<String>, String> {
    let root = config.0.canonicalize().map_err(|e| e.to_string())?;
    let requested = config_path(&root, &path)?;
    if !requested.exists() {
        return Ok(None);
    }
    let requested = requested.canonicalize().map_err(|e| e.to_string())?;
    if !requested.starts_with(&root) {
        return Err("config path escapes Drift's config directory".into());
    }
    if requested.metadata().map_err(|e| e.to_string())?.len() > 1_048_576 {
        return Err("config file exceeds 1 MiB".into());
    }
    std::fs::read_to_string(requested)
        .map(Some)
        .map_err(|e| e.to_string())
}

// Update checks compare semver against the release manifest; the plugin only offers
// strictly newer versions, so dev builds ahead of the latest release stay put.
#[tauri::command]
async fn check_update(app: tauri::AppHandle) -> Result<Option<String>, String> {
    if cfg!(debug_assertions) {
        return Ok(None);
    }
    let updater = tauri_plugin_updater::UpdaterExt::updater(&app).map_err(|e| e.to_string())?;
    let update = updater.check().await.map_err(|e| e.to_string())?;
    Ok(update.map(|u| u.version))
}

#[tauri::command]
async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    let updater = tauri_plugin_updater::UpdaterExt::updater(&app).map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or("no update available")?;
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    app.restart();
}

#[tauri::command]
fn engine_status(engine: State<Engine>) -> EngineStatus {
    let url = engine.url.lock().unwrap().clone();
    if url.is_some() {
        return EngineStatus { url, error: None };
    }
    let (has_child, status) = {
        let mut child = engine.child.lock().unwrap();
        (child.is_some(), child.as_mut().and_then(|child| child.try_wait().ok().flatten()))
    };
    let diagnostic = engine.diagnostic.lock().unwrap();
    let error = status
        .map(|status| {
            if diagnostic.is_empty() {
                format!("embedded engine exited with {status}")
            } else {
                format!("embedded engine exited with {status}: {diagnostic}")
            }
        })
        .or_else(|| (!has_child && !diagnostic.is_empty()).then(|| diagnostic.clone()));
    EngineStatus { url: None, error }
}

#[tauri::command]
async fn pick_folder() -> Option<String> {
    rfd::FileDialog::new()
        .pick_folder()
        .map(|path| path.to_string_lossy().to_string())
}

#[tauri::command]
fn store_workspaces(store: State<Store>) -> Result<Vec<Workspace>, String> {
    store.workspaces().map_err(|e| e.to_string())
}

#[tauri::command]
fn store_removed_workspaces(store: State<Store>) -> Result<Vec<Workspace>, String> {
    store.removed_workspaces().map_err(|e| e.to_string())
}

#[tauri::command]
fn store_add_workspace(
    store: State<Store>,
    id: String,
    path: String,
    name: String,
    icon: String,
) -> Result<Workspace, String> {
    store.add_workspace(&id, &path, &name, &icon).map_err(|e| e.to_string())
}

#[tauri::command]
fn store_save_workspace(
    store: State<Store>,
    id: String,
    path: String,
    name: String,
    icon: String,
) -> Result<(), String> {
    store.save_workspace(&id, &path, &name, &icon).map_err(|e| e.to_string())
}

#[tauri::command]
fn store_touch_workspace(store: State<Store>, id: String) -> Result<(), String> {
    store.touch_workspace(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn store_remove_workspace(store: State<Store>, id: String) -> Result<(), String> {
    store.remove_workspace(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn store_purge_removed_workspaces(store: State<Store>, before: i64) -> Result<Vec<String>, String> {
    store.purge_removed_workspaces(before).map_err(|e| e.to_string())
}

#[tauri::command]
fn store_archived(store: State<Store>) -> Result<Vec<ArchivedSession>, String> {
    store.archived().map_err(|e| e.to_string())
}

#[tauri::command]
fn store_archive_session(store: State<Store>, session_id: String, workspace_id: String) -> Result<(), String> {
    store
        .archive_session(&session_id, &workspace_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn store_unarchive_session(store: State<Store>, session_id: String) -> Result<(), String> {
    store.unarchive_session(&session_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn store_purge_archived(store: State<Store>, before: i64) -> Result<Vec<String>, String> {
    store.purge_archived(before).map_err(|e| e.to_string())
}

fn engine_binary() -> Option<std::path::PathBuf> {
    let name = "drift-engine.exe";
    let bundled = std::env::current_exe().ok()?.parent()?.join(name);
    if bundled.exists() {
        return Some(bundled);
    }
    let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(name);
    dev.exists().then_some(dev)
}

fn engine_extensions() -> Option<std::path::PathBuf> {
    let bundled = std::env::current_exe().ok()?.parent()?.join("drift-extensions");
    if bundled.exists() {
        return Some(bundled);
    }
    let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .join("engine")
        .join("opencode");
    dev.exists().then_some(dev)
}

fn spawn_engine(app: tauri::AppHandle, shared_database: bool) {
    std::thread::spawn(move || {
        let Some(binary) = engine_binary() else {
            *app.state::<Engine>().diagnostic.lock().unwrap() = "embedded engine binary not found".into();
            return;
        };
        let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME"));
        let mut command = Command::new(binary);
        command
            .args(["serve", "--port", "0"])
            .env_remove("OPENCODE_SERVER_PASSWORD")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if shared_database {
            engine_db::configure_shared(&mut command);
        }
        if let Some(extensions) = engine_extensions() {
            command.env("OPENCODE_CONFIG_DIR", extensions);
        }
        if let Ok(home) = home {
            command.current_dir(home);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                *app.state::<Engine>().diagnostic.lock().unwrap() = format!("failed to start embedded engine: {error}");
                return;
            }
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let engine = app.state::<Engine>();
        *engine.child.lock().unwrap() = Some(child);
        if let Some(stderr) = stderr {
            let diagnostics_app = app.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if !line.trim().is_empty() {
                        *diagnostics_app.state::<Engine>().diagnostic.lock().unwrap() = line;
                    }
                }
            });
        }
        let Some(stdout) = stdout else { return };
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(index) = line.find("http://") {
                let engine = app.state::<Engine>();
                *engine.url.lock().unwrap() = Some(line[index..].trim().to_string());
            }
        }
    });
}

fn main() {
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
        .invoke_handler(tauri::generate_handler![
            engine_status,
            check_update,
            install_update,
            config_read,
            pick_folder,
            store_workspaces,
            store_removed_workspaces,
            store_add_workspace,
            store_save_workspace,
            store_touch_workspace,
            store_remove_workspace,
            store_purge_removed_workspaces,
            store_archived,
            store_archive_session,
            store_unarchive_session,
            store_purge_archived
        ])
        .setup(|app| {
            let data_dir = app.path().app_data_dir().expect("no app data dir");
            let config_dir = app.path().app_config_dir().expect("no app config dir");
            std::fs::create_dir_all(&config_dir).expect("failed to create config dir");
            app.manage(ConfigRoot(config_dir));
            let shared_database = if cfg!(debug_assertions) {
                false
            } else {
                engine_binary()
                    .map(|_| match engine_db::prepare_shared() {
                        Ok(imported) => {
                            if imported > 0 {
                                eprintln!("imported {imported} Drift sessions into the shared OpenCode database");
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
            app.manage(store);
            spawn_engine(app.handle().clone(), shared_database);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build drift")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                let engine = app.state::<Engine>();
                let child = engine.child.lock().unwrap().take();
                if let Some(mut child) = child {
                    let _ = child.kill();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::config_path;
    use std::path::Path;

    #[test]
    fn config_paths_stay_under_the_config_directory() {
        let root = Path::new("config");
        assert_eq!(
            config_path(root, "plugins/example.mjs").unwrap(),
            root.join("plugins/example.mjs")
        );
        assert!(config_path(root, "../outside.mjs").is_err());
        assert!(config_path(root, "C:\\outside.mjs").is_err());
    }
}
