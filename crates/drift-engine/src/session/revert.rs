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

#[derive(Debug)]
pub enum RevertError {
    NoSession,
    /// Only a prompt the user sent can be the point to go back to.
    NotAPrompt,
    /// A turn or job holds the session; its files and history are in motion.
    Busy,
    Files(String),
    Store(rusqlite::Error),
}

impl From<rusqlite::Error> for RevertError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

/// The session after an undo or redo, and the files left alone because they changed since.
#[derive(Debug, Serialize, ToSchema)]
pub struct Undone {
    pub session: Session,
    pub kept: Vec<String>,
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

    /// Runs `work` holding the session, so no turn starts while its files and history change underneath.
    async fn exclusively<T>(&self, session_id: &str, work: impl Future<Output = Result<T, RevertError>>) -> Result<T, RevertError> {
        if !self.turns.claim(session_id, &CancellationToken::new()) {
            return Err(RevertError::Busy);
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
        let workspace = self.workspace_of(&session)?;
        let kept = match session.revert.as_ref().map(|r| r.message_id.as_str()) {
            None => self.shift(&session, &workspace, message_id, None, Direction::Back).await?,
            Some(current) if message_id < current => self.shift(&session, &workspace, message_id, Some(current), Direction::Back).await?,
            Some(current) if message_id > current => self.shift(&session, &workspace, current, Some(message_id), Direction::Forward).await?,
            Some(_) => Vec::new(),
        };
        let session = self.mark(session_id, Some(&Revert { message_id: message_id.into(), kept: kept.clone() }))?;
        Ok(Undone { session, kept })
    }

    async fn unrevert_claimed(&self, session_id: &str) -> Result<Undone, RevertError> {
        let session = self.store.session(session_id)?.ok_or(RevertError::NoSession)?;
        let Some(revert) = &session.revert else { return Ok(Undone { session, kept: Vec::new() }) };
        let workspace = self.workspace_of(&session)?;
        let kept = self.shift(&session, &workspace, &revert.message_id, None, Direction::Forward).await?;
        Ok(Undone { session: self.mark(session_id, None)?, kept })
    }

    /// Applies the net change of the turns in `[from, to)` in one direction. A file whose content is
    /// not what that change expects was edited by someone else since; it is kept, not overwritten.
    async fn shift(&self, session: &Session, workspace: &Path, from: &str, to: Option<&str>, direction: Direction) -> Result<Vec<String>, RevertError> {
        let mut kept = Vec::new();
        for change in self.net_changes(session, from, to)? {
            let (expected, target) = match direction {
                Direction::Back => (change.after, change.before),
                Direction::Forward => (change.before, change.after),
            };
            let current = self.snapshots.current(workspace, &change.path).await.map_err(|e| RevertError::Files(e.to_string()))?;
            if current != expected {
                kept.push(change.path);
                continue;
            }
            self.snapshots.put(workspace, &change.path, target.as_deref()).await.map_err(|e| RevertError::Files(e.to_string()))?;
        }
        Ok(kept)
    }

    /// Per path, the state before its first change and after its last one in `[from, to)`, across the
    /// session and its subagents. Ids are time-ordered, so sorting by them orders calls across sessions.
    fn net_changes(&self, session: &Session, from: &str, to: Option<&str>) -> Result<Vec<FileChange>, RevertError> {
        let mut calls: Vec<(String, String, Vec<FileChange>)> = Vec::new();
        for member in self.store.session_tree(&session.id)? {
            for message in self.store.transcript(&member)?.iter().filter(|m| m.info.id.as_str() >= from && to.is_none_or(|to| m.info.id.as_str() < to)) {
                for row in &message.parts {
                    if let Some(changes) = recorded_changes(&row.part) {
                        calls.push((message.info.id.clone(), row.id.clone(), changes));
                    }
                }
            }
        }
        calls.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        let mut net: Vec<FileChange> = Vec::new();
        for change in calls.into_iter().flat_map(|(_, _, changes)| changes) {
            match net.iter_mut().find(|n| n.path == change.path) {
                Some(existing) => existing.after = change.after,
                None => net.push(change),
            }
        }
        net.retain(|change| change.before != change.after);
        Ok(net)
    }

    fn mark(&self, session_id: &str, revert: Option<&Revert>) -> Result<Session, RevertError> {
        let session = self.store.set_revert(session_id, revert)?.ok_or(RevertError::NoSession)?;
        self.hub.publish(Event::SessionUpdated { session: session.clone() });
        Ok(session)
    }

    fn workspace_of(&self, session: &Session) -> Result<PathBuf, RevertError> {
        let workspace = self.store.workspace(&session.workspace_id)?.ok_or(RevertError::NoSession)?;
        Ok(crate::tool::canonical(Path::new(&workspace.path)))
    }
}

fn is_prompt(message: &MessageWithParts) -> bool {
    message.info.role == Role::User && !message.parts.iter().any(|row| matches!(row.part, Part::Compaction { .. }))
}

fn recorded_changes(part: &Part) -> Option<Vec<FileChange>> {
    let Part::ToolCall { metadata: Some(metadata), .. } = part else { return None };
    serde_json::from_value(metadata.get("changes")?.clone()).ok()
}

#[cfg(test)]
mod tests;
