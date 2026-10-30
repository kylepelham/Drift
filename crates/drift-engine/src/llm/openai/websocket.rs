//! Responses over a reusable WebSocket: one connection per conversation, falling back to SSE where unsupported.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::llm::http::Timeouts;
use crate::llm::{Chunk, ChunkStream, Error};

mod cache;
mod connection;
mod dropped;
mod handshake;
mod socket;

/// A connection with no request for this long is closed.
const IDLE: Duration = Duration::from_secs(5 * 60);
/// Reconnect before the server's 60-minute connection limit.
const LIFETIME: Duration = Duration::from_secs(55 * 60);
/// An endpoint that refused the upgrade is asked again after this long.
const RETRY_UPGRADE: Duration = Duration::from_secs(10 * 60);
/// The most conversations holding a connection at once.
const MAX_CONNECTIONS: usize = 32;
/// Chunks buffered between the connection task and the turn reading them.
const CHUNK_BUFFER: usize = 32;

/// One request, ready to send over either transport.
pub(super) struct Prepared {
    /// The authenticated SSE request; its URL and headers also open the WebSocket.
    pub handshake: reqwest::Request,
    pub body: Value,
    /// The conversation, which keys the reusable connection.
    pub session: Option<String>,
    pub timeouts: Timeouts,
    /// A ChatGPT sign-in, whose Codex backend needs a beta header on the upgrade.
    pub subscription: bool,
    /// The turn on a ChatGPT sign-in, which keeps the routing pin the upgrade's answer names.
    pub turn: Option<super::turn_state::Turn>,
}

/// Open connections, keyed by conversation and credentials.
pub(super) struct Pool {
    entries: Mutex<HashMap<String, Entry>>,
    /// Endpoints that refused the upgrade, and when to try them again.
    unsupported: Mutex<HashMap<String, Instant>>,
    /// Conversations whose WebSocket just failed, on SSE for now.
    dropped: dropped::Dropped,
    /// How long a connection waits for its next request before closing.
    idle: Duration,
}

impl Default for Pool {
    fn default() -> Self {
        Self {
            entries: Mutex::default(),
            unsupported: Mutex::default(),
            dropped: dropped::Dropped::default(),
            idle: IDLE,
        }
    }
}

struct Entry {
    slot: Slot,
    touched: Instant,
}

/// One conversation's connection; the lock keeps its requests in order.
type Slot = Arc<tokio::sync::Mutex<Option<Connection>>>;

struct Connection {
    jobs: mpsc::Sender<connection::Job>,
    /// When it is replaced, ahead of the server's own limit.
    expires: Instant,
}

impl Connection {
    fn expired(&self) -> bool {
        self.jobs.is_closed() || Instant::now() >= self.expires
    }
}

impl std::fmt::Debug for Pool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let connections = self.entries.lock().unwrap().len();
        formatter
            .debug_struct("WebSockets")
            .field("connections", &connections)
            .finish()
    }
}

impl Pool {
    pub(super) fn shared() -> Arc<Self> {
        static POOL: OnceLock<Arc<Pool>> = OnceLock::new();
        POOL.get_or_init(Arc::default).clone()
    }

    /// Streams the request over its conversation's connection; `None` means use SSE instead.
    pub(super) async fn stream(
        &self,
        client: &reqwest::Client,
        prepared: &Prepared,
    ) -> Result<Option<ChunkStream>, Error> {
        let endpoint = prepared.handshake.url().as_str();
        let key = prepared
            .session
            .as_ref()
            .map(|session| identity(&prepared.handshake, session));
        if self.unsupported(endpoint) || key.as_deref().is_some_and(|key| self.dropped.resting(key)) {
            return Ok(None);
        }

        let opened = self.sender(client, prepared, key.as_deref()).await;
        let opened = opened.inspect_err(|error| self.dropped.note(key.as_deref(), error));
        // A refused sign-in or a limit is the request's own answer; a connection that failed to open is not.
        let Some(jobs) = (match opened {
            Err(Error::Transport(_)) => return Ok(None),
            opened => opened?,
        }) else {
            self.disable(endpoint);
            return Ok(None);
        };

        // Hand the request to the connection task and stream back what it forwards.
        let (send, receive) = mpsc::channel(CHUNK_BUFFER);
        let job = connection::Job {
            body: prepared.body.clone(),
            timeouts: prepared.timeouts,
            send,
        };
        let sent = tokio::time::timeout(prepared.timeouts.headers, jobs.send(job))
            .await
            .map_err(|_| Error::Transport("waiting for the Responses WebSocket timed out".into()))?;
        if sent.is_err() {
            let error = Error::Transport("the Responses WebSocket closed before the request began".into());
            self.dropped.note(key.as_deref(), &error);
            return Ok(None);
        }

        let stream = futures_util::stream::unfold(receive, |mut receive| async move {
            receive.recv().await.map(|chunk| (chunk, receive))
        });

        Ok(begun(self.dropped.watch(key, Box::pin(stream))).await)
    }

    /// The conversation's live connection, opened if it has none; `None` when the endpoint refused it.
    async fn sender(
        &self,
        client: &reqwest::Client,
        prepared: &Prepared,
        key: Option<&str>,
    ) -> Result<Option<mpsc::Sender<connection::Job>>, Error> {
        let slot = self.slot(key);
        let mut connection = slot.lock().await;

        if connection.as_ref().is_some_and(Connection::expired) {
            *connection = None;
        }

        if connection.is_none() {
            let Some(socket) = handshake::connect(client, prepared).await? else {
                return Ok(None);
            };

            let (jobs, receive) = mpsc::channel(1);
            tokio::spawn(connection::run(socket, receive, self.idle));
            *connection = Some(Connection {
                jobs,
                expires: Instant::now() + LIFETIME,
            });
        }

        Ok(connection.as_ref().map(|connection| connection.jobs.clone()))
    }

    /// The conversation's slot; a request without a conversation gets a connection of its own.
    fn slot(&self, key: Option<&str>) -> Slot {
        let Some(key) = key else {
            return Arc::default();
        };
        let now = Instant::now();

        // Forget idle slots nobody holds.
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|_, entry| entry.touched.elapsed() < self.idle || Arc::strong_count(&entry.slot) > 1);

        // At the limit, the least recently used free slot makes room; if every slot is busy, this request goes alone.
        if !entries.contains_key(key) && entries.len() >= MAX_CONNECTIONS {
            let Some(oldest) = least_recent_free(&entries) else {
                return Arc::default();
            };
            entries.remove(&oldest);
        }

        let entry = entries.entry(key.into()).or_insert_with(|| Entry {
            slot: Arc::default(),
            touched: now,
        });
        entry.touched = now;

        entry.slot.clone()
    }

    fn unsupported(&self, endpoint: &str) -> bool {
        let mut unsupported = self.unsupported.lock().unwrap();
        unsupported.retain(|_, until| *until > Instant::now());

        unsupported.contains_key(endpoint)
    }

    fn disable(&self, endpoint: &str) {
        let mut unsupported = self.unsupported.lock().unwrap();
        if unsupported.len() >= MAX_CONNECTIONS {
            unsupported.clear();
        }

        unsupported.insert(endpoint.into(), Instant::now() + RETRY_UPGRADE);
    }
}

fn least_recent_free(entries: &HashMap<String, Entry>) -> Option<String> {
    entries
        .iter()
        .filter(|(_, entry)| Arc::strong_count(&entry.slot) == 1)
        .min_by_key(|(_, entry)| entry.touched)
        .map(|(key, _)| key.clone())
}

/// The stream once its response has begun, or `None` when the socket dropped first. A request lost that early
/// sent nothing back, so SSE can take it again without the user seeing a failure.
async fn begun(mut stream: ChunkStream) -> Option<ChunkStream> {
    let mut leading = Vec::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Err(error) if socket::dropped(&error) => return None,
            Ok(Chunk::Limits(limits)) => leading.push(Ok(Chunk::Limits(limits))),
            first => {
                leading.push(first);
                break;
            }
        }
    }

    Some(Box::pin(futures_util::stream::iter(leading).chain(stream)))
}

/// A hash of the endpoint, conversation and every header, so credentials and modes never share a connection.
fn identity(request: &reqwest::Request, session: &str) -> String {
    let mut digest = Sha256::new();
    let mut update = |bytes: &[u8]| {
        digest.update(bytes.len().to_le_bytes());
        digest.update(bytes);
    };

    update(request.url().as_str().as_bytes());
    update(session.as_bytes());

    let mut headers: Vec<_> = request.headers().iter().collect();
    headers.sort_by(|(a, a_value), (b, b_value)| {
        a.as_str()
            .cmp(b.as_str())
            .then_with(|| a_value.as_bytes().cmp(b_value.as_bytes()))
    });
    for (name, value) in headers {
        update(name.as_str().as_bytes());
        update(value.as_bytes());
    }

    crate::hex_bytes(&digest.finalize())
}

#[cfg(test)]
mod tests;
