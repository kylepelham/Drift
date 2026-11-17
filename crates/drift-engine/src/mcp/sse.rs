//! MCP's older HTTP+SSE transport, which rmcp no longer ships: a long-lived GET that streams the
//! server's messages, whose first event names the URL to POST the client's messages to.

use futures_util::StreamExt;
use reqwest::header::HeaderMap;
use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
use rmcp::service::RoleClient;
use tokio::sync::mpsc;

use crate::llm::sse::Parser;

pub(super) struct SseTransport {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    headers: HeaderMap,
    incoming: mpsc::Receiver<ServerJsonRpcMessage>,
    reader: tokio::task::JoinHandle<()>,
}

impl SseTransport {
    /// Opens the stream and waits for the server to say where messages go.
    pub(super) async fn connect(
        client: reqwest::Client,
        url: &str,
        headers: HeaderMap,
    ) -> Result<Self, super::Failure> {
        let base = reqwest::Url::parse(url).map_err(|e| format!("{url} is not a URL: {e}"))?;
        let response = client
            .get(base.clone())
            .headers(headers.clone())
            .header("accept", "text/event-stream")
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            let needs_sign_in = matches!(
                response.status(),
                reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
            );
            return Err(super::Failure {
                message: format!("{url} answered {}", response.status()),
                needs_sign_in,
            });
        }

        let mut bytes = response.bytes_stream();
        let mut parser = Parser::default();
        let mut pending: Vec<String> = Vec::new();
        let endpoint = loop {
            let chunk = bytes
                .next()
                .await
                .ok_or("the stream ended before the server named its message endpoint")?
                .map_err(|e| e.to_string())?;
            let events = parser.feed(&chunk);
            if let Some(at) = events.iter().position(|e| e.event == "endpoint") {
                pending.extend(events[at + 1..].iter().map(|e| e.data.clone()));
                break endpoint_of(&base, events[at].data.trim())?;
            }
        };

        let (tx, incoming) = mpsc::channel(64);
        let reader = tokio::spawn(async move {
            for data in pending {
                forward(&tx, &data).await;
            }

            while let Some(Ok(chunk)) = bytes.next().await {
                for event in parser
                    .feed(&chunk)
                    .into_iter()
                    .filter(|e| e.event == "message" || e.event.is_empty())
                {
                    forward(&tx, &event.data).await;
                }
            }
        });

        Ok(Self {
            client,
            endpoint,
            headers,
            incoming,
            reader,
        })
    }
}

/// Where the server says to POST, refused unless it is the stream's own origin, since posts carry its credentials.
fn endpoint_of(base: &reqwest::Url, named: &str) -> Result<reqwest::Url, String> {
    let endpoint = base.join(named).map_err(|e| format!("bad message endpoint: {e}"))?;
    let origin = |url: &reqwest::Url| {
        (
            url.scheme().to_owned(),
            url.host_str().map(str::to_owned),
            url.port_or_known_default(),
        )
    };
    if origin(&endpoint) != origin(base) {
        return Err(format!(
            "the server named a message endpoint on another origin ({endpoint}); Drift only posts to {}",
            base.origin().ascii_serialization()
        ));
    }

    Ok(endpoint)
}

/// A message the server streamed, passed on if it parses; anything else on the stream is not for us.
async fn forward(tx: &mpsc::Sender<ServerJsonRpcMessage>, data: &str) {
    if let Ok(message) = serde_json::from_str::<ServerJsonRpcMessage>(data) {
        let _ = tx.send(message).await;
    }
}

impl Drop for SseTransport {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl rmcp::transport::Transport<RoleClient> for SseTransport {
    type Error = std::io::Error;

    fn send(
        &mut self,
        item: ClientJsonRpcMessage,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'static {
        let request = self
            .client
            .post(self.endpoint.clone())
            .headers(self.headers.clone())
            .json(&item);

        async move {
            let response = request.send().await.map_err(std::io::Error::other)?;
            if response.status().is_success() {
                Ok(())
            } else {
                Err(std::io::Error::other(format!(
                    "the server refused a message: {}",
                    response.status()
                )))
            }
        }
    }

    fn receive(&mut self) -> impl std::future::Future<Output = Option<ServerJsonRpcMessage>> + Send {
        self.incoming.recv()
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        self.reader.abort();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::endpoint_of;

    #[test]
    fn a_message_endpoint_stays_on_the_streams_origin() {
        let base = reqwest::Url::parse("https://mcp.example.com/sse").unwrap();
        let joined = endpoint_of(&base, "/messages?session=1").unwrap();
        assert_eq!(joined.as_str(), "https://mcp.example.com/messages?session=1");
        assert!(endpoint_of(&base, "https://mcp.example.com:443/messages").is_ok());

        for elsewhere in [
            "https://evil.example.net/messages",
            "//evil.example.net/messages",
            "http://mcp.example.com/messages",
            "https://mcp.example.com:8443/messages",
        ] {
            let refused = endpoint_of(&base, elsewhere).unwrap_err();
            assert!(refused.contains("another origin"), "{elsewhere}: {refused}");
        }
    }
}
