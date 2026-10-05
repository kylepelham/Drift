//! Language servers: started for a workspace on the first write to a file one handles, they report
//! the errors a writing call left behind, within a short wait. A server that is missing, crashes or
//! answers late never fails or holds up a call; the result just carries no diagnostics.

mod client;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};
use serde::Serialize;

pub use client::Diagnostic;
use client::Client;

use crate::config::LspConfig;

/// How long a writing call waits for errors, after its formatters ran.
const WAIT: Duration = Duration::from_secs(3);
/// A server unused this long is stopped; the next write starts it again.
const IDLE: Duration = Duration::from_secs(10 * 60);
/// A command that could not start is not tried again for this long.
const RETRY: Duration = Duration::from_secs(5 * 60);
const MAX_PER_FILE: usize = 10;
const MAX_TOTAL: usize = 30;
const MAX_MESSAGE_CHARS: usize = 300;

/// Servers used when on PATH, each over stdio: name, command, the extensions it handles.
const BUILTIN: [(&str, &[&str], &[&str]); 5] = [
    ("rust-analyzer", &["rust-analyzer"], &[".rs"]),
    ("typescript", &["typescript-language-server", "--stdio"], &[".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"]),
    ("pyright", &["pyright-langserver", "--stdio"], &[".py", ".pyi"]),
    ("gopls", &["gopls"], &[".go"]),
    ("clangd", &["clangd"], &[".c", ".h", ".cc", ".cpp", ".cxx", ".hpp", ".hh", ".hxx"]),
];

#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub name: String,
    pub command: Vec<String>,
    pub extensions: Vec<String>,
    pub language: Option<String>,
}

/// The built-ins less those the config turns off or replaces, then the config's own.
pub fn resolve(config: &BTreeMap<String, LspConfig>) -> Vec<Spec> {
    // The engine's own tests write source files; they never start whatever servers this machine has.
    let table: &[(&str, &[&str], &[&str])] = if cfg!(test) { &[] } else { &BUILTIN };
    let builtin = table.iter().filter(|(name, _, _)| !config.contains_key(*name)).map(|(name, command, extensions)| Spec {
        name: name.to_string(),
        command: command.iter().map(|part| part.to_string()).collect(),
        extensions: extensions.iter().map(|ext| ext.to_string()).collect(),
        language: None,
    });
    let custom = config.iter().filter_map(|(name, server)| match server {
        LspConfig::Custom { command, extensions, language } if !command.is_empty() => Some(Spec { name: name.clone(), command: command.clone(), extensions: extensions.clone(), language: language.clone() }),
        _ => None,
    });
    builtin.chain(custom).collect()
}

fn handles(spec: &Spec, file: &Path) -> bool {
    let name = file.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
    spec.extensions.iter().any(|ext| name.ends_with(&ext.to_lowercase()))
}

/// The LSP language id of `file`, from its extension unless the server names one.
fn language(spec: &Spec, file: &Path) -> String {
    if let Some(language) = &spec.language {
        return language.clone();
    }
    let ext = file.extension().map(|ext| ext.to_string_lossy().to_lowercase()).unwrap_or_default();
    let id = match ext.as_str() {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "py" | "pyi" => "python",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        other => other,
    };
    id.to_string()
}

/// The errors one server reported for one file after a write.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub file: PathBuf,
    pub server: String,
    pub errors: Vec<Diagnostic>,
}

type Running = Arc<Mutex<HashMap<(PathBuf, String), Arc<Client>>>>;

/// The servers running for each workspace; dropping it (the engine stopping) ends them all.
#[derive(Default)]
pub struct Servers {
    running: Running,
    failed: Mutex<HashMap<String, Instant>>,
    reaping: std::sync::atomic::AtomicBool,
}

impl Servers {
    /// What the servers that handle `files` report about them within [`WAIT`]; files with no errors are left out.
    pub async fn report(&self, workspace: &Path, files: &[PathBuf], config: &BTreeMap<String, LspConfig>) -> Vec<Found> {
        let deadline = tokio::time::Instant::now() + WAIT;
        let specs = resolve(config);
        let mut checks = Vec::new();
        for spec in &specs {
            let handled: Vec<(PathBuf, String)> = files.iter().filter(|file| handles(spec, file)).map(|file| (file.clone(), language(spec, file))).collect();
            if handled.is_empty() {
                continue;
            }
            if let Some(client) = self.client(workspace, spec).await {
                checks.push(async move { (spec.name.clone(), client.check(&handled, deadline).await) });
            }
        }
        let reported = futures_util::future::join_all(checks).await;
        reported.into_iter().flat_map(|(server, files)| files.into_iter().filter(|(_, errors)| !errors.is_empty()).map(move |(file, errors)| Found { file, server: server.clone(), errors })).collect()
    }

    /// The workspace's running server for `spec`, started now if it is not running and did not fail to start lately.
    async fn client(&self, workspace: &Path, spec: &Spec) -> Option<Arc<Client>> {
        let slot = (workspace.to_path_buf(), spec.name.clone());
        if let Some(client) = self.running.lock().unwrap().get(&slot).filter(|client| client.alive()) {
            return Some(client.clone());
        }
        let tried = self.failed.lock().unwrap().get(&spec.name).is_some_and(|at| at.elapsed() < RETRY);
        if tried {
            return None;
        }
        match Client::start(&spec.command, workspace).await {
            Ok(client) => {
                self.running.lock().unwrap().insert(slot, client.clone());
                self.reap_idle();
                Some(client)
            }
            Err(_) => {
                self.failed.lock().unwrap().insert(spec.name.clone(), Instant::now());
                None
            }
        }
    }

    /// Once, a task that stops servers left idle (or dead); it ends when the engine does.
    fn reap_idle(&self) {
        if self.reaping.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let running = Arc::downgrade(&self.running);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let Some(running) = running.upgrade() else { return };
                let idle: Vec<Arc<Client>> = {
                    let mut map = running.lock().unwrap();
                    let stale: Vec<_> = map.iter().filter(|(_, client)| !client.alive() || client.idle_for() > IDLE).map(|(slot, _)| slot.clone()).collect();
                    stale.iter().filter_map(|slot| map.remove(slot)).collect()
                };
                for client in idle {
                    client.shutdown().await;
                }
            }
        });
    }
}

impl Drop for Servers {
    fn drop(&mut self) {
        self.running.lock().unwrap().clear();
    }
}

/// What the model is told: each file's errors, bounded, under the server that reported them.
pub fn note(found: &[Found], workspace: &Path) -> Option<String> {
    let mut lines = Vec::new();
    let mut total = 0;
    for entry in found {
        let shown: Vec<&Diagnostic> = entry.errors.iter().take(MAX_PER_FILE.min(MAX_TOTAL - total)).collect();
        if shown.is_empty() {
            break;
        }
        total += shown.len();
        let path = crate::tool::display(&entry.file, workspace);
        lines.push(format!("{} reports errors in {path}:", entry.server));
        lines.extend(shown.iter().map(|error| format!("  {path}:{}:{} {}", error.line, error.column, clip(&error.message))));
        if entry.errors.len() > shown.len() {
            lines.push(format!("  ({} more not shown)", entry.errors.len() - shown.len()));
        }
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

#[derive(Serialize)]
struct Shown<'a> {
    file: String,
    server: &'a str,
    line: u32,
    column: u32,
    message: String,
}

/// The same errors as the call's metadata keeps them.
pub fn metadata(found: &[Found], workspace: &Path) -> serde_json::Value {
    let shown: Vec<Shown> = found
        .iter()
        .flat_map(|entry| entry.errors.iter().take(MAX_PER_FILE).map(move |error| (entry, error)))
        .take(MAX_TOTAL)
        .map(|(entry, error)| Shown { file: crate::tool::display(&entry.file, workspace), server: &entry.server, line: error.line, column: error.column, message: clip(&error.message) })
        .collect();
    serde_json::to_value(shown).unwrap_or_default()
}

fn clip(message: &str) -> String {
    match message.char_indices().nth(MAX_MESSAGE_CHARS) {
        Some((at, _)) => format!("{}...", &message[..at]),
        None => message.to_string(),
    }
}

/// Characters a `file:` URI cannot carry as they are.
const URI: &AsciiSet = &CONTROLS.add(b' ').add(b'"').add(b'#').add(b'%').add(b'<').add(b'>').add(b'?').add(b'[').add(b']').add(b'`').add(b'{').add(b'}').add(b'^').add(b'|');

/// `file:///C:/work/a.rs` for `C:\work\a.rs`, `file:///home/a.rs` for `/home/a.rs`.
fn uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let rooted = if text.starts_with('/') { text } else { format!("/{text}") };
    format!("file://{}", utf8_percent_encode(&rooted, URI))
}

fn path_of(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let decoded = percent_decode_str(rest).decode_utf8().ok()?;
    let drive = decoded.as_bytes().get(2) == Some(&b':') && decoded.starts_with('/');
    Some(PathBuf::from(if drive { &decoded[1..] } else { &decoded[..] }))
}

/// One name for a file however a server spells its URI (`c%3A`, `C:`, back or forward slashes).
fn key(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) { text.to_lowercase() } else { text }
}

#[cfg(test)]
mod tests;
