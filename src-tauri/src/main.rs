#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engine_db;
mod mcp;
mod store;

use serde::Serialize;
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use store::{ArchivedSession, Store, Workspace};
use tauri::{Emitter, Manager, RunEvent, State};
use tauri_plugin_opener::OpenerExt;

/// Windows `CREATE_NO_WINDOW`: keeps spawned console processes from flashing a terminal.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Windows clipboard format for UTF-16 text.
#[cfg(windows)]
const CF_UNICODETEXT: u32 = 13;

/// The embedded engine is always addressed with this username; only the password varies per run.
const ENGINE_USERNAME: &str = "opencode";
/// Bind to loopback on an ephemeral port; the engine reports the port it actually got.
const ENGINE_SERVE_ARGS: [&str; 5] = ["serve", "--hostname", "127.0.0.1", "--port", "0"];
/// Shutdown is best effort - if the engine is wedged we would rather leak than hang on exit.
const ENGINE_DISPOSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
const HTTP_OK_STATUS_LINE: &str = "HTTP/1.1 200";

/// Config files are polled rather than watched, so the interval bounds how stale a change can be.
const CONFIG_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
const MAX_CONFIG_FILE_BYTES: u64 = 1_048_576;
const MAX_WATCHED_MCP_FILES: usize = 4096;
const MAX_WATCHED_FILE_BYTES: u64 = 1_048_576;
/// Bounds on `{file:...}` reference expansion inside a config file.
const MAX_FILE_REFERENCES: usize = 256;
const FILE_REFERENCE_PREFIX: &str = "{file:";
const MAX_REFERENCE_PATH_CHARS: usize = 4096;
/// Plugin directories are scanned recursively; this stops a symlink loop from running forever.
const MAX_PLUGIN_SCAN_DEPTH: usize = 16;

struct Engine {
    url: Mutex<Option<String>>,
    child: Mutex<Option<Child>>,
    diagnostic: Mutex<String>,
    password: String,
}

impl Default for Engine {
    fn default() -> Self {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("failed to generate engine password");
        Self {
            url: Mutex::new(None),
            child: Mutex::new(None),
            diagnostic: Mutex::new(String::new()),
            password: bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
        }
    }
}

#[derive(Serialize)]
struct EngineStatus {
    url: Option<String>,
    error: Option<String>,
    password: Option<String>,
}

#[derive(Serialize)]
struct OpenFileResult {
    positioned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum EditorKind {
    GotoFlag,
    Location,
    NotepadPlus,
}

struct Editor {
    executable: PathBuf,
    kind: EditorKind,
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
    if requested.metadata().map_err(|e| e.to_string())?.len() > MAX_CONFIG_FILE_BYTES {
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
        return EngineStatus {
            url,
            error: None,
            password: Some(engine.password.clone()),
        };
    }
    let (has_child, status) = {
        let mut child = engine.child.lock().unwrap();
        (
            child.is_some(),
            child
                .as_mut()
                .and_then(|child| child.try_wait().ok().flatten()),
        )
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
    EngineStatus {
        url: None,
        error,
        password: None,
    }
}

#[cfg(windows)]
#[tauri::command]
fn clipboard_write_text(window: tauri::WebviewWindow, text: String) -> Result<(), String> {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    if text.is_empty() {
        return Ok(());
    }
    let value = clipboard_utf16(&text);
    let hwnd = window.hwnd().map_err(|error| error.to_string())?.0;
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, value.len() * std::mem::size_of::<u16>());
        if memory.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let target = GlobalLock(memory).cast::<u16>();
        if target.is_null() {
            GlobalFree(memory);
            return Err(std::io::Error::last_os_error().to_string());
        }
        std::ptr::copy_nonoverlapping(value.as_ptr(), target, value.len());
        GlobalUnlock(memory);

        let history_memory = GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of::<u32>());
        if history_memory.is_null() {
            GlobalFree(memory);
            return Err(std::io::Error::last_os_error().to_string());
        }
        let history_target = GlobalLock(history_memory).cast::<u32>();
        if history_target.is_null() {
            GlobalFree(history_memory);
            GlobalFree(memory);
            return Err(std::io::Error::last_os_error().to_string());
        }
        history_target.write(1);
        GlobalUnlock(history_memory);
        let history_name = clipboard_utf16("CanIncludeInClipboardHistory");
        let history_format = RegisterClipboardFormatW(history_name.as_ptr());
        if history_format == 0 {
            GlobalFree(history_memory);
            GlobalFree(memory);
            return Err(std::io::Error::last_os_error().to_string());
        }

        if OpenClipboard(hwnd) == 0 {
            GlobalFree(history_memory);
            GlobalFree(memory);
            return Err(std::io::Error::last_os_error().to_string());
        }
        if EmptyClipboard() == 0 {
            let error = std::io::Error::last_os_error().to_string();
            CloseClipboard();
            GlobalFree(history_memory);
            GlobalFree(memory);
            return Err(error);
        }
        if SetClipboardData(CF_UNICODETEXT, memory).is_null() {
            let error = std::io::Error::last_os_error().to_string();
            CloseClipboard();
            GlobalFree(history_memory);
            GlobalFree(memory);
            return Err(error);
        }
        if SetClipboardData(history_format, history_memory).is_null() {
            let error = std::io::Error::last_os_error().to_string();
            CloseClipboard();
            GlobalFree(history_memory);
            return Err(error);
        }
        CloseClipboard();
    }
    Ok(())
}

#[cfg(not(windows))]
#[tauri::command]
fn clipboard_write_text(_window: tauri::WebviewWindow, _text: String) -> Result<(), String> {
    Ok(())
}

fn clipboard_utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[tauri::command]
async fn pick_folder() -> Option<String> {
    rfd::FileDialog::new()
        .pick_folder()
        .map(|path| path.to_string_lossy().to_string())
}

#[tauri::command]
fn open_file(
    app: tauri::AppHandle,
    path: String,
    line: Option<u32>,
    column: Option<u32>,
) -> Result<OpenFileResult, String> {
    let positioned =
        line.is_some_and(|line| open_positioned(&path, line.max(1), column.unwrap_or(1).max(1)));
    if positioned {
        return Ok(OpenFileResult { positioned });
    }
    app.opener()
        .open_path(&path, None::<&str>)
        .map_err(|error| error.to_string())?;
    Ok(OpenFileResult { positioned })
}

fn open_positioned(path: &str, line: u32, column: u32) -> bool {
    static EDITOR: OnceLock<Option<Editor>> = OnceLock::new();
    let Some(editor) = EDITOR.get_or_init(detect_editor) else {
        return false;
    };
    spawn_editor(
        &editor.executable,
        &editor_arguments(editor.kind, path, line, column),
    )
    .is_ok()
}

fn editor_arguments(kind: EditorKind, path: &str, line: u32, column: u32) -> Vec<String> {
    let location = format!("{path}:{line}:{column}");
    match kind {
        EditorKind::GotoFlag => vec!["--goto".to_string(), location],
        EditorKind::Location => vec![location],
        EditorKind::NotepadPlus => {
            vec![format!("-n{line}"), format!("-c{column}"), path.to_string()]
        }
    }
}

#[cfg(windows)]
fn detect_editor() -> Option<Editor> {
    for name in ["DRIFT_EDITOR", "VISUAL", "EDITOR"] {
        let Some(value) = std::env::var(name).ok() else {
            continue;
        };
        let path = PathBuf::from(value.trim_matches('"'));
        if path.is_file() {
            return Some(Editor {
                kind: editor_kind(&path),
                executable: path,
            });
        }
    }
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let program = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let candidates = [
        local
            .as_ref()
            .map(|root| root.join("Programs/Microsoft VS Code/Code.exe")),
        local
            .as_ref()
            .map(|root| root.join("Programs/Microsoft VS Code Insiders/Code - Insiders.exe")),
        local
            .as_ref()
            .map(|root| root.join("Programs/Cursor/Cursor.exe")),
        local
            .as_ref()
            .map(|root| root.join("Programs/Windsurf/Windsurf.exe")),
        local.as_ref().map(|root| root.join("Programs/Zed/Zed.exe")),
        program
            .as_ref()
            .map(|root| root.join("Microsoft VS Code/Code.exe")),
        program
            .as_ref()
            .map(|root| root.join("Sublime Text/sublime_text.exe")),
        program
            .as_ref()
            .map(|root| root.join("Notepad++/notepad++.exe")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|path| path.is_file())
        .map(|path| Editor {
            kind: editor_kind(&path),
            executable: path,
        })
}

#[cfg(not(windows))]
fn detect_editor() -> Option<Editor> {
    let executable = ["DRIFT_EDITOR", "VISUAL", "EDITOR"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok())
        .unwrap_or_else(|| "code".to_string());
    let path = PathBuf::from(executable);
    Some(Editor {
        kind: editor_kind(&path),
        executable: path,
    })
}

fn editor_kind(path: &Path) -> EditorKind {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.contains("notepad++") {
        return EditorKind::NotepadPlus;
    }
    if name.starts_with("zed") || name.starts_with("sublime") || name.starts_with("subl") {
        return EditorKind::Location;
    }
    EditorKind::GotoFlag
}

fn spawn_editor(executable: &Path, args: &[String]) -> std::io::Result<Child> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.spawn()
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
    store
        .add_workspace(&id, &path, &name, &icon)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn store_save_workspace(
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
fn store_touch_workspace(store: State<Store>, id: String) -> Result<(), String> {
    store.touch_workspace(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn store_remove_workspace(store: State<Store>, id: String) -> Result<(), String> {
    store.remove_workspace(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn store_purge_removed_workspaces(store: State<Store>, before: i64) -> Result<Vec<String>, String> {
    store
        .purge_removed_workspaces(before)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn store_archived(store: State<Store>) -> Result<Vec<ArchivedSession>, String> {
    store.archived().map_err(|e| e.to_string())
}

#[tauri::command]
fn store_archive_session(
    store: State<Store>,
    session_id: String,
    workspace_id: String,
) -> Result<(), String> {
    store
        .archive_session(&session_id, &workspace_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn store_unarchive_session(store: State<Store>, session_id: String) -> Result<(), String> {
    store
        .unarchive_session(&session_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn store_purge_archived(store: State<Store>, before: i64) -> Result<Vec<String>, String> {
    store.purge_archived(before).map_err(|e| e.to_string())
}

#[tauri::command]
fn mcp_snapshot(
    runtime: State<mcp::McpRuntime>,
    store: State<Store>,
    directory: String,
) -> Result<mcp::McpSnapshot, String> {
    runtime.snapshot(&store, &directory)
}

#[tauri::command]
fn prompt_snapshot(
    runtime: State<mcp::McpRuntime>,
    store: State<Store>,
) -> Result<mcp::PromptSnapshot, String> {
    runtime.prompt_snapshot(&store)
}

#[tauri::command]
fn prompt_save(
    runtime: State<mcp::McpRuntime>,
    store: State<Store>,
    key: String,
    value: Value,
    original: Option<Value>,
) -> Result<(), String> {
    runtime.save_prompt(&store, &key, value, original)
}

#[tauri::command]
fn prompt_reset(
    runtime: State<mcp::McpRuntime>,
    store: State<Store>,
    key: String,
) -> Result<(), String> {
    runtime.reset_prompt(&store, &key)
}

#[tauri::command]
fn mcp_save(
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
fn mcp_remove(
    app: tauri::AppHandle,
    runtime: State<mcp::McpRuntime>,
    store: State<Store>,
    name: String,
    generation: i64,
) -> Result<(), String> {
    runtime.remove(&store, &name, generation, || stop_engine_instances(&app))
}

#[tauri::command]
fn mcp_approve(
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
fn mcp_reject(
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
fn mcp_revoke(
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
    let bundled = std::env::current_exe()
        .ok()?
        .parent()?
        .join("drift-extensions");
    if bundled.exists() {
        return Some(bundled);
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let generated = root.join("generated").join("drift-extensions");
    if generated.join("opencode.json").is_file()
        && generated.join("prompt-catalog.json").is_file()
        && generated
            .join("plugin")
            .join("prompt-overrides.js")
            .is_file()
    {
        return Some(generated);
    }
    let source = root.parent()?.join("engine").join("opencode");
    source.exists().then_some(source)
}

fn spawn_engine(app: tauri::AppHandle, shared_database: bool, config_dir: PathBuf) {
    std::thread::spawn(move || {
        let Some(binary) = engine_binary() else {
            *app.state::<Engine>().diagnostic.lock().unwrap() =
                "embedded engine binary not found".into();
            return;
        };
        let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME"));
        let password = app.state::<Engine>().password.clone();
        let mut command = Command::new(binary);
        command
            .args(ENGINE_SERVE_ARGS)
            .env("OPENCODE_SERVER_PASSWORD", password)
            .env("OPENCODE_SERVER_USERNAME", ENGINE_USERNAME)
            .env_remove("OPENCODE_CONFIG")
            .env_remove("OPENCODE_CONFIG_CONTENT")
            .env("OPENCODE_CONFIG_DIR", config_dir)
            .env("DRIFT_MCP_APPROVAL_REQUIRED", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if shared_database {
            engine_db::configure_shared(&mut command);
        }
        if let Ok(home) = home {
            command.current_dir(home);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                *app.state::<Engine>().diagnostic.lock().unwrap() =
                    format!("failed to start embedded engine: {error}");
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
        *app.state::<Engine>().url.lock().unwrap() = None;
    });
}

fn stop_engine_instances(app: &tauri::AppHandle) -> Result<(), String> {
    let engine = app.state::<Engine>();
    let Some(url) = engine.url.lock().unwrap().clone() else {
        return Ok(());
    };
    let parsed = url::Url::parse(&url).map_err(|error| error.to_string())?;
    let host = parsed.host_str().ok_or("embedded engine URL has no host")?;
    let port = parsed
        .port_or_known_default()
        .ok_or("embedded engine URL has no port")?;
    let mut stream = TcpStream::connect((host, port)).map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(ENGINE_DISPOSE_TIMEOUT))
        .map_err(|error| error.to_string())?;
    let authorization = basic_authorization(ENGINE_USERNAME, &engine.password);
    let request = format!(
        "POST /global/dispose HTTP/1.1\r\nHost: {host}:{port}\r\nAuthorization: Basic {authorization}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| error.to_string())?;
    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .map_err(|error| error.to_string())?;
    if !response.starts_with(HTTP_OK_STATUS_LINE) {
        return Err("embedded engine refused global disposal".into());
    }
    Ok(())
}

fn basic_authorization(username: &str, password: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let input = format!("{username}:{password}");
    let mut encoded = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.as_bytes().chunks(3) {
        let value = ((chunk[0] as u32) << 16)
            | (chunk.get(1).copied().unwrap_or(0) as u32) << 8
            | chunk.get(2).copied().unwrap_or(0) as u32;
        encoded.push(ALPHABET[((value >> 18) & 63) as usize] as char);
        encoded.push(ALPHABET[((value >> 12) & 63) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            ALPHABET[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            ALPHABET[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    encoded
}

fn watch_mcp_configs(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let mut previous = external_mcp_signature(&app.state::<Store>());
        loop {
            std::thread::sleep(CONFIG_POLL_INTERVAL);
            let current = external_mcp_signature(&app.state::<Store>());
            if current == previous {
                continue;
            }
            let reloaded = app
                .state::<mcp::McpRuntime>()
                .reload(&app.state::<Store>(), || stop_engine_instances(&app));
            match reloaded {
                Ok(()) => {
                    previous = current;
                    let _ = app.emit("mcp-config-changed", ());
                }
                Err(error) => eprintln!("failed to reload changed MCP configuration: {error}"),
            }
        }
    });
}

fn external_mcp_signature(store: &Store) -> Vec<(PathBuf, u64, u128, u64)> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let home = PathBuf::from(home);
        roots.push(home.join(".config/opencode"));
        roots.push(home.join(".opencode"));
    }
    if let Some(root) = std::env::var_os("XDG_CONFIG_HOME") {
        roots.push(PathBuf::from(root).join("opencode"));
    }
    if let Some(root) = std::env::var_os("APPDATA") {
        roots.push(PathBuf::from(root).join("opencode"));
    }
    roots.push(managed_config_root());
    if let Ok(workspaces) = store.workspaces() {
        for workspace in workspaces {
            for ancestor in Path::new(&workspace.path).ancestors() {
                roots.push(ancestor.to_path_buf());
                roots.push(ancestor.join(".opencode"));
            }
        }
    }
    roots.sort();
    roots.dedup();
    roots.truncate(MAX_WATCHED_MCP_FILES);
    let configs = roots
        .iter()
        .flat_map(|root| {
            [
                root.join("opencode.json"),
                root.join("opencode.jsonc"),
                root.join("config.json"),
            ]
        })
        .collect::<Vec<_>>();
    let plugins = roots
        .into_iter()
        .flat_map(|root| [root.join("plugin"), root.join("plugins")])
        .collect::<Vec<_>>();
    let mut paths = watched_mcp_paths(configs, plugins);
    if cfg!(target_os = "macos") {
        let managed = PathBuf::from("/Library/Managed Preferences");
        paths.push(managed.join("ai.opencode.managed.plist"));
        if let Some(user) = std::env::var_os("USER") {
            paths.push(managed.join(user).join("ai.opencode.managed.plist"));
        }
    }
    file_signatures(paths)
}

fn watched_mcp_paths(mut configs: Vec<PathBuf>, mut plugin_roots: Vec<PathBuf>) -> Vec<PathBuf> {
    configs.sort();
    configs.dedup();
    configs.truncate(MAX_WATCHED_MCP_FILES);
    plugin_roots.sort();
    plugin_roots.dedup();
    plugin_roots.truncate(MAX_WATCHED_MCP_FILES);
    let mut paths = configs.clone();
    for config in configs {
        let Ok(metadata) = std::fs::metadata(&config) else {
            continue;
        };
        if metadata.len() > MAX_WATCHED_FILE_BYTES {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&config) else {
            continue;
        };
        paths.extend(config_file_references(
            &contents,
            config.parent().unwrap_or(Path::new(".")),
        ));
    }
    for root in plugin_roots {
        collect_plugin_files(&root, &mut paths);
        if paths.len() >= MAX_WATCHED_MCP_FILES {
            break;
        }
    }
    paths.sort();
    paths.dedup();
    paths.truncate(MAX_WATCHED_MCP_FILES);
    paths
}

fn config_file_references(contents: &str, parent: &Path) -> Vec<PathBuf> {
    let mut references = Vec::new();
    let mut cursor = 0;
    while references.len() < MAX_FILE_REFERENCES {
        let Some(start) = contents[cursor..]
            .find(FILE_REFERENCE_PREFIX)
            .map(|index| cursor + index + FILE_REFERENCE_PREFIX.len())
        else {
            break;
        };
        let Some(end) = contents[start..].find('}').map(|index| start + index) else {
            break;
        };
        let value = contents[start..end].trim();
        if !value.is_empty() && value.len() <= MAX_REFERENCE_PATH_CHARS {
            let path = if let Some(relative) = value.strip_prefix("~/") {
                std::env::var_os("USERPROFILE")
                    .or_else(|| std::env::var_os("HOME"))
                    .map(PathBuf::from)
                    .unwrap_or_else(|| parent.to_path_buf())
                    .join(relative)
            } else {
                let path = PathBuf::from(value);
                if path.is_absolute() {
                    path
                } else {
                    parent.join(path)
                }
            };
            references.push(path);
        }
        cursor = end + 1;
    }
    references
}

fn collect_plugin_files(root: &Path, paths: &mut Vec<PathBuf>) {
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    while let Some((directory, depth)) = pending.pop() {
        if paths.len() >= MAX_WATCHED_MCP_FILES {
            break;
        }
        if depth > MAX_PLUGIN_SCAN_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        let mut entries = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| right.cmp(left));
        for path in entries {
            if path.is_dir() {
                pending.push((path, depth + 1));
            } else if path.is_file() {
                paths.push(path);
            }
        }
    }
}

fn file_signatures(mut paths: Vec<PathBuf>) -> Vec<(PathBuf, u64, u128, u64)> {
    paths.sort();
    paths.dedup();
    paths.truncate(MAX_WATCHED_MCP_FILES);
    paths
        .into_iter()
        .filter_map(|path| {
            let metadata = std::fs::metadata(&path).ok()?;
            if !metadata.is_file() {
                return None;
            }
            let modified = metadata
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_nanos();
            let mut bytes = Vec::new();
            std::fs::File::open(&path)
                .ok()?
                .take(MAX_WATCHED_FILE_BYTES)
                .read_to_end(&mut bytes)
                .ok()?;
            let mut hasher = DefaultHasher::new();
            bytes.hash(&mut hasher);
            Some((path, metadata.len(), modified, hasher.finish()))
        })
        .collect()
}

fn managed_config_root() -> PathBuf {
    if cfg!(target_os = "windows") {
        return std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
            .join("opencode");
    }
    if cfg!(target_os = "macos") {
        return PathBuf::from("/Library/Application Support/opencode");
    }
    PathBuf::from("/etc/opencode")
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
            clipboard_write_text,
            config_read,
            pick_folder,
            open_file,
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
            store_purge_archived,
            mcp_snapshot,
            prompt_snapshot,
            prompt_save,
            prompt_reset,
            mcp_save,
            mcp_remove,
            mcp_approve,
            mcp_reject,
            mcp_revoke
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
            let extensions = engine_extensions().expect("embedded engine extensions not found");
            let mcp_runtime = mcp::McpRuntime::new(&data_dir, extensions);
            mcp_runtime
                .materialize(&store)
                .expect("failed to prepare Drift MCP policy");
            let engine_config = mcp_runtime.config_dir().to_path_buf();
            app.manage(store);
            app.manage(mcp_runtime);
            spawn_engine(app.handle().clone(), shared_database, engine_config);
            watch_mcp_configs(app.handle().clone());
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
#[path = "main_tests.rs"]
mod tests;
