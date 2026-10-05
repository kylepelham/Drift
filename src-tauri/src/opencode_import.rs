//! Brings opencode's sign-ins, MCP servers, config, workspaces and conversations in on a background
//! thread: at startup, and again whenever a workspace is added, since its directory may hold
//! conversations an earlier run skipped. Each item comes in once (see `drift-migrate`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};

use drift_engine::event::Event;
use drift_engine::Engine;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use crate::native::Native;
use crate::store::Store;

pub(crate) struct Importer(mpsc::Sender<()>);

impl Importer {
    /// Asks for another run; requests made while one runs fold into one more.
    pub(crate) fn request(&self) {
        let _ = self.0.send(());
    }
}

pub(crate) fn start(app: &AppHandle) -> Importer {
    let (requests, received) = mpsc::channel();
    let app = app.clone();
    std::thread::spawn(move || {
        while received.recv().is_ok() {
            while received.try_recv().is_ok() {}
            run(&app);
        }
    });
    let importer = Importer(requests);
    importer.request();
    importer
}

fn run(app: &AppHandle) {
    let Ok(dir) = crate::engine_db::opencode_data_dir() else { return };
    let store = app.state::<Store>();
    let engine = app.state::<Native>().engine().clone();
    import_settings(&engine, &store, &dir);
    for source in sources(&dir) {
        match store.import_opencode_workspaces(&source) {
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
        match drift_migrate::import_sessions(&engine.store, &source, &archived, &mut history, &mut announce) {
            Ok(report) if report.imported > 0 || !report.failed.is_empty() => {
                eprintln!("opencode import from {}: {} imported ({} edits can be undone), {} failed", source.display(), report.imported, report.undoable, report.failed.len());
                for (id, error) in &report.failed {
                    eprintln!("opencode import: {id}: {error}");
                }
            }
            Ok(_) => {}
            Err(error) => eprintln!("opencode import from {}: {error}", source.display()),
        }
    }
}

/// opencode's sign-ins, MCP servers (its own and those Drift's old manager kept) and global config.
fn import_settings(engine: &Arc<Engine>, store: &Store, data_dir: &Path) {
    let read = |path: PathBuf| std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str::<Value>(&drift_engine::config::jsonc::strip(&text)).ok());
    let config_dir = opencode_config_dir();
    let config = config_dir.as_ref().and_then(|dir| read(dir.join("opencode.json")).or_else(|| read(dir.join("opencode.jsonc"))));
    let state = store.mcp_state().ok();
    let approved: HashSet<String> = state.iter().flat_map(|state| &state.decisions).filter(|decision| decision.decision == "approved").map(|decision| decision.fingerprint.clone()).collect();
    let server = |name: &str, definition: &Value| drift_migrate::OcServer {
        name: name.into(),
        definition: definition.clone(),
        approved: crate::mcp_external::fingerprint(name, definition).is_some_and(|fingerprint| approved.contains(&fingerprint)),
    };
    let mut servers: Vec<drift_migrate::OcServer> = state.iter().flat_map(|state| &state.servers).map(|row| server(&row.name, &row.config)).collect();
    servers.extend(config.as_ref().and_then(|config| config["mcp"].as_object()).into_iter().flatten().map(|(name, definition)| server(name, definition)));
    let settings = drift_migrate::Settings { auth: read(data_dir.join("auth.json")), config, config_dir: config_dir.unwrap_or_default(), servers };
    let providers: Vec<String> = engine.catalog.read().unwrap().providers.keys().cloned().collect();
    let Some(home) = drift_engine::config::home() else { return };
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
        }
        Err(error) => eprintln!("opencode import: settings: {error}"),
    }
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
}
