#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
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
        .invoke_handler(tauri::generate_handler![engine_url])
        .setup(|app| {
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
