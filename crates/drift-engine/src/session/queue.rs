//! A user's prompt for another agent or level waits here, durably, for the running turn's step to end, then runs as its own turn.

use std::path::Path;
use std::sync::Arc;

use super::turn::{pickable, Admission, Choice, Prompt, Receipt, TurnError};
use crate::event::Event;
use crate::id;
use crate::session::types::{Part, Session};
use crate::store::QueuedRow;
use crate::Engine;

impl Engine {
    /// A user's prompt while others wait joins them (same agent and level) or replaces them; `None` when nothing waits.
    pub(super) fn join_queue(self: &Arc<Self>, session_id: &str, prompt: &Prompt, payload_hash: &str) -> Result<Option<Receipt>, TurnError> {
        if let Some(id) = prompt.submission_id.as_deref() {
            if let Some((session, hash)) = self.store.queued_submission(id)? {
                if session != session_id || hash != payload_hash {
                    return Err(TurnError::SubmissionReused);
                }
                return self.queued_receipt(session_id, Vec::new()).map(Some);
            }
        }
        if self.store.queued(session_id)?.is_empty() {
            return Ok(None);
        }
        self.queue(session_id, prompt, payload_hash, false)
    }

    /// Queues `prompt` for a turn of its own; the running turn hands over at its next step.
    pub(super) fn queue_behind(self: &Arc<Self>, session_id: &str, prompt: &Prompt, payload_hash: &str) -> Result<Receipt, TurnError> {
        self.queue(session_id, prompt, payload_hash, true)?.ok_or(TurnError::Busy)
    }

    fn queue(self: &Arc<Self>, session_id: &str, prompt: &Prompt, payload_hash: &str, start_new: bool) -> Result<Option<Receipt>, TurnError> {
        let session = self.store.session(session_id)?.ok_or(TurnError::NoSession)?;
        if let Some(agent) = &prompt.agent {
            self.check_agent(&session, agent)?;
        }
        let mut stored = prompt.clone();
        let submission_id = stored.submission_id.get_or_insert_with(|| id::new("sub")).clone();
        let row = QueuedRow { submission_id, payload_hash: payload_hash.into(), prompt_json: serde_json::to_string(&stored).map_err(|e| TurnError::Store(e.to_string()))?, error: None, created_at: 0 };
        let queueing = self.turns.queueing.lock().unwrap();
        let waiting = self.store.queued(session_id)?;
        let Some(first) = waiting.first() else {
            if !start_new {
                return Ok(None);
            }
            self.store.queue(session_id, &row, false)?;
            drop(queueing);
            return self.queued(session_id, Vec::new()).map(Some);
        };
        let replace = prompts(std::slice::from_ref(first)).first().is_none_or(|first| self.waiting_choice(&session, first).differs(prompt));
        let replaced = self.store.queue(session_id, &row, replace)?;
        drop(queueing);
        self.queued(session_id, prompts(&replaced).into_iter().flat_map(|p| p.parts).collect()).map(Some)
    }

    /// Announces what waits now and makes sure it starts if nothing is running to hand over.
    fn queued(self: &Arc<Self>, session_id: &str, returned: Vec<Part>) -> Result<Receipt, TurnError> {
        let receipt = self.queued_receipt(session_id, returned)?;
        self.hub.publish(Event::SessionUpdated { session: receipt.session.clone() });
        if !self.turns.is_running(session_id) {
            self.start_queued_soon(session_id);
        }
        Ok(receipt)
    }

    fn queued_receipt(&self, session_id: &str, returned: Vec<Part>) -> Result<Receipt, TurnError> {
        let session = self.store.session(session_id)?.ok_or(TurnError::NoSession)?;
        Ok(Receipt { session, message: None, returned })
    }

    fn check_agent(&self, session: &Session, agent: &str) -> Result<(), TurnError> {
        let workspace = self.store.workspace(&session.workspace_id)?.ok_or(TurnError::NoWorkspace)?;
        pickable(&self.workspace_config(&crate::tool::canonical(Path::new(&workspace.path))), agent)
    }

    /// What the oldest waiting prompt runs as; later prompts are judged against it.
    fn waiting_choice(&self, session: &Session, first: &Prompt) -> Choice {
        let agent = first.agent.clone().unwrap_or_else(|| session.agent.clone());
        let variant = first.variant.clone().unwrap_or_else(|| session.variant.clone());
        let model = first.model.as_ref().or(session.model.as_ref());
        let catalog = self.catalog.read().unwrap();
        let variants = model.and_then(|m| catalog.providers.get(&m.provider)?.models.get(&m.model)).map(|m| m.variants.clone()).unwrap_or_default();
        Choice::new(model.cloned(), agent, variant.as_deref(), variants)
    }

    /// Starts what waits in the background; see [`Self::start_queued`].
    pub(super) fn start_queued_soon(self: &Arc<Self>, session_id: &str) {
        let Some(runtime) = tokio::runtime::Handle::try_current().ok().or_else(|| self.runtime.get().cloned()) else { return };
        let engine = self.clone();
        let id = session_id.to_string();
        runtime.spawn(async move { engine.start_queued(&id).await });
    }

    /// Starts everything waiting as one prompt and turn; a taken session's job starts it when it ends, and a failure stays with its reason.
    async fn start_queued(self: &Arc<Self>, session_id: &str) {
        let rows = self.store.queued(session_id).unwrap_or_default();
        if rows.is_empty() || rows.iter().any(|row| row.error.is_some()) {
            return;
        }
        let submissions: Vec<(&str, &str)> = rows.iter().map(|row| (row.submission_id.as_str(), row.payload_hash.as_str())).collect();
        let Some(prompt) = merged(&rows) else { return self.fail_queue(session_id, &submissions, "a waiting prompt could not be read") };
        match self.admit(session_id, prompt, Admission { queued: &submissions, ..Admission::default() }).await {
            Ok(_) | Err(TurnError::Busy | TurnError::Stopped) => {}
            Err(error) => self.fail_queue(session_id, &submissions, &error.to_string()),
        }
    }

    /// Marks the attempted prompts as unable to start, only if they are exactly what still waits; otherwise what replaced them starts.
    fn fail_queue(self: &Arc<Self>, session_id: &str, attempted: &[(&str, &str)], error: &str) {
        match self.store.fail_queued(session_id, attempted, error) {
            Ok(true) => {
                if let Ok(Some(session)) = self.store.session(session_id) {
                    self.hub.publish(Event::SessionUpdated { session });
                }
            }
            Ok(false) => self.start_queued_soon(session_id),
            Err(_) => {}
        }
    }

    /// Removes what waits in the session and returns its parts, for the user to have back; nothing of it ran.
    pub fn discard_queued(&self, session_id: &str) -> Vec<Part> {
        let queueing = self.turns.queueing.lock().unwrap();
        let rows = self.store.take_queued(session_id).unwrap_or_default();
        drop(queueing);
        if rows.is_empty() {
            return Vec::new();
        }
        if let Ok(Some(session)) = self.store.session(session_id) {
            self.hub.publish(Event::SessionUpdated { session });
        }
        prompts(&rows).into_iter().flat_map(|prompt| prompt.parts).collect()
    }

    /// Starts what was left waiting when the engine last stopped.
    pub fn resume_queued(self: &Arc<Self>) {
        for session_id in self.store.waiting_sessions().unwrap_or_default() {
            self.start_queued_soon(&session_id);
        }
    }
}

fn prompts(rows: &[QueuedRow]) -> Vec<Prompt> {
    rows.iter().filter_map(|row| serde_json::from_str(&row.prompt_json).ok()).collect()
}

/// The waiting prompts as one: the oldest's model, agent and level (later ones joined only by matching them), everyone's parts in order.
fn merged(rows: &[QueuedRow]) -> Option<Prompt> {
    let mut all = prompts(rows).into_iter();
    let mut first = all.next()?;
    for later in all {
        first.parts.extend(later.parts);
    }
    Some(first)
}

#[cfg(test)]
mod tests;
