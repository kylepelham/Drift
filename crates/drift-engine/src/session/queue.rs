//! Prompt delivery `queue`: a prompt that waits for the running turn to end and then starts a turn of its own,
//! unlike the default `steer`, which joins the running turn. Stop holds the queue until the user sends a prompt.

use std::sync::Arc;

use crate::Engine;
use crate::event::Event;
use crate::session::turn::{Admission, Prompt, Receipt, TurnError};
use crate::store::QueuedPrompt;

impl Engine {
    /// Queues a prompt behind the running turn; with none running it starts at once.
    pub fn queue_prompt(self: &Arc<Self>, session_id: &str, prompt: Prompt) -> Result<QueuedPrompt, TurnError> {
        if self.store.session(session_id)?.is_none() {
            return Err(TurnError::NoSession);
        }
        let id = crate::id::new("que");
        self.store.queue_prompt(&id, session_id, &prompt)?;
        let queued = self.store.queued_prompt(&id)?.ok_or(TurnError::NoSession)?;
        self.publish_queue(session_id);
        self.drain_queue(session_id);

        Ok(queued)
    }

    pub fn queued_prompts(&self, session_id: &str) -> Result<Vec<QueuedPrompt>, TurnError> {
        Ok(self.store.queued_prompts(session_id)?)
    }

    /// Replaces a queued prompt; false when it already started or was discarded.
    pub fn edit_queued(self: &Arc<Self>, id: &str, prompt: &Prompt) -> Result<bool, TurnError> {
        let Some(queued) = self.store.queued_prompt(id)? else {
            return Ok(false);
        };
        let edited = self.store.edit_queued_prompt(id, prompt)?;
        self.publish_queue(&queued.session_id);
        self.drain_queue(&queued.session_id);

        Ok(edited)
    }

    pub fn discard_queued(&self, id: &str) -> Result<bool, TurnError> {
        let Some(queued) = self.store.queued_prompt(id)? else {
            return Ok(false);
        };
        let removed = self.store.unqueue_prompt(id)?;
        self.publish_queue(&queued.session_id);

        Ok(removed)
    }

    /// Sends a queued prompt now: into the running turn as a steer, or as a new turn when none runs.
    /// One that cannot be sent keeps its place with the reason, as a drained one does.
    pub async fn send_queued_now(self: &Arc<Self>, id: &str) -> Result<Option<Receipt>, TurnError> {
        let Some(queued) = self.store.queued_prompt(id)? else {
            return Ok(None);
        };
        let mut prompt = queued.prompt;
        prompt.submission_id = Some(format!("queue:{id}"));

        let sent = self.submit(&queued.session_id, prompt).await;
        match &sent {
            Ok(_) => {
                self.store.unqueue_prompt(id)?;
            }
            Err(error) => self.store.fail_queued_prompt(id, &error.to_string())?,
        }
        self.publish_queue(&queued.session_id);

        sent.map(Some)
    }

    /// Starts the first queued prompt's turn when the session is idle and its queue not held by Stop.
    /// A prompt that cannot start keeps its place with the reason, and holds the ones behind it.
    pub(crate) fn drain_queue(self: &Arc<Self>, session_id: &str) {
        if self.turns.queue_held(session_id) || self.turns.is_running(session_id) {
            return;
        }
        let first = self
            .store
            .queued_prompts(session_id)
            .ok()
            .and_then(|queue| queue.into_iter().next());
        let Some(first) = first.filter(|first| first.error.is_none()) else {
            return;
        };

        let engine = self.clone();
        tokio::spawn(async move {
            let mut prompt = first.prompt.clone();
            prompt.submission_id = Some(format!("queue:{}", first.id));
            let how = Admission {
                new_turn_only: true,
                ..Admission::default()
            };
            match engine.admit(&first.session_id, prompt, how).await {
                Ok(_) => {
                    let _ = engine.store.unqueue_prompt(&first.id);
                }
                Err(TurnError::Busy) => return,
                Err(error) => {
                    let _ = engine.store.fail_queued_prompt(&first.id, &error.to_string());
                }
            }
            engine.publish_queue(&first.session_id);
        });
    }

    fn publish_queue(&self, session_id: &str) {
        let queued = self.store.queued_prompts(session_id).unwrap_or_default();
        self.hub.publish(Event::QueueUpdated {
            session_id: session_id.into(),
            queued,
        });
    }
}
