//! Brings opencode's conversations into Drift's store. Reads opencode's database, never writes it;
//! each conversation lands once, in the workspace whose directory it ran in, written a page at a time
//! and listed only once its last page is in. Recent edits get undo records (`undo`).

#![expect(
    clippy::too_many_arguments,
    reason = "parameter structs replace these in the lint pass; remove with it"
)]

mod map;
mod settings;
mod source;
mod undo;

pub use settings::{LeftOut, OcConfig, OcServer, REPORT, Settings, SettingsReport, import_settings, mcp_config};

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use drift_engine::session::types::Session;
use drift_engine::store::Store;

pub use undo::Blobs;

/// Rebuilt versions kept in the engine's own undo history, where its undo reads them.
pub struct History<'a> {
    snapshots: &'a drift_engine::session::snapshot::Snapshots,
    runtime: tokio::runtime::Runtime,
}

impl<'a> History<'a> {
    pub fn new(snapshots: &'a drift_engine::session::snapshot::Snapshots) -> std::io::Result<Self> {
        Ok(Self {
            snapshots,
            runtime: tokio::runtime::Builder::new_current_thread().enable_all().build()?,
        })
    }
}

impl Blobs for History<'_> {
    fn store(&mut self, owner: &str, root: &Path, bytes: &[u8]) -> Option<String> {
        let root = drift_engine::tool::canonical(root);
        self.snapshots.bind(owner, &root);
        self.runtime.block_on(self.snapshots.store_bytes(&root, bytes)).ok()
    }
}

/// Messages read from opencode at once.
const READ_PAGE: usize = 200;
/// A page written to the store holds at most this many messages or about this many bytes, so no
/// transaction holds the one connection long enough for the UI to notice.
const WRITE_MESSAGES: usize = 100;
const WRITE_BYTES: usize = 2_000_000;

/// What one run did; conversations neither imported nor known are counted under their directory.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub imported: usize,
    /// Brought in by an earlier run, or already a conversation here.
    pub known: usize,
    /// Directories that are not a Drift workspace, with how many conversations ran there; a later run
    /// imports them once the workspace is added.
    pub unmatched: BTreeMap<String, usize>,
    /// Conversations that could not be read or written, with why; a later run tries them again.
    pub failed: Vec<(String, String)>,
    /// Imported calls given undo records.
    pub undoable: usize,
    /// Imported conversations that held prompts opencode queued but never ran; only what ran comes in.
    pub pending: Vec<String>,
}

/// How far a run has got, for a progress display.
pub enum Progress<'a> {
    /// This many conversations are to be brought in.
    Planned(usize),
    /// One of them is done with: listed (`Some`), or found already here or failed (`None`).
    Finished(Option<&'a Session>),
}

/// Imports every conversation in `source` not yet brought in, telling `progress` how far it has got.
/// `archived` names conversations Drift itself archived; those and the ones opencode archived arrive
/// archived as of now. Rebuilt versions for undo go to `blobs`.
pub fn import_sessions(
    store: &Store,
    source: &Path,
    archived: &HashSet<String>,
    blobs: &mut dyn Blobs,
    progress: &mut dyn FnMut(Progress),
) -> rusqlite::Result<Report> {
    let source = source::Source::open(source)?;
    let workspaces: HashMap<String, (String, PathBuf)> = store
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
    let mut planned: Vec<undo::Planned> = Vec::new();
    let mut placed: HashMap<&str, usize> = HashMap::new();
    for session in &sessions {
        if store.was_imported(&session.id)? || store.holds_session(&session.id)? {
            report.known += 1;
            continue;
        }
        let Some((owner, root)) = workspace_for(store, session, &workspaces, &placed, &planned) else {
            *report.unmatched.entry(session.directory.clone()).or_default() += 1;
            continue;
        };
        placed.insert(&session.id, planned.len());
        planned.push(undo::Planned { session, owner, root });
    }
    if planned.is_empty() {
        return Ok(report);
    }
    progress(Progress::Planned(planned.len()));
    let records = undo::records(&source, &planned, now, blobs)?;
    let pending = source.pending_inputs();
    let checkpoints = store.import_checkpoints();
    let mut failed: HashSet<&str> = HashSet::new();
    for plan in &planned {
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
            progress(Progress::Finished(None));
            continue;
        }
        let archived_at = (plan.session.archived || archived.contains(&plan.session.id)).then_some(now);
        match import_one(store, &source, plan, archived_at, &records, &checkpoints) {
            Ok(Some((session, undoable))) => {
                report.imported += 1;
                report.undoable += undoable;
                if pending.contains(&session.id) {
                    report.pending.push(session.title.clone());
                }
                progress(Progress::Finished(Some(&session)));
            }
            Ok(None) => {
                report.known += 1;
                progress(Progress::Finished(None));
            }
            Err(error) => {
                failed.insert(&plan.session.id);
                report.failed.push((plan.session.id.clone(), error.to_string()));
                progress(Progress::Finished(None));
            }
        }
    }
    Ok(report)
}

/// A subagent goes with its parent; anything else to the workspace it ran in, else the one holding its repository.
fn workspace_for(
    store: &Store,
    session: &source::OcSession,
    workspaces: &HashMap<String, (String, PathBuf)>,
    placed: &HashMap<&str, usize>,
    planned: &[undo::Planned],
) -> Option<(String, PathBuf)> {
    if let Some(parent) = &session.parent_id {
        if let Some(&at) = placed.get(parent.as_str()) {
            return Some((planned[at].owner.clone(), planned[at].root.clone()));
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
    let by = |path: &str| workspaces.get(&directory_key(path)).cloned();
    by(&session.directory).or_else(|| {
        session
            .worktree
            .as_deref()
            .filter(|root| !matches!(*root, "" | "/"))
            .and_then(by)
    })
}

/// The conversation, listed, with how many of its calls can be undone; `None` when it was already here.
fn import_one(
    store: &Store,
    source: &source::Source,
    plan: &undo::Planned,
    archived_at: Option<i64>,
    records: &map::Records,
    checkpoints: &drift_engine::store::ImportCheckpoints,
) -> rusqlite::Result<Option<(Session, usize)>> {
    let session = map::session(plan.session, &plan.owner);
    if !store.begin_import(&session)? {
        return Ok(None);
    }
    let written = write_pages(store, source, &session.id, records, checkpoints)
        .and_then(|undoable| Ok((undoable, source.todos(&session.id)?)));
    let (undoable, todos) = match written {
        Ok(written) => written,
        Err(error) => {
            let _ = store.discard_import(&session.id);
            return Err(error);
        }
    };
    let todos: Vec<_> = todos.iter().filter_map(map::todo).collect();
    Ok(store
        .finish_import(&session.id, archived_at, &todos)?
        .map(|session| (session, undoable)))
}

/// Reads and writes the conversation a page at a time; the number of its calls given undo records.
fn write_pages(
    store: &Store,
    source: &source::Source,
    session_id: &str,
    records: &map::Records,
    checkpoints: &drift_engine::store::ImportCheckpoints,
) -> rusqlite::Result<usize> {
    let mut ids = map::Ids::default();
    let (mut batch, mut bytes, mut undoable) = (Vec::new(), 0, 0);
    let mut after = None;
    loop {
        let page = source.messages_after(session_id, after.as_ref(), READ_PAGE)?;
        if page.is_empty() {
            break;
        }
        for message in &page {
            let parts = source.parts(&message.id)?;
            undoable += parts.iter().filter(|part| records.contains_key(&part.id)).count();
            bytes += message.data.len() + parts.iter().map(|part| part.data.len()).sum::<usize>();
            batch.push(map::message(message, &parts, session_id, &mut ids, records));
            if bytes >= WRITE_BYTES || batch.len() >= WRITE_MESSAGES {
                store.import_page(&batch)?;
                checkpoints.run();
                (batch, bytes) = (Vec::new(), 0);
            }
        }
        after = page.into_iter().last();
    }
    if !batch.is_empty() {
        store.import_page(&batch)?;
    }
    Ok(undoable)
}

/// One spelling per directory: case and slash style differ between opencode and the workspace list.
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
