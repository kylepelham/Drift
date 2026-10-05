//! One language server over stdio: JSON-RPC framed by `Content-Length`. It initializes in the
//! background, and the errors it publishes are kept per file with a count of how often it published,
//! so a check reports only what arrived after its own change.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{oneshot, watch, Notify};

use crate::platform::process;

/// A message larger than this is not a server talking sense; the connection ends.
const MAX_MESSAGE: usize = 64 * 1024 * 1024;
/// How long a server may take to answer `initialize`, in the background; edits meanwhile hear nothing.
const INITIALIZE: Duration = Duration::from_secs(90);
/// After the first errors for a change arrive, a little longer for a second round (rust-analyzer's own, then cargo's).
const SETTLE: Duration = Duration::from_millis(300);
/// A file larger than this is not sent; the server would only slow down.
const MAX_FILE: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub line: u32,
    pub column: u32,
    pub message: String,
}

pub struct Client {
    stdin: tokio::sync::Mutex<ChildStdin>,
    next_id: AtomicI64,
    pending: Mutex<HashMap<i64, oneshot::Sender<Value>>>,
    /// Errors by file (`key`), with how many times the server published for it.
    published: Mutex<HashMap<String, (u64, Vec<Diagnostic>)>>,
    changed: Notify,
    versions: Mutex<HashMap<String, i32>>,
    ready: watch::Sender<bool>,
    /// The server answers `textDocument/diagnostic` (pull) rather than, or as well as, publishing.
    pulls: AtomicBool,
    alive: AtomicBool,
    used: Mutex<Instant>,
    process: Mutex<Option<(Child, process::Tree)>>,
}

impl Client {
    /// Starts `program` in `root` and initializes it in the background; `Err` when it cannot start at all.
    pub async fn start(program: &Path, args: &[String], root: &Path) -> std::io::Result<Arc<Self>> {
        let mut spawn = tokio::process::Command::new(program);
        process::use_current_path(&mut spawn, &Default::default());
        spawn.args(args).current_dir(root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        spawn.creation_flags(0x0800_0000);
        let (mut child, tree) = process::spawn_owned(&mut spawn).await?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else { return Err(std::io::Error::other("no pipes")) };
        let client = Arc::new(Self {
            stdin: tokio::sync::Mutex::new(stdin),
            next_id: AtomicI64::new(1),
            pending: Mutex::default(),
            published: Mutex::default(),
            changed: Notify::new(),
            versions: Mutex::default(),
            ready: watch::channel(false).0,
            pulls: AtomicBool::new(false),
            alive: AtomicBool::new(true),
            used: Mutex::new(Instant::now()),
            process: Mutex::new(Some((child, tree))),
        });
        tokio::spawn(read_loop(Arc::downgrade(&client), stdout));
        let starting = client.clone();
        let root = root.to_path_buf();
        tokio::spawn(async move { starting.initialize(&root).await });
        Ok(client)
    }

    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn idle_for(&self) -> Duration {
        self.used.lock().unwrap().elapsed()
    }

    /// Sends each file's current text and returns the errors the server published for it after that,
    /// by `deadline`; a file it said nothing new about by then is left out.
    pub async fn check(&self, files: &[(PathBuf, String)], deadline: tokio::time::Instant) -> Vec<(PathBuf, Vec<Diagnostic>)> {
        *self.used.lock().unwrap() = Instant::now();
        if !self.wait_ready(deadline).await {
            return Vec::new();
        }
        let mut sent = Vec::new();
        for (file, language) in files {
            let small = tokio::fs::metadata(file).await.is_ok_and(|meta| meta.len() <= MAX_FILE);
            let Some(text) = tokio::fs::read_to_string(file).await.ok().filter(|_| small) else { continue };
            let key = super::key(file);
            let before = self.generation(&key);
            self.sync(file, &key, language, &text).await;
            sent.push((file.clone(), key, before));
        }
        if self.pulls.load(Ordering::SeqCst) {
            futures_util::future::join_all(sent.iter().map(|(file, _, _)| self.pull(file, deadline))).await;
        }
        self.settle(&sent, deadline).await;
        sent.into_iter().filter_map(|(file, key, before)| self.fresh(&key, before).map(|errors| (file, errors))).collect()
    }

    /// Asks the server to stop, then ends its process tree whether it listened or not.
    pub async fn shutdown(&self) {
        if self.alive() {
            let _ = self.request("shutdown", Value::Null, Duration::from_secs(2)).await;
            let _ = self.notify("exit", Value::Null).await;
        }
        self.kill();
    }

    fn kill(&self) {
        self.alive.store(false, Ordering::SeqCst);
        if let Some((child, tree)) = self.process.lock().unwrap().take() {
            tree.kill();
            drop(child);
        }
        self.changed.notify_waiters();
    }

    async fn initialize(&self, root: &Path) {
        let uri = super::uri(root);
        let name = root.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let params = json!({
            "processId": std::process::id(),
            "clientInfo": { "name": "Drift" },
            "rootUri": uri,
            "workspaceFolders": [{ "uri": uri, "name": name }],
            "capabilities": {
                "textDocument": { "synchronization": { "didSave": true }, "publishDiagnostics": { "versionSupport": true }, "diagnostic": { "dynamicRegistration": true } },
                "workspace": { "configuration": true, "workspaceFolders": true },
                "window": { "workDoneProgress": true },
            },
        });
        let Some(reply) = self.request("initialize", params, INITIALIZE).await else { return self.kill() };
        if !reply["result"]["capabilities"]["diagnosticProvider"].is_null() {
            self.pulls.store(true, Ordering::SeqCst);
        }
        let _ = self.notify("initialized", json!({})).await;
        self.ready.send_replace(true);
    }

    async fn wait_ready(&self, deadline: tokio::time::Instant) -> bool {
        let mut ready = self.ready.subscribe();
        let became = tokio::time::timeout_at(deadline, ready.wait_for(|ready| *ready)).await;
        matches!(became, Ok(Ok(_))) && self.alive()
    }

    async fn sync(&self, file: &Path, key: &str, language: &str, text: &str) {
        let uri = super::uri(file);
        let version = {
            let mut versions = self.versions.lock().unwrap();
            let next = versions.get(key).map_or(1, |version| version + 1);
            versions.insert(key.to_string(), next);
            next
        };
        let _ = match version {
            1 => self.notify("textDocument/didOpen", json!({ "textDocument": { "uri": uri, "languageId": language, "version": 1, "text": text } })).await,
            _ => self.notify("textDocument/didChange", json!({ "textDocument": { "uri": uri, "version": version }, "contentChanges": [{ "text": text }] })).await,
        };
        let _ = self.notify("textDocument/didSave", json!({ "textDocument": { "uri": uri }, "text": text })).await;
    }

    /// Until the server has published for every file sent (and a moment more for a second round), or `deadline`.
    async fn settle(&self, sent: &[(PathBuf, String, u64)], deadline: tokio::time::Instant) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.alive() || sent.iter().all(|(_, key, before)| self.generation(key) > *before) {
                break;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return;
            }
        }
        tokio::time::sleep_until((tokio::time::Instant::now() + SETTLE).min(deadline)).await;
    }

    /// Asks for a file's errors, for a server that is asked rather than publishing; the answer is kept as a published one.
    async fn pull(&self, file: &Path, deadline: tokio::time::Instant) {
        let wait = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Some(reply) = self.request("textDocument/diagnostic", json!({ "textDocument": { "uri": super::uri(file) } }), wait).await else { return };
        if reply["result"]["kind"] == "full" {
            self.publish(&json!({ "uri": super::uri(file), "diagnostics": reply["result"]["items"] }));
        }
    }

    fn generation(&self, key: &str) -> u64 {
        self.published.lock().unwrap().get(key).map_or(0, |(generation, _)| *generation)
    }

    fn fresh(&self, key: &str, before: u64) -> Option<Vec<Diagnostic>> {
        self.published.lock().unwrap().get(key).filter(|(generation, _)| *generation > before).map(|(_, errors)| errors.clone())
    }

    async fn request(&self, method: &str, params: Value, wait: Duration) -> Option<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (answer, answered) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, answer);
        if self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).await.is_err() {
            self.pending.lock().unwrap().remove(&id);
            return None;
        }
        let reply = tokio::time::timeout(wait, answered).await.ok().and_then(Result::ok);
        self.pending.lock().unwrap().remove(&id);
        reply.filter(|reply| reply.get("error").is_none())
    }

    async fn notify(&self, method: &str, params: Value) -> std::io::Result<()> {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params })).await
    }

    async fn send(&self, message: &Value) -> std::io::Result<()> {
        let body = serde_json::to_vec(message)?;
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes()).await?;
        stdin.write_all(&body).await?;
        stdin.flush().await
    }

    async fn handle(&self, message: Value) {
        match (message.get("id").cloned(), message["method"].as_str()) {
            (Some(id), Some(method)) => {
                let registers = message["params"]["registrations"].as_array().into_iter().flatten().any(|registration| registration["method"] == "textDocument/diagnostic");
                if method == "client/registerCapability" && registers {
                    self.pulls.store(true, Ordering::SeqCst);
                }
                let _ = self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": answer(method, &message["params"]) })).await;
            }
            (None, Some("textDocument/publishDiagnostics")) => self.publish(&message["params"]),
            (Some(id), None) => {
                if let Some(waiting) = id.as_i64().and_then(|id| self.pending.lock().unwrap().remove(&id)) {
                    let _ = waiting.send(message);
                }
            }
            _ => {}
        }
    }

    fn publish(&self, params: &Value) {
        let Some(file) = params["uri"].as_str().and_then(super::path_of) else { return };
        let errors = params["diagnostics"].as_array().into_iter().flatten().filter(|found| found["severity"].as_i64() == Some(1)).map(diagnostic).collect();
        let mut published = self.published.lock().unwrap();
        let entry = published.entry(super::key(&file)).or_insert((0, Vec::new()));
        *entry = (entry.0 + 1, errors);
        drop(published);
        self.changed.notify_waiters();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.kill();
    }
}

/// What a server asking the client gets: settings it asks for are left at its defaults; anything else is acknowledged.
fn answer(method: &str, params: &Value) -> Value {
    match method {
        "workspace/configuration" => Value::Array(vec![Value::Null; params["items"].as_array().map_or(0, Vec::len)]),
        _ => Value::Null,
    }
}

fn diagnostic(found: &Value) -> Diagnostic {
    let start = &found["range"]["start"];
    let at = |field: &str| u32::try_from(start[field].as_u64().unwrap_or(0)).unwrap_or(u32::MAX).saturating_add(1);
    let message = found["message"].as_str().unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ");
    Diagnostic { line: at("line"), column: at("character"), message }
}

async fn read_loop(client: Weak<Client>, stdout: ChildStdout) {
    let mut reader = BufReader::new(stdout);
    while let Ok(Some(message)) = read_message(&mut reader).await {
        let Some(client) = client.upgrade() else { return };
        client.handle(message).await;
    }
    if let Some(client) = client.upgrade() {
        client.pending.lock().unwrap().clear();
        client.kill();
    }
}

/// One framed message; `None` at the end of the stream or on a frame that makes no sense.
async fn read_message(reader: &mut BufReader<ChildStdout>) -> std::io::Result<Option<Value>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        if let Some((_, value)) = line.split_once(':').filter(|(name, _)| name.eq_ignore_ascii_case("content-length")) {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let Some(length) = length.filter(|length| *length <= MAX_MESSAGE) else { return Ok(None) };
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body).ok())
}
