//! Shared HTTP clients; ordinary requests can use HTTP/2, while WebSocket upgrades need HTTP/1.1.

use futures_util::StreamExt;
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
        Self {
            headers: Duration::from_secs(120),
            idle: Duration::from_secs(300),
        }
    }
}

/// Routes to models on this machine, which may spend minutes loading or reading a long prompt on a CPU.
const LOCAL_ROUTES: [&str; 2] = ["lmstudio", "ollama"];

impl Timeouts {
    /// The limits a route starts with; drift.json `timeouts` can change any of them.
    pub fn for_route(provider: &str) -> Self {
        if LOCAL_ROUTES.contains(&provider) {
            return Self {
                headers: Duration::from_secs(600),
                idle: Duration::from_secs(600),
            };
        }

        Self::default()
    }
}

const CONNECT: Duration = Duration::from_secs(15);

/// Sent with every request that does not name its own; GitHub's API refuses a request without one.
pub const USER_AGENT: &str = concat!("Drift/", env!("CARGO_PKG_VERSION"));

/// The shared client. Cloning it shares its connection pool.
pub fn client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| builder().build().unwrap_or_default()).clone()
}

/// For WebSocket upgrades: the Codex backend refuses one negotiated over HTTP/2 (405).
pub(super) fn websocket_client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| builder().http1_only().build().unwrap_or_default())
        .clone()
}

/// What every client shares: Drift's user agent, a bounded connect and TCP keepalive.
fn builder() -> reqwest::ClientBuilder {
    // The shell installs TLS in release builds; test processes install it before their first client.
    #[cfg(test)]
    let _ = rustls::crypto::ring::default_provider().install_default();

    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT)
        .tcp_keepalive(Duration::from_secs(30))
}

/// A non-streamed body (an error, a token exchange) is read no further than this: enough for any
/// provider's JSON.
const MAX_ERROR_BODY: usize = 64 * 1024;
/// And for no longer than this, however slowly it trickles in.
const MAX_ERROR_WAIT: Duration = Duration::from_secs(10);

/// A response body cut at a size and a time limit, so a stalled or endless one cannot hold the turn:
/// whatever arrived in time is what it says.
pub async fn bounded_body(response: reqwest::Response, timeouts: &Timeouts) -> String {
    let deadline = tokio::time::Instant::now() + timeouts.idle.min(MAX_ERROR_WAIT);
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();

    while body.len() < MAX_ERROR_BODY {
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(Some(Ok(chunk))) => {
                let remaining = MAX_ERROR_BODY - body.len();
                body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
            _ => break,
        }
    }

    String::from_utf8_lossy(&body).into_owned()
}

/// Sends `request`, failing as a transport error if the response has not begun within `timeouts.headers`.
pub async fn send(request: reqwest::RequestBuilder, timeouts: &Timeouts) -> Result<reqwest::Response, Error> {
    match tokio::time::timeout(timeouts.headers, request.send()).await {
        Ok(response) => Ok(response?),
        Err(_) => Err(Error::Transport(format!(
            "no response within {} s",
            timeouts.headers.as_secs()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn every_request_names_drift_as_its_user_agent_as_githubs_api_requires() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let read = socket.read(&mut request).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                .await
                .unwrap();
            String::from_utf8_lossy(&request[..read]).to_lowercase()
        });

        super::client().get(&url).send().await.unwrap();

        let request = server.await.unwrap();
        assert!(
            request.contains(&format!("user-agent: {}", super::USER_AGENT.to_lowercase())),
            "{request}"
        );
    }
}
