//! `/spawn <instruction>`: a new thread, linked to its source, that starts at once with a copy of the
//! source's finished conversation and the instruction; the model works out what of it matters.

use std::sync::Arc;

use super::turn::{Prompt, TurnError};
use super::types::{Part, Session, Visibility};
use crate::event::Event;
use crate::store::NewSession;
use crate::Engine;

const TITLE_WORDS: usize = 6;

#[derive(Debug)]
pub enum BranchError {
    NoSession,
    /// Subagents are workers on their parent's goal; only a conversation can spawn a thread.
    FromSubagent,
    EmptyInstruction,
    Turn(TurnError),
    Store(rusqlite::Error),
}

impl From<rusqlite::Error> for BranchError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl Engine {
    /// Spawns a thread from `source_id` and starts it on `instruction`. It runs on its own from then on.
    pub async fn spawn(self: &Arc<Self>, source_id: &str, instruction: &str) -> Result<Session, BranchError> {
        let instruction = instruction.trim();
        if instruction.is_empty() {
            return Err(BranchError::EmptyInstruction);
        }
        let source = self.store.session(source_id)?.ok_or(BranchError::NoSession)?;
        if source.visibility == Visibility::Hidden {
            return Err(BranchError::FromSubagent);
        }
        let title = instruction.split_whitespace().take(TITLE_WORDS).collect::<Vec<_>>().join(" ");
        let new = NewSession { workspace_id: &source.workspace_id, parent_id: Some(&source.id), visibility: Visibility::Sibling, title: &title, agent: &source.agent, model: source.model.as_ref() };
        let session = match self.finished(&source)?.pop() {
            Some(through) => self.store.fork_session(&source.id, new, &through, Some(&through))?,
            None => self.store.create_branch(new, None)?,
        };
        self.hub.publish(Event::SessionCreated { session: session.clone() });
        let text = format!("This is a new thread spawned from the conversation above. Work only on this, using whatever of that conversation it needs:\n\n{instruction}");
        let prompt = Prompt { parts: vec![Part::Text { text }], model: source.model.clone(), variant: Some(source.variant.clone()), agent: None, submission_id: None };
        self.submit(&session.id, prompt).await.map_err(BranchError::Turn)?;
        Ok(session)
    }
}

#[cfg(test)]
mod tests;
