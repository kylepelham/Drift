#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod store;

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

#[tauri::command]
fn engine_url(engine: State<Engine>) -> Result<String, String> {
    engine
        .url
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "engine starting".into())
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

fn spawn_engine(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let Some(binary) = engine_binary() else { return };
        let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME"));
        let mut command = Command::new(binary);
        command
            .args(["serve", "--port", "0"])
            .env_remove("OPENCODE_SERVER_PASSWORD")
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
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
        let Ok(mut child) = command.spawn() else { return };
        let stdout = child.stdout.take();
        let engine = app.state::<Engine>();
        *engine.child.lock().unwrap() = Some(child);
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
        .manage(Engine::default())
        .invoke_handler(tauri::generate_handler![
            engine_url,
            config_read,
            pick_folder,
            store_workspaces,
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
            app.manage(store::open(&data_dir).expect("failed to open drift store"));
            spawn_engine(app.handle().clone());
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
