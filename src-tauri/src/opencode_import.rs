//! Brings opencode's workspaces and conversations in on a background thread: at startup, and again
//! whenever a workspace is added, since its directory may hold conversations an earlier run skipped.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use drift_engine::event::Event;
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
    for source in sources(&dir) {
        match store.import_opencode_workspaces(&source) {
            Ok(0) => {}
            Ok(_) => {
                let _ = app.emit("workspaces-changed", ());
            }
            Err(error) => eprintln!("opencode import: workspaces from {}: {error}", source.display()),
        }
        let archived: HashSet<String> = store.archived().map(|rows| rows.into_iter().map(|row| row.session_id).collect()).unwrap_or_default();
        let mut announce = |session: &drift_engine::session::types::Session| {
            engine.hub.publish(Event::SessionCreated { session: session.clone() });
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
