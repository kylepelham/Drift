//! Brings opencode's sign-ins, MCP servers, config, workspaces and conversations in on a background
//! thread: at startup, and again whenever a workspace is added, since its directory may hold
//! conversations an earlier run skipped. Each item comes in once (see `drift-migrate`).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};

use drift_engine::event::Event;
use drift_engine::Engine;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::native::Native;
use crate::store::Store;

pub(crate) struct Importer {
    requests: mpsc::Sender<()>,
    /// What the last run that brought anything in did, until the window takes it to show once.
    summary: Arc<Mutex<Option<Summary>>>,
}

impl Importer {
    /// Asks for another run; requests made while one runs fold into one more.
    pub(crate) fn request(&self) {
        let _ = self.requests.send(());
    }
}

/// What a run brought in and left behind, shown to the user once.
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Summary {
    conversations: usize,
    undoable: usize,
    /// Conversations holding prompts opencode queued but never ran.
    pending: Vec<String>,
    /// Folders that are not workspaces, with how many conversations wait for them.
    waiting: BTreeMap<String, usize>,
    sign_ins: Vec<String>,
    servers: Vec<String>,
    servers_off: Vec<String>,
    files: usize,
    left_out: drift_migrate::LeftOut,
    failed: usize,
}

impl Summary {
    fn worth_showing(&self) -> bool {
        let left = &self.left_out;
        let left_out = !(left.sign_ins.is_empty() && left.plugins.is_empty() && left.settings.is_empty() && left.servers.is_empty() && left.failed.is_empty());
        self.conversations > 0 || !self.sign_ins.is_empty() || !self.servers.is_empty() || self.files > 0 || left_out || !self.pending.is_empty() || self.failed > 0
    }
}

pub(crate) fn start(app: &AppHandle) -> Importer {
    let (requests, received) = mpsc::channel();
    let summary = Arc::new(Mutex::new(None));
    let (app, kept) = (app.clone(), summary.clone());
    std::thread::spawn(move || {
        while received.recv().is_ok() {
            while received.try_recv().is_ok() {}
            run(&app, &kept);
        }
    });
    let importer = Importer { requests, summary };
    importer.request();
    importer
}

/// The last import's summary, once: a second call returns nothing until another run brings something in.
#[tauri::command]
pub(crate) fn opencode_import_summary(importer: State<Importer>) -> Option<Summary> {
    importer.summary.lock().unwrap().take()
}

fn run(app: &AppHandle, kept: &Mutex<Option<Summary>>) {
    let Some(dir) = opencode_data_dir() else { return };
    let store = app.state::<Store>();
    let engine = app.state::<Native>().engine().clone();
    let mut summary = Summary::default();
    if let Some(report) = import_settings(&engine, &store, &dir) {
        summary.sign_ins = report.credentials;
        summary.servers = report.servers;
        summary.servers_off = report.disabled_servers;
        summary.files = report.files.len();
        summary.left_out = report.left_out;
    }
    for source in sources(&dir) {
        import_source(app, &engine, &store, &source, &mut summary);
    }
    if summary.worth_showing() {
        *kept.lock().unwrap() = Some(summary);
        let _ = app.emit("opencode-import-done", ());
    }
}

/// A folder opencode's own tests or tools made, never one a person works in.
fn scratch(directory: &str) -> bool {
    let path = directory.replace('\\', "/").to_lowercase();
    path.contains("/appdata/local/temp/") || path.starts_with("/tmp/")
}

fn import_source(app: &AppHandle, engine: &Arc<Engine>, store: &Store, source: &Path, summary: &mut Summary) {
    match store.import_opencode_workspaces(source) {
        Ok(0) => {}
        Ok(_) => {
            let _ = app.emit("workspaces-changed", ());
        }
        Err(error) => eprintln!("opencode import: workspaces from {}: {error}", source.display()),
    }
    let archived: HashSet<String> = store.archived().map(|rows| rows.into_iter().map(|row| row.session_id).collect()).unwrap_or_default();
    let (mut done, mut total) = (0usize, 0usize);
    // Each conversation is announced as it lands; the window shows how far the import has got.
    let mut announce = |step: drift_migrate::Progress| {
        match step {
            drift_migrate::Progress::Planned(count) => total = count,
            drift_migrate::Progress::Finished(session) => {
                done += 1;
                if let Some(session) = session {
                    engine.hub.publish(Event::SessionCreated { session: session.clone() });
                }
            }
        }
        let _ = app.emit("opencode-import", serde_json::json!({ "done": done, "total": total }));
    };
    let mut history = match drift_migrate::History::new(&engine.snapshots) {
        Ok(history) => history,
        Err(error) => return eprintln!("opencode import: {error}"),
    };
    match drift_migrate::import_sessions(&engine.store, source, &archived, &mut history, &mut announce) {
        Ok(report) => {
            if report.imported > 0 || !report.failed.is_empty() {
                eprintln!("opencode import from {}: {} imported ({} edits can be undone), {} failed", source.display(), report.imported, report.undoable, report.failed.len());
                for (id, error) in &report.failed {
                    eprintln!("opencode import: {id}: {error}");
                }
            }
            summary.conversations += report.imported;
            summary.undoable += report.undoable;
            summary.failed += report.failed.len();
            summary.pending.extend(report.pending);
            for (directory, count) in report.unmatched.into_iter().filter(|(directory, _)| !scratch(directory)) {
                *summary.waiting.entry(directory).or_default() += count;
            }
        }
        Err(error) => eprintln!("opencode import from {}: {error}", source.display()),
    }
}

/// opencode's sign-ins, MCP servers (its own and those Drift's old manager kept) and global config.
fn import_settings(engine: &Arc<Engine>, store: &Store, data_dir: &Path) -> Option<drift_migrate::SettingsReport> {
    let read = |path: PathBuf| std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str::<Value>(&drift_engine::config::jsonc::strip(&text)).ok());
    let config_dir = opencode_config_dir();
    let config = config_dir.as_ref().and_then(|dir| read(dir.join("opencode.json")).or_else(|| read(dir.join("opencode.jsonc"))));
    let state = store.mcp_state().ok();
    let approved: HashSet<String> = state.iter().flat_map(|state| &state.decisions).filter(|decision| decision.decision == "approved").map(|decision| decision.fingerprint.clone()).collect();
    let server = |name: &str, definition: &Value| drift_migrate::OcServer {
        name: name.into(),
        definition: definition.clone(),
        approved: fingerprint(name, definition).is_some_and(|fingerprint| approved.contains(&fingerprint)),
    };
    let mut servers: Vec<drift_migrate::OcServer> = state.iter().flat_map(|state| &state.servers).map(|row| server(&row.name, &row.config)).collect();
    servers.extend(config.as_ref().and_then(|config| config["mcp"].as_object()).into_iter().flatten().map(|(name, definition)| server(name, definition)));
    let settings = drift_migrate::Settings { auth: read(data_dir.join("auth.json")), config, config_dir: config_dir.unwrap_or_default(), servers };
    let providers: Vec<String> = engine.catalog.read().unwrap().providers.keys().cloned().collect();
    let home = drift_engine::config::home()?;
    match drift_migrate::import_settings(&engine.store, &engine.credentials, &providers, &home, &settings) {
        Ok(report) => {
            if !report.credentials.is_empty() {
                engine.hub.publish(Event::CatalogUpdated {});
            }
            if !report.servers.is_empty() {
                let engine = engine.clone();
                tauri::async_runtime::spawn(async move { engine.connect_all_mcp() });
            }
            if !report.credentials.is_empty() || !report.servers.is_empty() || !report.skipped.is_empty() || !report.files.is_empty() {
                eprintln!("opencode import: sign-ins {:?}, MCP servers {:?} (off: {:?}), config {:?}, files {:?}", report.credentials, report.servers, report.disabled_servers, report.config_written, report.files);
                for line in &report.skipped {
                    eprintln!("opencode import: left out {line}");
                }
            }
            Some(report)
        }
        Err(error) => {
            eprintln!("opencode import: settings: {error}");
            None
        }
    }
}

/// The fingerprint Drift's old MCP approval step recorded for a named definition (`enabled` aside):
/// a server is imported switched on only when this matches an approval.
fn fingerprint(name: &str, definition: &Value) -> Option<String> {
    use sha2::Digest;
    let effective: serde_json::Map<String, Value> = definition.as_object()?.iter().filter(|(key, _)| key.as_str() != "enabled").map(|(key, value)| (key.clone(), value.clone())).collect();
    let serialized = canonical(&Value::Array(vec![Value::String(name.to_string()), Value::Object(effective)]))?;
    Some(format!("sha256:{:x}", sha2::Sha256::digest(serialized.as_bytes())))
}

/// As the approval step serialized it: keys sorted by UTF-16 code unit, numbers as `JSON.stringify` writes them.
fn canonical(value: &Value) -> Option<String> {
    match value {
        Value::Null | Value::Bool(_) | Value::String(_) => serde_json::to_string(value).ok(),
        Value::Number(number) => match number.as_f64().filter(|float| float.fract() == 0.0 && float.abs() <= 9_007_199_254_740_991.0) {
            Some(whole) => Some(format!("{}", whole as i64)),
            None => serde_json::to_string(number).ok(),
        },
        Value::Array(items) => Some(format!("[{}]", items.iter().map(canonical).collect::<Option<Vec<_>>>()?.join(","))),
        Value::Object(entries) => {
            let mut sorted: Vec<(&String, &Value)> = entries.iter().collect();
            sorted.sort_by_key(|(key, _)| key.encode_utf16().collect::<Vec<_>>());
            let parts = sorted.into_iter().map(|(key, item)| Some(format!("{}:{}", serde_json::to_string(key).ok()?, canonical(item)?))).collect::<Option<Vec<_>>>()?;
            Some(format!("{{{}}}", parts.join(",")))
        }
    }
}

/// Where opencode keeps its databases and `auth.json`, as opencode looks for it.
fn opencode_data_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME").map(|root| PathBuf::from(root).join("opencode")).or_else(|| drift_engine::config::home().map(|home| home.join(".local").join("share").join("opencode")))
}

/// Where opencode keeps its global config, as opencode looks for it.
fn opencode_config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME").map(|root| PathBuf::from(root).join("opencode")).or_else(|| drift_engine::config::home().map(|home| home.join(".config").join("opencode")))
}

/// The shared database first, then any channel database an older build wrote apart from it.
fn sources(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "db") && path.file_stem().and_then(|stem| stem.to_str()).is_some_and(|stem| stem.starts_with("opencode")))
        .collect();
    found.sort_by_key(|path| (path.file_name().is_none_or(|name| name != "opencode.db"), path.clone()));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approvals_match_the_fingerprint_drifts_old_approval_step_recorded() {
        let definition = serde_json::json!({ "type": "remote", "url": "https://example.com/mcp", "headers": { "Authorization": "Bearer x" }, "enabled": true, "timeout": 30000 });
        assert_eq!(fingerprint("docs", &definition).as_deref(), Some("sha256:933d9f99f6458ef8004d9f0e9b5fe8768211fe67a62e7baa87b08d8e9a5220dd"), "the vector the old plugin and locator shared");
        let disabled = serde_json::json!({ "type": "remote", "url": "https://example.com/mcp", "headers": { "Authorization": "Bearer x" }, "enabled": false, "timeout": 30000.0 });
        assert_eq!(fingerprint("docs", &disabled), fingerprint("docs", &definition), "enabled is left out and a whole float reads as the integer");
    }

    #[test]
    fn the_shared_database_goes_first_and_only_opencode_databases_are_read() {
        let dir = std::env::temp_dir().join(format!("drift-import-sources-{}", drift_engine::id::new("t")));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["opencode-master.db", "opencode.db", "opencode.db-wal", "other.db", "opencode-dev.db"] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        let names: Vec<String> = super::sources(&dir).iter().map(|path| path.file_name().unwrap().to_string_lossy().into_owned()).collect();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(names, ["opencode.db", "opencode-dev.db", "opencode-master.db"]);
    }

    #[test]
    fn temp_folders_never_wait_for_a_workspace() {
        assert!(scratch("C:\\Users\\Kyle\\AppData\\Local\\Temp\\opencode-test\\repo"));
        assert!(scratch("/tmp/opencode/repo"));
        assert!(!scratch("C:\\Users\\Kyle\\Desktop\\C++\\Drift"));
    }

    #[test]
    fn a_run_that_brought_nothing_in_shows_nothing() {
        assert!(!Summary::default().worth_showing());
        assert!(Summary { conversations: 1, ..Summary::default() }.worth_showing());
        let left_out = drift_migrate::LeftOut { plugins: vec!["oh-my-opencode".into()], ..Default::default() };
        assert!(Summary { left_out, ..Summary::default() }.worth_showing());
    }
}
