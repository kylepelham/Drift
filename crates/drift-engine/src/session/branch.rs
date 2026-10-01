//! `/spawn <instruction>`: a new thread, linked to its source, that starts at once with a copy of the
//! source's finished conversation and the instruction; the model works out what of it matters.

use std::sync::Arc;

use super::turn::{Prompt, TurnError};
use super::types::{Message, MessageWithParts, Part, PartRow, Role, Session, Visibility};
use crate::event::Event;
use crate::store::NewSession;
use crate::Engine;

const TITLE_WORDS: usize = 6;
const FRAMING: &str = "This is a new thread spawned from the conversation above. Work only on what follows, using whatever of that conversation it needs.";

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
        let prompt = Prompt { parts: vec![Part::Text { text: instruction.into() }], model: source.model.clone(), variant: Some(source.variant.clone()), agent: None, submission_id: None };
        self.submit(&session.id, prompt).await.map_err(BranchError::Turn)?;
        Ok(session)
    }
}

/// Copied messages keep their original times, so in a spawned thread they are the ones older than the thread.
pub fn is_copied(session: &Session, message: &Message) -> bool {
    session.branch_cutoff.is_some() && message.created_at < session.created_at
}

/// Tells the model, in the request only, that the thread's first own prompt follows a copied conversation.
pub(super) fn frame_spawned(session: &Session, transcript: &mut [MessageWithParts]) {
    if session.branch_cutoff.is_none() {
        return;
    }
    let Some(first) = transcript.iter_mut().find(|m| m.info.role == Role::User && !is_copied(session, &m.info)) else { return };
    let framing = PartRow { id: String::new(), message_id: first.info.id.clone(), session_id: session.id.clone(), part: Part::Text { text: FRAMING.into() } };
    first.parts.insert(0, framing);
}

#[cfg(test)]
mod tests;
