//! Undo and redo: hide a conversation back to one of its prompts and put back what its calls changed,
//! its subagents' calls included. Only those files: one edited since the session last wrote it is
//! kept and reported, and nothing else in the workspace is touched. Nothing is deleted until the next
//! prompt commits the undo (see `Store::admit_prompt`).

use std::future::Future;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;

use super::snapshot::FileChange;
use super::types::{MessageWithParts, Part, Revert, Role, Session};
use crate::event::Event;
use crate::Engine;

/// How long an undo waits for the turn it stopped to finish its stop, a tool's cleanup included.
#[cfg(not(test))]
const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(15);
#[cfg(test)]
const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

#[derive(Debug)]
pub enum RevertError {
    NoSession,
    /// Only a prompt the user sent can be the point to go back to.
    NotAPrompt,
    /// A job that would not stop holds the session; its files and history are in motion.
    Busy,
    Files(String),
    Store(rusqlite::Error),
}

impl From<rusqlite::Error> for RevertError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

/// The session after an undo or redo, and the files it left alone.
#[derive(Debug, Serialize, ToSchema)]
pub struct Undone {
    pub session: Session,
    /// Changed by someone else since the session last wrote them.
    pub kept: Vec<String>,
    /// Seen changing while a command ran, which does not show who changed them.
    pub unattributed: Vec<String>,
}

/// A path's net change over a range, and whether its chain of changes was broken by someone else's edit.
struct Net {
    /// The workspace whose history and directory the change belongs to.
    owner: String,
    change: FileChange,
    broken: bool,
}

#[derive(Default)]
struct Shifted {
    kept: Vec<String>,
    unattributed: Vec<String>,
    /// What the shift changed, oldest first, kept until the marker is saved so a failed save can put it back.
    applied: Vec<Applied>,
    /// The turns of every file the shift may change, held until the marker is saved or the files put back.
    _turns: Vec<crate::tool::lock::Held>,
}

/// A path a shift changed, and the content it had before, to restore if a later path fails.
struct Applied {
    workspace: PathBuf,
    path: String,
    previous: Option<String>,
}

enum Direction {
    /// Put each file back to how it was before the first change in the range.
    Back,
    /// Put each file forward to how the last change in the range left it.
    Forward,
}

impl Engine {
    /// Hides `message_id`, a prompt, with everything after it, and undoes what those turns changed.
    /// Called again while undone it moves the point either way, and the files follow.
    pub async fn revert(&self, session_id: &str, message_id: &str) -> Result<Undone, RevertError> {
        self.exclusively(session_id, self.revert_claimed(session_id, message_id)).await
    }

    /// Brings back everything an undo hid, and redoes what those turns changed.
    pub async fn unrevert(&self, session_id: &str) -> Result<Undone, RevertError> {
        self.exclusively(session_id, self.unrevert_claimed(session_id)).await
    }

    /// Runs `work` holding the session, so no turn starts while its files and history change
    /// underneath. A running turn is stopped first: asking to undo is asking for it to end.
    async fn exclusively<T>(&self, session_id: &str, work: impl Future<Output = Result<T, RevertError>>) -> Result<T, RevertError> {
        let deadline = tokio::time::Instant::now() + STOP_WAIT;
        while !self.turns.claim(session_id, &CancellationToken::new()) {
            self.abort(session_id);
            let until = CancellationToken::new();
            if tokio::time::timeout_at(deadline, self.turns.wait_idle(session_id, &until)).await.is_err() {
                return Err(RevertError::Busy);
            }
        }
        let result = work.await;
        self.turns.release(session_id);
        result
    }

    async fn revert_claimed(&self, session_id: &str, message_id: &str) -> Result<Undone, RevertError> {
        let session = self.store.session(session_id)?.ok_or(RevertError::NoSession)?;
        if !self.store.transcript(session_id)?.iter().any(|m| m.info.id == message_id && is_prompt(m)) {
            return Err(RevertError::NotAPrompt);
        }
        let shifted = match session.revert.as_ref().map(|r| r.message_id.as_str()) {
            None => self.shift(&session, message_id, None, Direction::Back).await?,
            Some(current) if message_id < current => self.shift(&session, message_id, Some(current), Direction::Back).await?,
            Some(current) if message_id > current => self.shift(&session, current, Some(message_id), Direction::Forward).await?,
            Some(_) => Shifted::default(),
        };
        let revert = Revert { message_id: message_id.into(), kept: shifted.kept.clone() };
        self.mark_or_put_back(session_id, Some(&revert), shifted).await
    }

    async fn unrevert_claimed(&self, session_id: &str) -> Result<Undone, RevertError> {
        let session = self.store.session(session_id)?.ok_or(RevertError::NoSession)?;
        let Some(revert) = &session.revert else { return Ok(Undone { session, kept: Vec::new(), unattributed: Vec::new() }) };
        let shifted = self.shift(&session, &revert.message_id, None, Direction::Forward).await?;
        self.mark_or_put_back(session_id, None, shifted).await
    }

    /// Saves the marker that matches the files just shifted; if it cannot be saved, the files go back too, so both sides still agree.
    async fn mark_or_put_back(&self, session_id: &str, revert: Option<&Revert>, shifted: Shifted) -> Result<Undone, RevertError> {
        match self.mark(session_id, revert) {
            Ok(session) => Ok(Undone { session, kept: shifted.kept, unattributed: shifted.unattributed }),
            Err(error) => Err(self.put_back(shifted.applied, format!("could not save the conversation's undo point ({error})")).await),
        }
    }

    /// Applies the net change of the turns in `[from, to)` in one direction. A file whose content is
    /// not what that change expects was edited by someone else since; it is kept, not overwritten. A
    /// change only observed while a command ran is never applied: it may not be the session's. Each
    /// change is applied where its owning workspace is now, whichever workspace the session is in.
    async fn shift(&self, session: &Session, from: &str, to: Option<&str>, direction: Direction) -> Result<Shifted, RevertError> {
        let nets = self.net_changes(session, from, to)?;
        let mut shifted = Shifted { _turns: self.turns_for(&nets).await, ..Shifted::default() };
        for net in nets {
            match self.shift_one(net, &direction, &mut shifted).await {
                Ok(Some(done)) => shifted.applied.push(done),
                Ok(None) => {}
                // All or nothing: what this shift already put back is returned to how it was.
                Err(error) => return Err(self.put_back(shifted.applied, error).await),
            }
        }
        Ok(shifted)
    }

    /// The turns of the files `nets` may change, workspace by workspace in one order, all taken before any is changed.
    async fn turns_for(&self, nets: &[Net]) -> Vec<crate::tool::lock::Held> {
        let mut by_workspace: std::collections::BTreeMap<PathBuf, Vec<PathBuf>> = std::collections::BTreeMap::new();
        for net in nets.iter().filter(|net| !net.change.observed && !net.broken) {
            if let Some(workspace) = self.root_of(&net.owner) {
                by_workspace.entry(workspace.clone()).or_default().push(workspace.join(&net.change.path));
            }
        }
        let mut held = Vec::new();
        for (workspace, paths) in by_workspace {
            held.push(crate::tool::lock::files(&workspace, &paths).await);
        }
        held
    }

    /// Applies one path's change, or records why it is left alone; `Some` names what to put back if a later path fails.
    async fn shift_one(&self, Net { owner, change, broken }: Net, direction: &Direction, shifted: &mut Shifted) -> Result<Option<Applied>, String> {
        if change.observed {
            shifted.unattributed.push(change.path);
            return Ok(None);
        }
        // Someone else changed the file between two of the session's writes, or the workspace is gone.
        let Some(workspace) = self.root_of(&owner).filter(|_| !broken) else {
            shifted.kept.push(change.path);
            return Ok(None);
        };
        let (expected, target) = match direction {
            Direction::Back => (change.after, change.before),
            Direction::Forward => (change.before, change.after),
        };
        let current = self.snapshots.current(&workspace, &change.path).await.map_err(|e| e.to_string())?;
        if current != expected {
            shifted.kept.push(change.path);
            return Ok(None);
        }
        self.snapshots.put(&self.store, &workspace, &change.path, target.as_deref()).await.map_err(|e| format!("{}: {e}", change.path))?;
        Ok(Some(Applied { workspace, path: change.path, previous: expected }))
    }

    /// Returns the paths a failed shift already changed to their content before it, newest first; the shift still holds their turns.
    async fn put_back(&self, applied: Vec<Applied>, error: String) -> RevertError {
        let mut stuck = Vec::new();
        for Applied { workspace, path, previous } in applied.into_iter().rev() {
            if let Err(failure) = self.snapshots.put(&self.store, &workspace, &path, previous.as_deref()).await {
                stuck.push(format!("{path} ({failure})"));
            }
        }
        let state = if stuck.is_empty() { "no file was changed".to_string() } else { format!("these could not be put back: {}", stuck.join("; ")) };
        RevertError::Files(format!("{error}; {state}"))
    }

    /// Per path, the state before its first change and after its last one in `[from, to)`, across the
    /// session and its subagents, in the order the calls' writes finished (their `at` stamp; records
    /// made before it fall back to their message's id). A path whose next change did not start where
    /// the previous one ended is `broken`.
    fn net_changes(&self, session: &Session, from: &str, to: Option<&str>) -> Result<Vec<Net>, RevertError> {
        let mut calls: Vec<(String, String, String, Vec<FileChange>)> = Vec::new();
        for member in self.store.session_tree(&session.id)? {
            for message in self.store.transcript(&member)?.iter().filter(|m| m.info.id.as_str() >= from && to.is_none_or(|to| m.info.id.as_str() < to)) {
                for row in &message.parts {
                    if let Some(record) = recorded_changes(&row.part) {
                        // Recorded before changes named their owner: the session's workspace then and now.
                        let owner = record.owner.unwrap_or_else(|| session.workspace_id.clone());
                        let at = record.at.unwrap_or_else(|| message.info.id.clone());
                        calls.push((stamp(&at).to_string(), row.id.clone(), owner, record.changes));
                    }
                }
            }
        }
        calls.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        let mut net: Vec<Net> = Vec::new();
        for (owner, change) in calls.into_iter().flat_map(|(_, _, owner, changes)| changes.into_iter().map(move |c| (owner.clone(), c))) {
            match net.iter_mut().find(|n| n.owner == owner && n.change.path == change.path) {
                Some(existing) => {
                    existing.broken |= existing.change.after != change.before;
                    existing.change.after = change.after;
                    existing.change.observed |= change.observed;
                }
                None => net.push(Net { owner, change, broken: false }),
            }
        }
        net.retain(|n| n.broken || n.change.before != n.change.after);
        Ok(net)
    }

    fn mark(&self, session_id: &str, revert: Option<&Revert>) -> Result<Session, String> {
        let session = self.store.set_revert(session_id, revert).map_err(|e| e.to_string())?.ok_or("the session is gone")?;
        // Undone messages may hold a check's full output; the next report must not lean on it.
        self.turns.forget_checked(session_id);
        self.hub.publish(Event::SessionUpdated { session: session.clone() });
        Ok(session)
    }

    /// Where a workspace is now, bound to its history; `None` once the workspace is gone.
    fn root_of(&self, owner: &str) -> Option<PathBuf> {
        let workspace = self.store.workspace(owner).ok()??;
        let root = crate::tool::canonical(Path::new(&workspace.path));
        self.snapshots.bind(owner, &root);
        Some(root)
    }
}

fn is_prompt(message: &MessageWithParts) -> bool {
    message.info.role == Role::User && !message.parts.iter().any(|row| matches!(row.part, Part::Compaction { .. }))
}

/// A call's recorded changes, with the workspace that owns their history and when its writes finished, when the record says.
struct Record {
    owner: Option<String>,
    at: Option<String>,
    changes: Vec<FileChange>,
}

fn recorded_changes(part: &Part) -> Option<Record> {
    let Part::ToolCall { metadata: Some(metadata), .. } = part else { return None };
    let changes = serde_json::from_value(metadata.get("changes")?.clone()).ok()?;
    let text = |key: &str| metadata[key].as_str().map(str::to_string);
    Some(Record { owner: text("owner"), at: text("at"), changes })
}

/// An id's time-ordered part without its prefix, so a change stamp and a message id compare by time.
fn stamp(id: &str) -> &str {
    id.split_once('_').map_or(id, |(_, rest)| rest)
}

#[cfg(test)]
mod tests;
