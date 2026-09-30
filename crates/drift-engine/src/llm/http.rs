//! One HTTP client for every provider and service: shared connections, a bounded connect, a bounded
//! wait for the response to begin, and a stream that goes quiet too long counts as broken.

use std::sync::OnceLock;
use std::time::Duration;

use super::Error;

/// How long each stage of a provider request may take.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timeouts {
    /// Until the response status and headers arrive; long prompts are still answered well within it.
    pub headers: Duration,
    /// Between two pieces of a stream. Reasoning models can think quietly for minutes; a live
    /// connection still sends pings or comments.
    pub idle: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self { headers: Duration::from_secs(120), idle: Duration::from_secs(300) }
    }
}

const CONNECT: Duration = Duration::from_secs(15);

/// The shared client. Cloning it shares its connection pool.
pub fn client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| reqwest::Client::builder().connect_timeout(CONNECT).tcp_keepalive(Duration::from_secs(30)).build().unwrap_or_default())
        .clone()
}

/// A non-streamed body (an error, a token exchange) is read no further than this: enough for any
/// provider's JSON.
const MAX_ERROR_BODY: usize = 64 * 1024;
/// And for no longer than this, however slowly it trickles in.
const MAX_ERROR_WAIT: Duration = Duration::from_secs(10);

/// A response body cut at a size and a time limit, so a stalled or endless one cannot hold the turn:
/// whatever arrived in time is what it says.
pub async fn bounded_body(response: reqwest::Response, timeouts: &Timeouts) -> String {
    use futures_util::StreamExt;
    let deadline = tokio::time::Instant::now() + timeouts.idle.min(MAX_ERROR_WAIT);
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while body.len() < MAX_ERROR_BODY {
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(Some(Ok(chunk))) => body.extend_from_slice(&chunk[..chunk.len().min(MAX_ERROR_BODY - body.len())]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&body).into_owned()
}

/// Sends `request`, failing as a transport error if the response has not begun within `timeouts.headers`.
pub async fn send(request: reqwest::RequestBuilder, timeouts: &Timeouts) -> Result<reqwest::Response, Error> {
    match tokio::time::timeout(timeouts.headers, request.send()).await {
        Ok(response) => Ok(response?),
        Err(_) => Err(Error::Transport(format!("no response within {} s", timeouts.headers.as_secs()))),
    }
}
