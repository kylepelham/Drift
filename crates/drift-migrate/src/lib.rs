//! Brings opencode's conversations into Drift's store. Reads opencode's database, never writes it;
//! each conversation lands once, whole or not at all, in the workspace whose directory it ran in.

mod map;
mod source;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use drift_engine::session::types::Session;
use drift_engine::store::Store;

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
}

/// Imports every conversation in `source` not yet brought in, calling `imported` with each as it
/// lands. `archived` names conversations Drift itself archived; those and the ones opencode archived
/// arrive archived as of now.
pub fn import_sessions(store: &Store, source: &Path, archived: &HashSet<String>, imported: &mut dyn FnMut(&Session)) -> rusqlite::Result<Report> {
    let source = source::Source::open(source)?;
    let workspaces: HashMap<String, String> = store.workspaces()?.into_iter().map(|workspace| (directory_key(&workspace.path), workspace.id)).collect();
    let now = drift_engine::id::now_ms();
    let mut report = Report::default();
    let mut placed: HashMap<String, String> = HashMap::new();
    for session in source.sessions()? {
        if store.was_imported(&session.id)? || store.session(&session.id)?.is_some() {
            report.known += 1;
            continue;
        }
        let Some(workspace_id) = workspace_for(store, &session, &workspaces, &placed) else {
            *report.unmatched.entry(session.directory.clone()).or_default() += 1;
            continue;
        };
        let placement = map::Placement { workspace_id: &workspace_id, archived: session.archived || archived.contains(&session.id), now };
        match import_one(store, &source, &session, &placement) {
            Ok(Some(native)) => {
                report.imported += 1;
                placed.insert(session.id.clone(), workspace_id);
                imported(&native);
            }
            Ok(None) => report.known += 1,
            Err(error) => report.failed.push((session.id.clone(), error.to_string())),
        }
    }
    Ok(report)
}

/// A subagent goes with its parent; anything else to the workspace it ran in, else the one holding its repository.
fn workspace_for(store: &Store, session: &source::OcSession, workspaces: &HashMap<String, String>, placed: &HashMap<String, String>) -> Option<String> {
    if let Some(parent) = &session.parent_id {
        let parents = placed.get(parent).cloned().or_else(|| store.session(parent).ok().flatten().map(|parent| parent.workspace_id));
        if parents.is_some() {
            return parents;
        }
    }
    let by = |path: &str| workspaces.get(&directory_key(path)).cloned();
    by(&session.directory).or_else(|| session.worktree.as_deref().filter(|root| !matches!(*root, "" | "/")).and_then(by))
}

fn import_one(store: &Store, source: &source::Source, session: &source::OcSession, placement: &map::Placement) -> rusqlite::Result<Option<Session>> {
    let messages = source.messages(&session.id)?;
    let todos = source.todos(&session.id).unwrap_or_default();
    let conversation = map::conversation(session, &messages, &todos, placement);
    Ok(store.import_session(&conversation)?.then_some(conversation.session))
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
