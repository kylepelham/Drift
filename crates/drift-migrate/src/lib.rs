//! Imports opencode conversations once, in pages, without writing to opencode's database.

mod map;
mod settings;
mod source;
mod undo;

pub use settings::{
    LeftOut, McpConfigError, OcConfig, OcServer, REPORT, Settings, SettingsReport, import_settings, mcp_config,
};

use drift_engine::session::types::Session;
use drift_engine::store::ImportCheckpoints;
use drift_engine::store::Store;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

pub use undo::Blobs;

const READ_PAGE: usize = 200;
// Small write transactions keep the store's single connection available to the UI.
const WRITE_MESSAGES: usize = 100;
const WRITE_BYTES: usize = 2_000_000;

type Workspaces = HashMap<String, (String, PathBuf)>;

pub struct History<'a> {
    snapshots: &'a drift_engine::session::snapshot::Snapshots,
    runtime: tokio::runtime::Runtime,
}

impl<'a> History<'a> {
    pub fn new(snapshots: &'a drift_engine::session::snapshot::Snapshots) -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;

        Ok(Self { snapshots, runtime })
    }
}

impl Blobs for History<'_> {
    fn store(&mut self, owner: &str, root: &Path, bytes: &[u8]) -> Option<String> {
        let root = drift_engine::tool::canonical(root);
        self.snapshots.bind(owner, &root);

        self.runtime.block_on(self.snapshots.store_bytes(&root, bytes)).ok()
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub imported: usize,
    /// Brought in by an earlier run, or already a conversation here.
    pub known: usize,
    /// Conversations waiting for a workspace to be added, counted by directory.
    pub unmatched: BTreeMap<String, usize>,
    /// Conversations that could not be read or written, with why; a later run tries them again.
    pub failed: Vec<(String, String)>,
    /// Imported calls given undo records.
    pub undoable: usize,
    /// Imported conversations that held prompts opencode queued but never ran; only what ran comes in.
    pub pending: Vec<String>,
}

pub enum Progress<'a> {
    /// This many conversations are to be brought in.
    Planned(usize),
    /// One of them is done with: listed (`Some`), or found already here or failed (`None`).
    Finished(Option<&'a Session>),
}

pub struct SessionImport<'a> {
    pub store: &'a Store,
    pub source: &'a Path,
    /// Drift's archived sessions remain archived, with the retention period starting at import time.
    pub archived: &'a HashSet<String>,
    pub blobs: &'a mut dyn Blobs,
    pub progress: &'a mut dyn FnMut(Progress),
}

struct ImportRun<'a> {
    store: &'a Store,
    source: &'a source::Source,
    records: map::Records,
    checkpoints: ImportCheckpoints<'a>,
    pending: HashSet<String>,
}

pub fn import_sessions(import: SessionImport<'_>) -> rusqlite::Result<Report> {
    let source = source::Source::open(import.source)?;
    let workspaces: Workspaces = import
        .store
        .workspaces()?
        .into_iter()
        .map(|workspace| {
            (
                directory_key(&workspace.path),
                (workspace.id, PathBuf::from(workspace.path)),
            )
        })
        .collect();
    let now = drift_engine::id::now_ms();
    let mut report = Report::default();
    let sessions = source.sessions()?;

    let planned = plan_sessions(import.store, &sessions, &workspaces, &mut report)?;
    if planned.is_empty() {
        return Ok(report);
    }

    (import.progress)(Progress::Planned(planned.len()));
    let run = ImportRun {
        store: import.store,
        source: &source,
        records: undo::records(&source, &planned, now, import.blobs)?,
        pending: source.pending_inputs(),
        checkpoints: import.store.import_checkpoints(),
    };

    import_planned(&run, &planned, import, now, &mut report);

    Ok(report)
}

fn plan_sessions<'a>(
    store: &Store,
    sessions: &'a [source::OcSession],
    workspaces: &Workspaces,
    report: &mut Report,
) -> rusqlite::Result<Vec<undo::Planned<'a>>> {
    let mut planned: Vec<undo::Planned> = Vec::new();
    let mut placed: HashMap<&str, usize> = HashMap::new();

    for session in sessions {
        if store.was_imported(&session.id)? || store.holds_session(&session.id)? {
            report.known += 1;
            continue;
        }

        let Some((owner, root)) = workspace_for(store, session, workspaces, &placed, &planned) else {
            *report.unmatched.entry(session.directory.clone()).or_default() += 1;
            continue;
        };

        placed.insert(&session.id, planned.len());
        planned.push(undo::Planned { session, owner, root });
    }

    Ok(planned)
}

fn import_planned(
    run: &ImportRun<'_>,
    planned: &[undo::Planned<'_>],
    import: SessionImport<'_>,
    now: i64,
    report: &mut Report,
) {
    let mut failed: HashSet<&str> = HashSet::new();

    for plan in planned {
        if plan
            .session
            .parent_id
            .as_deref()
            .is_some_and(|parent| failed.contains(parent))
        {
            failed.insert(&plan.session.id);
            report.failed.push((
                plan.session.id.clone(),
                "the conversation that started it did not import".into(),
            ));
            (import.progress)(Progress::Finished(None));
            continue;
        }

        let archived_at = (plan.session.archived || import.archived.contains(&plan.session.id)).then_some(now);
        match import_one(run, plan, archived_at) {
            Ok(Some((session, undoable))) => {
                report.imported += 1;
                report.undoable += undoable;
                if run.pending.contains(&session.id) {
                    report.pending.push(session.title.clone());
                }
                (import.progress)(Progress::Finished(Some(&session)));
            }
            Ok(None) => {
                report.known += 1;
                (import.progress)(Progress::Finished(None));
            }
            Err(error) => {
                failed.insert(&plan.session.id);
                report.failed.push((plan.session.id.clone(), error.to_string()));
                (import.progress)(Progress::Finished(None));
            }
        }
    }
}

// Subagents inherit their parent's workspace, including when their own directory is elsewhere.
fn workspace_for(
    store: &Store,
    session: &source::OcSession,
    workspaces: &Workspaces,
    placed: &HashMap<&str, usize>,
    planned: &[undo::Planned],
) -> Option<(String, PathBuf)> {
    if let Some(parent) = &session.parent_id {
        if let Some(&index) = placed.get(parent.as_str()) {
            return Some((planned[index].owner.clone(), planned[index].root.clone()));
        }

        let stored = store
            .session(parent)
            .ok()
            .flatten()
            .and_then(|parent| store.workspace(&parent.workspace_id).ok().flatten());
        if let Some(workspace) = stored {
            return Some((workspace.id, PathBuf::from(workspace.path)));
        }
    }

    let by_directory = |path: &str| workspaces.get(&directory_key(path)).cloned();
    by_directory(&session.directory).or_else(|| {
        session
            .worktree
            .as_deref()
            .filter(|root| !matches!(*root, "" | "/"))
            .and_then(by_directory)
    })
}

fn import_one(
    run: &ImportRun<'_>,
    plan: &undo::Planned,
    archived_at: Option<i64>,
) -> rusqlite::Result<Option<(Session, usize)>> {
    let session = map::session(plan.session, &plan.owner);
    if !run.store.begin_import(&session)? {
        return Ok(None);
    }

    let written = write_pages(run, &session.id).and_then(|undoable| {
        let todos = run.source.todos(&session.id)?;
        Ok((undoable, todos))
    });
    let (undoable, todos) = match written {
        Ok(written) => written,
        Err(error) => {
            let _ = run.store.discard_import(&session.id);
            return Err(error);
        }
    };

    let todos: Vec<_> = todos.iter().filter_map(map::todo).collect();
    Ok(run
        .store
        .finish_import(&session.id, archived_at, &todos)?
        .map(|session| (session, undoable)))
}

fn write_pages(run: &ImportRun<'_>, session_id: &str) -> rusqlite::Result<usize> {
    let mut ids = map::Ids::default();
    let mut undoable = 0;
    let mut after = None;

    let mut batch = Vec::new();
    let mut bytes = 0;

    loop {
        let page = run.source.messages_after(session_id, after.as_ref(), READ_PAGE)?;
        if page.is_empty() {
            break;
        }

        for message in &page {
            let parts = run.source.parts(&message.id)?;
            undoable += parts.iter().filter(|part| run.records.contains_key(&part.id)).count();

            let part_bytes: usize = parts.iter().map(|part| part.data.len()).sum();
            bytes += message.data.len() + part_bytes;
            batch.push(map::message(message, &parts, session_id, &mut ids, &run.records));

            if bytes >= WRITE_BYTES || batch.len() >= WRITE_MESSAGES {
                run.store.import_page(&batch)?;
                run.checkpoints.run();
                (batch, bytes) = (Vec::new(), 0);
            }
        }

        after = page.into_iter().last();
    }

    if !batch.is_empty() {
        run.store.import_page(&batch)?;
    }

    Ok(undoable)
}

// opencode and Drift can spell the same directory with different case, slashes and trailing separators.
fn directory_key(path: &str) -> String {
    let path = path.replace('\\', "/").to_lowercase();
    let trimmed = path.trim_end_matches('/');

    match trimmed {
        "" => "/".into(),
        drive if drive.ends_with(':') => format!("{drive}/"),
        other => other.into(),
    }
}

#[cfg(test)]
mod tests;
