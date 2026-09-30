//! Fork and move: whole-session operations that never touch a turn in flight.

use super::types::{MessageStatus, Role, Session, Visibility};
use crate::event::Event;
use crate::store::NewSession;
use crate::Engine;

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
        let transcript = self.store.transcript(source_id)?;
        // The in-flight turn starts at the last user message; nothing from there on is stable.
        let stable = if self.turns.is_running(source_id) {
            transcript.iter().rposition(|m| m.info.role == Role::User).unwrap_or(0)
        } else {
            transcript.len()
        };
        let finished = &transcript[..stable];
        let through = match at {
            Some(id) => finished.iter().find(|m| m.info.id == id && m.info.status != MessageStatus::Streaming).ok_or(TreeError::BadMessage)?,
            None => finished.iter().rev().find(|m| m.info.status != MessageStatus::Streaming).ok_or(TreeError::Empty)?,
        };
        let title = if source.title.is_empty() { "Fork".to_string() } else { format!("{} (fork)", source.title) };
        let session = self.store.fork_session(
            source_id,
            NewSession { workspace_id: &source.workspace_id, parent_id: None, visibility: Visibility::Sibling, title: &title, agent: &source.agent, model: source.model.as_ref() },
            &through.info.id,
        )?;
        self.hub.publish(Event::SessionCreated { session: session.clone() });
        Ok(session)
    }

    /// Moves a session and its subagents to another workspace. Refused while any of them is running or
    /// still planning a turn, because a turn keeps the workspace path it planned with. The check and the
    /// update hold claims off together, so no turn can be admitted in between.
    pub fn move_session(&self, id: &str, workspace_id: &str) -> Result<Vec<String>, TreeError> {
        self.store.session(id)?.ok_or(TreeError::NoSession)?;
        self.store.workspace(workspace_id)?.ok_or(TreeError::NoWorkspace)?;
        let tree = self.store.session_tree(id)?;
        self.turns.while_idle(&tree, || self.store.move_sessions(&tree, workspace_id)).ok_or(TreeError::Busy)??;
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
