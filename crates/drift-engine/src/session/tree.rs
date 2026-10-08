//! Fork and move: whole-session operations that never touch a turn in flight.

use super::types::{MessageStatus, Role, Session, Visibility};
use crate::Engine;
use crate::event::Event;
use crate::store::NewSession;

#[derive(Debug)]
pub enum TreeError {
    NoSession,
    NoWorkspace,
    /// A turn is running in the session or one of its subagents.
    Busy,
    /// The fork point is not a finished message of this session.
    BadMessage,
    /// Nothing finished to copy yet.
    Empty,
    /// Part of the history went while it was being copied, as a committed undo does; nothing was made.
    Changed,
    Store(rusqlite::Error),
}

impl From<rusqlite::Error> for TreeError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl Engine {
    /// A new top-level conversation with a copy of the source's finished history through `at`, or
    /// through its last stable message. A turn still running is left out.
    pub fn fork(&self, source_id: &str, at: Option<&str>) -> Result<Session, TreeError> {
        let source = self.store.session(source_id)?.ok_or(TreeError::NoSession)?;
        let through = match at {
            Some(id) => self
                .finished(&source)?
                .into_iter()
                .find(|finished| finished == id)
                .ok_or(TreeError::BadMessage)?,
            None => self.finished(&source)?.pop().ok_or(TreeError::Empty)?,
        };
        let title = if source.title.is_empty() {
            "Fork".to_string()
        } else {
            format!("{} (fork)", source.title)
        };
        let session = self
            .store
            .fork_session(
                source_id,
                NewSession {
                    workspace_id: &source.workspace_id,
                    parent_id: None,
                    visibility: Visibility::Sibling,
                    title: &title,
                    agent: &source.agent,
                    model: source.model.as_ref(),
                },
                &through,
                None,
            )?
            .ok_or(TreeError::Changed)?;
        self.hub.publish(Event::SessionCreated {
            session: session.clone(),
        });
        Ok(session)
    }

    /// Finished visible message IDs before the running turn, read without loading their parts.
    pub(super) fn finished(&self, source: &Session) -> rusqlite::Result<Vec<String>> {
        let messages = self.store.message_infos(&source.id)?;
        // A running turn is unstable from the prompt it began at, however many are steered in after it.
        let stable = match self.turns.began(&source.id) {
            Some(began) => messages.iter().position(|m| m.id >= began).unwrap_or(messages.len()),
            // Another job, such as a compaction, is unstable from the last prompt.
            None if self.turns.is_running(&source.id) => {
                messages.iter().rposition(|m| m.role == Role::User).unwrap_or(0)
            }
            None => messages.len(),
        };
        let visible = source
            .revert
            .as_ref()
            .and_then(|r| messages.iter().position(|m| m.id >= r.message_id))
            .unwrap_or(messages.len());
        Ok(messages[..stable.min(visible)]
            .iter()
            .filter(|m| m.status != MessageStatus::Streaming)
            .map(|m| m.id.clone())
            .collect())
    }

    /// Moves a session and its subagents to another workspace. Refused while any of them is running or
    /// still planning a turn, because a turn keeps the workspace path it planned with. The check and the
    /// update hold claims off together, so no turn can be admitted in between.
    pub fn move_session(&self, id: &str, workspace_id: &str) -> Result<Vec<String>, TreeError> {
        self.store.session(id)?.ok_or(TreeError::NoSession)?;
        self.store.workspace(workspace_id)?.ok_or(TreeError::NoWorkspace)?;
        let tree = self.store.session_tree(id)?;
        self.turns
            .while_idle(&tree, || self.store.move_sessions(&tree, workspace_id))
            .ok_or(TreeError::Busy)??;
        for member in &tree {
            if let Some(session) = self.store.session(member)? {
                self.hub.publish(Event::SessionUpdated { session });
            }
        }
        Ok(tree)
    }
}

#[cfg(test)]
mod tests;
