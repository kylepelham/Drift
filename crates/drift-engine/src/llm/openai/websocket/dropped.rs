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
/// The longest rest, reached when each new socket fails soon after the last rest ended.
const LONGEST: Duration = Duration::from_secs(60 * 60);
/// A failure this soon after a rest ended doubles the next one.
const SOON: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Copy)]
struct Rest {
    until: Instant,
    /// Failures in a row, each soon after the rest before it.
    streak: u32,
}

#[derive(Clone, Default)]
pub(super) struct Dropped(Arc<Mutex<HashMap<String, Rest>>>);

impl Dropped {
    /// True while the conversation `key` should use SSE.
    pub(super) fn resting(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut rests = self.0.lock().unwrap();
        rests.retain(|_, rest| rest.until + SOON > now);

        rests.get(key).is_some_and(|rest| rest.until > now)
    }

    /// Notes a failure of the conversation's WebSocket; an API error is the request's own, not the socket's.
    pub(super) fn note(&self, key: Option<&str>, error: &Error) {
        let (Some(key), Error::Transport(_)) = (key, error) else {
            return;
        };

        let now = Instant::now();
        let mut rests = self.0.lock().unwrap();
        if rests.len() >= MAX_CONNECTIONS && !rests.contains_key(key) {
            rests.clear();
        }
        let streak = rests
            .get(key)
            .filter(|rest| now < rest.until + SOON)
            .map_or(0, |rest| rest.streak.saturating_add(1).min(5));
        let until = now + REST.saturating_mul(1 << streak).min(LONGEST);
        rests.insert(key.into(), Rest { until, streak });
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
