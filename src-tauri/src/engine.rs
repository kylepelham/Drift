//! Supervising the embedded opencode engine process.

use crate::engine_db;
#[cfg(windows)]
use crate::CREATE_NO_WINDOW;
use serde::Serialize;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::{Emitter, Manager, State};

/// The embedded engine is always addressed with this username; only the password varies per run.
const ENGINE_USERNAME: &str = "opencode";
/// Bind to loopback on an ephemeral port; the engine reports the port it actually got.
const ENGINE_SERVE_ARGS: [&str; 5] = ["serve", "--hostname", "127.0.0.1", "--port", "0"];
/// Shutdown is best effort - if the engine is wedged we would rather leak than hang on exit.
const ENGINE_DISPOSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
const ENGINE_EXIT_POLL: std::time::Duration = std::time::Duration::from_millis(10);
const ENGINE_EXIT_POLLS: usize = 100;
const HTTP_OK_STATUS_LINE: &str = "HTTP/1.1 200";

pub(crate) struct Engine {
    url: Mutex<Option<String>>,
    /// Held so the process can be killed on app exit.
    pub(crate) child: Mutex<Option<Child>>,
    diagnostic: Mutex<String>,
    /// Random per run; the frontend receives it from `engine_status` and uses it for basic auth.
    pub(crate) password: String,
    /// How the engine was launched, so a caller that stops it can start an equivalent one.
    launch: Mutex<Option<(bool, PathBuf)>>,
    /// Bumped per spawn so a stopped engine's reader cannot publish over its replacement.
    generation: AtomicU64,
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
            launch: Mutex::new(None),
            generation: AtomicU64::new(0),
        }
    }
}

#[derive(Serialize)]
pub(crate) struct EngineStatus {
    url: Option<String>,
    error: Option<String>,
    password: Option<String>,
}

#[tauri::command]
pub(crate) fn engine_status(engine: State<Engine>) -> EngineStatus {
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
        .map(|status| engine_exit_error(&status, &diagnostic))
        .or_else(|| (!has_child && !diagnostic.is_empty()).then(|| diagnostic.clone()));
    EngineStatus {
        url: None,
        error,
        password: None,
    }
}

fn engine_exit_error(status: &dyn std::fmt::Display, diagnostic: &str) -> String {
    if diagnostic.is_empty() {
        format!("embedded engine exited with {status}")
    } else {
        format!("embedded engine exited with {status}: {diagnostic}")
    }
}

#[tauri::command]
pub(crate) fn restart_engine(app: tauri::AppHandle) -> Result<(), String> {
    let engine = app.state::<Engine>();
    if engine.url.lock().unwrap().is_some() {
        return Ok(());
    }
    if engine.launch.lock().unwrap().is_none() {
        return Err("embedded engine launch configuration is unavailable".into());
    }
    stop_engine_child(&app);
    respawn_engine(&app);
    Ok(())
}

pub(crate) fn engine_binary() -> Option<std::path::PathBuf> {
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

pub(crate) fn engine_extensions() -> Option<std::path::PathBuf> {
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

pub(crate) fn spawn_engine(app: tauri::AppHandle, shared_database: bool, config_dir: PathBuf) {
    let generation = {
        let engine = app.state::<Engine>();
        *engine.launch.lock().unwrap() = Some((shared_database, config_dir.clone()));
        let generation = engine.generation.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut url = engine.url.lock().unwrap();
            if engine.generation.load(Ordering::SeqCst) == generation {
                *url = None;
            }
        }
        {
            let mut diagnostic = engine.diagnostic.lock().unwrap();
            if engine.generation.load(Ordering::SeqCst) == generation {
                diagnostic.clear();
            }
        }
        generation
    };
    std::thread::spawn(move || {
        let Some(binary) = engine_binary() else {
            let engine = app.state::<Engine>();
            let mut diagnostic = engine.diagnostic.lock().unwrap();
            if engine.generation.load(Ordering::SeqCst) == generation {
                *diagnostic = "embedded engine binary not found".into();
            }
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
                let engine = app.state::<Engine>();
                let mut diagnostic = engine.diagnostic.lock().unwrap();
                if engine.generation.load(Ordering::SeqCst) == generation {
                    *diagnostic = format!("failed to start embedded engine: {error}");
                }
                return;
            }
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let engine = app.state::<Engine>();
        let mut slot = engine.child.lock().unwrap();
        if engine.generation.load(Ordering::SeqCst) != generation {
            drop(slot);
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        let previous = slot.replace(child);
        drop(slot);
        if let Some(mut previous) = previous {
            let _ = previous.kill();
            let _ = previous.wait();
        }
        let diagnostic_reader = stderr.map(|stderr| {
            let diagnostics_app = app.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let engine = diagnostics_app.state::<Engine>();
                    let mut diagnostic = engine.diagnostic.lock().unwrap();
                    if engine.generation.load(Ordering::SeqCst) == generation
                        && !line.trim().is_empty()
                    {
                        *diagnostic = line;
                    }
                }
            })
        });
        let Some(stdout) = stdout else { return };
        let current = || app.state::<Engine>().generation.load(Ordering::SeqCst) == generation;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(index) = line.find("http://") {
                let engine = app.state::<Engine>();
                let mut url = engine.url.lock().unwrap();
                if engine.generation.load(Ordering::SeqCst) == generation {
                    *url = Some(line[index..].trim().to_string());
                }
            }
        }
        // A superseded engine must not report its own exit as the current engine going down.
        let engine = app.state::<Engine>();
        let mut url = engine.url.lock().unwrap();
        if engine.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        *url = None;
        drop(url);
        for _ in 0..ENGINE_EXIT_POLLS {
            if !current() {
                return;
            }
            let exited = app
                .state::<Engine>()
                .child
                .lock()
                .unwrap()
                .as_mut()
                .and_then(|child| child.try_wait().ok().flatten())
                .is_some();
            if exited {
                if let Some(reader) = diagnostic_reader {
                    let _ = reader.join();
                }
                if current() {
                    let _ = app.emit("engine-exited", ());
                }
                return;
            }
            std::thread::sleep(ENGINE_EXIT_POLL);
        }
    });
}

/// Stops the sidecar and waits until Windows releases the executable lock.
pub(crate) fn stop_engine_child(app: &tauri::AppHandle) {
    let engine = app.state::<Engine>();
    engine.generation.fetch_add(1, Ordering::SeqCst);
    let child = engine.child.lock().unwrap().take();
    if let Some(mut child) = child {
        let _ = child.kill();
        let _ = child.wait();
    }
    *engine.url.lock().unwrap() = None;
    engine.diagnostic.lock().unwrap().clear();
}

/// Invalidates process readers and terminates the child without delaying app shutdown.
pub(crate) fn stop_engine_on_exit(app: &tauri::AppHandle) {
    let engine = app.state::<Engine>();
    engine.generation.fetch_add(1, Ordering::SeqCst);
    let child = engine.child.lock().unwrap().take();
    if let Some(mut child) = child {
        let _ = child.kill();
    }
    *engine.url.lock().unwrap() = None;
    engine.diagnostic.lock().unwrap().clear();
}

/// Starts a replacement sidecar using the parameters the first one was launched with.
pub(crate) fn respawn_engine(app: &tauri::AppHandle) {
    let launch = app.state::<Engine>().launch.lock().unwrap().clone();
    if let Some((shared_database, config_dir)) = launch {
        spawn_engine(app.clone(), shared_database, config_dir);
    }
}

pub(crate) fn stop_engine_instances(app: &tauri::AppHandle) -> Result<(), String> {
    let engine = app.state::<Engine>();
    let Some(url) = engine.url.lock().unwrap().clone() else {
        return Ok(());
    };
    let parsed = url::Url::parse(&url).map_err(|error| error.to_string())?;
    let host = parsed.host_str().ok_or("embedded engine URL has no host")?;
    let port = parsed
        .port_or_known_default()
        .ok_or("embedded engine URL has no port")?;
    let address = (host, port)
        .to_socket_addrs()
        .map_err(|error| error.to_string())?
        .next()
        .ok_or("embedded engine URL could not be resolved")?;
    let mut stream = TcpStream::connect_timeout(&address, ENGINE_DISPOSE_TIMEOUT)
        .map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(ENGINE_DISPOSE_TIMEOUT))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(ENGINE_DISPOSE_TIMEOUT))
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

pub(crate) fn basic_authorization(username: &str, password: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::engine_exit_error;

    #[test]
    fn exit_diagnostics_include_status_and_last_error() {
        assert_eq!(
            engine_exit_error(&"exit code 1", "database unavailable"),
            "embedded engine exited with exit code 1: database unavailable"
        );
        assert_eq!(
            engine_exit_error(&"exit code 1", ""),
            "embedded engine exited with exit code 1"
        );
    }
}
