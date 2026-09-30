//! Undo and redo: hide a conversation back to one of its prompts and put the files back as they were.
//! Nothing is deleted until the next prompt commits the undo (see `Store::admit_prompt`).

use std::future::Future;
use std::path::PathBuf;

use tokio_util::sync::CancellationToken;

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

impl Engine {
    /// Hides `message_id`, a prompt, with everything after it, and restores the files to how they were
    /// just before it. Called again while undone it moves the point either way, and the files follow.
    pub async fn revert(&self, session_id: &str, message_id: &str) -> Result<Session, RevertError> {
        self.exclusively(session_id, self.revert_claimed(session_id, message_id)).await
    }

    /// Brings back everything an undo hid, files included.
    pub async fn unrevert(&self, session_id: &str) -> Result<Session, RevertError> {
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

    async fn revert_claimed(&self, session_id: &str, message_id: &str) -> Result<Session, RevertError> {
        let session = self.store.session(session_id)?.ok_or(RevertError::NoSession)?;
        let transcript = self.store.transcript(session_id)?;
        if !transcript.iter().any(|m| m.info.id == message_id && is_prompt(m)) {
            return Err(RevertError::NotAPrompt);
        }
        let workspace = self.workspace_of(&session)?;
        // Redo returns to the tree as it was before the first undo, so only the first one records it.
        let latest = match &session.revert {
            Some(revert) => revert.snapshot.clone(),
            None => self.snapshots.take(&workspace).await.ok(),
        };
        if let Some(tree) = self.first_snapshot_from(&session, message_id)?.or_else(|| latest.clone()) {
            self.snapshots.restore(&workspace, &tree).await.map_err(|e| RevertError::Files(e.to_string()))?;
        }
        self.mark(session_id, Some(&Revert { message_id: message_id.into(), snapshot: latest }))
    }

    async fn unrevert_claimed(&self, session_id: &str) -> Result<Session, RevertError> {
        let session = self.store.session(session_id)?.ok_or(RevertError::NoSession)?;
        let Some(revert) = &session.revert else { return Ok(session) };
        if let Some(tree) = &revert.snapshot {
            self.snapshots.restore(&self.workspace_of(&session)?, tree).await.map_err(|e| RevertError::Files(e.to_string()))?;
        }
        self.mark(session_id, None)
    }

    fn mark(&self, session_id: &str, revert: Option<&Revert>) -> Result<Session, RevertError> {
        let session = self.store.set_revert(session_id, revert)?.ok_or(RevertError::NoSession)?;
        self.hub.publish(Event::SessionUpdated { session: session.clone() });
        Ok(session)
    }

    fn workspace_of(&self, session: &Session) -> Result<PathBuf, RevertError> {
        let workspace = self.store.workspace(&session.workspace_id)?.ok_or(RevertError::NoSession)?;
        Ok(crate::tool::canonical(std::path::Path::new(&workspace.path)))
    }

    /// The tree before the first write at or after `message_id`, across the session and its subagents,
    /// whose writes are snapshotted in their own sessions. `None` when nothing was written since.
    fn first_snapshot_from(&self, session: &Session, message_id: &str) -> Result<Option<String>, RevertError> {
        let mut earliest: Option<(String, String)> = None;
        for member in self.store.session_tree(&session.id)? {
            for message in self.store.transcript(&member)?.iter().filter(|m| m.info.id.as_str() >= message_id) {
                let Some(tree) = snapshot_of(message) else { continue };
                if earliest.as_ref().is_none_or(|(id, _)| message.info.id < *id) {
                    earliest = Some((message.info.id.clone(), tree));
                }
            }
        }
        Ok(earliest.map(|(_, tree)| tree))
    }
}

fn is_prompt(message: &MessageWithParts) -> bool {
    message.info.role == Role::User && !message.parts.iter().any(|row| matches!(row.part, Part::Compaction { .. }))
}

/// Every write in a step shares the snapshot taken before the first one.
fn snapshot_of(message: &MessageWithParts) -> Option<String> {
    message.parts.iter().find_map(|row| match &row.part {
        Part::ToolCall { metadata: Some(metadata), .. } => metadata["snapshot"].as_str().map(str::to_string),
        _ => None,
    })
}

#[cfg(test)]
mod tests;
