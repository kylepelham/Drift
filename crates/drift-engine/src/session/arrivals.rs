//! A call that waits notices a message arriving in its conversation (the user's steer, a subagent's result, a
//! process's end), so the model reads it now rather than when the wait runs out.

use tokio::sync::broadcast;

use super::types::Role;
use crate::Engine;
use crate::event::{Envelope, Event};

pub(crate) struct Arrivals<'a> {
    engine: &'a Engine,
    session_id: String,
    seen: Option<String>,
    events: broadcast::Receiver<Envelope>,
}

impl Engine {
    /// Watches for messages that arrive in `session_id` from now on.
    pub(crate) fn arrivals(&self, session_id: &str) -> Arrivals<'_> {
        // Subscribed before the newest message is read, so one landing in between is still caught.
        let events = self.hub.attach(None).rx;
        let seen = self.store.newest_prompt(session_id).ok().flatten();

        Arrivals {
            engine: self,
            session_id: session_id.into(),
            seen,
            events,
        }
    }
}

impl Arrivals<'_> {
    /// Returns once a message newer than when the watch began arrives in the conversation.
    pub(crate) async fn next(&mut self) {
        loop {
            match self.events.recv().await {
                Ok(envelope) => {
                    if let Event::MessageCreated { message } = envelope.event
                        && message.session_id == self.session_id
                        && message.role == Role::User
                    {
                        return;
                    }
                }
                // Events were missed: the store says whether one of them was a new message.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    if self.engine.store.newest_prompt(&self.session_id).ok().flatten() != self.seen {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => std::future::pending().await,
            }
        }
    }
}
