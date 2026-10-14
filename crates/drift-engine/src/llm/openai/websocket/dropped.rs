//! Conversations whose WebSocket just failed: their next requests go over SSE for a while, so a retry does not
//! wait on a socket opened straight after the one that dropped.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::Instant;

use super::MAX_CONNECTIONS;
use crate::llm::{ChunkStream, Error};

/// How long a conversation stays on SSE after its WebSocket failed.
const REST: Duration = Duration::from_secs(2 * 60);

#[derive(Clone, Default)]
pub(super) struct Dropped(Arc<Mutex<HashMap<String, Instant>>>);

impl Dropped {
    /// True while the conversation `key` should use SSE.
    pub(super) fn resting(&self, key: &str) -> bool {
        let mut until = self.0.lock().unwrap();
        until.retain(|_, until| *until > Instant::now());

        until.contains_key(key)
    }

    /// Notes a failure of the conversation's WebSocket; an API error is the request's own, not the socket's.
    pub(super) fn note(&self, key: Option<&str>, error: &Error) {
        let (Some(key), Error::Transport(_)) = (key, error) else {
            return;
        };

        let mut until = self.0.lock().unwrap();
        if until.len() >= MAX_CONNECTIONS {
            until.clear();
        }
        until.insert(key.into(), Instant::now() + REST);
    }

    /// Passes the stream through, noting a failure that arrives in it.
    pub(super) fn watch(&self, key: Option<String>, stream: ChunkStream) -> ChunkStream {
        let dropped = self.clone();

        Box::pin(futures_util::StreamExt::inspect(stream, move |chunk| {
            if let Err(error) = chunk {
                dropped.note(key.as_deref(), error);
            }
        }))
    }
}
