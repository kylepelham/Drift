#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod store;

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use store::{ArchivedSession, Store, Workspace};
use tauri::{Manager, RunEvent, State};

#[derive(Default)]
struct Engine {
    url: Mutex<Option<String>>,
    child: Mutex<Option<Child>>,
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
fn store_delete_workspace(store: State<Store>, id: String) -> Result<(), String> {
    store.delete_workspace(&id).map_err(|e| e.to_string())
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
            pick_folder,
            store_workspaces,
            store_save_workspace,
            store_touch_workspace,
            store_delete_workspace,
            store_archived,
            store_archive_session,
            store_purge_archived
        ])
        .setup(|app| {
            let data_dir = app.path().app_data_dir().expect("no app data dir");
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
