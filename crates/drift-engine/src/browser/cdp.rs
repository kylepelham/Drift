//! A DevTools Protocol connection: one WebSocket to the browser, each call matched to its reply by id, and
//! events handed on in the order they arrive. Pages are reached through flat sessions on the same socket.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

/// A call the browser has not answered in this long has failed; waits for a page are bounded separately.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Full-page screenshots and response bodies are the largest messages.
const MAX_MESSAGE: usize = 128 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub(crate) enum CdpError {
    #[error("the browser closed its connection")]
    Closed,
    #[error("the browser did not answer `{0}` within 30 s")]
    Timeout(String),
    #[error("{method}: {message}")]
    Protocol { method: String, message: String },
}

/// Something the browser reported without being asked; `session` names the page it came from.
#[derive(Debug)]
pub(crate) struct Event {
    pub(crate) method: String,
    pub(crate) params: Value,
    pub(crate) session: Option<String>,
}

type Reply = Result<Value, String>;

pub(crate) struct Connection {
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>,
    outgoing: mpsc::UnboundedSender<Message>,
}

impl Connection {
    /// Connects to a browser's `webSocketDebuggerUrl`; the receiver gets every event until the socket closes.
    pub(crate) async fn connect(url: &str, port: u16) -> std::io::Result<(Arc<Self>, mpsc::UnboundedReceiver<Event>)> {
        let stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE))
            .max_frame_size(Some(MAX_MESSAGE));
        let (socket, _) = tokio_tungstenite::client_async_with_config(url, stream, Some(config))
            .await
            .map_err(std::io::Error::other)?;
        let (mut sink, mut source) = socket.split();

        let (outgoing, mut queued) = mpsc::unbounded_channel::<Message>();
        let (events, received) = mpsc::unbounded_channel();
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>> = Arc::default();

        tokio::spawn(async move {
            while let Some(message) = queued.recv().await {
                if sink.send(message).await.is_err() {
                    break;
                }
            }
        });

        let answering = pending.clone();
        tokio::spawn(async move {
            while let Some(Ok(message)) = source.next().await {
                if let Message::Text(text) = message {
                    dispatch(&text, &answering, &events);
                }
            }
            // Dropping every waiting sender ends each call with `Closed`.
            answering.lock().unwrap().clear();
        });

        let connection = Arc::new(Self {
            next_id: AtomicU64::new(1),
            pending,
            outgoing,
        });

        Ok((connection, received))
    }

    /// Calls `method` on the browser, or on a page when `session` names one.
    pub(crate) async fn call(&self, method: &str, params: Value, session: Option<&str>) -> Result<Value, CdpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }

        let (sender, reply) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, sender);
        if self.outgoing.send(Message::Text(message.to_string().into())).is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err(CdpError::Closed);
        }

        let answer = tokio::time::timeout(CALL_TIMEOUT, reply).await;
        self.pending.lock().unwrap().remove(&id);

        match answer {
            Err(_) => Err(CdpError::Timeout(method.into())),
            Ok(Err(_)) => Err(CdpError::Closed),
            Ok(Ok(Err(message))) => Err(CdpError::Protocol {
                method: method.into(),
                message,
            }),
            Ok(Ok(Ok(result))) => Ok(result),
        }
    }

    /// Sends without waiting for the reply, for acknowledgements and input whose answer says nothing.
    pub(crate) fn send(&self, method: &str, params: Value, session: Option<&str>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }

        let _ = self.outgoing.send(Message::Text(message.to_string().into()));
    }
}

/// A reply resolves its call; anything else is an event.
fn dispatch(text: &str, pending: &Mutex<HashMap<u64, oneshot::Sender<Reply>>>, events: &mpsc::UnboundedSender<Event>) {
    let Ok(mut message) = serde_json::from_str::<Value>(text) else {
        return;
    };

    if let Some(id) = message["id"].as_u64() {
        let Some(sender) = pending.lock().unwrap().remove(&id) else {
            return;
        };
        let reply = match message["error"]["message"].as_str() {
            Some(error) => Err(error.to_string()),
            None => Ok(message["result"].take()),
        };
        let _ = sender.send(reply);
        return;
    }

    let Some(method) = message["method"].as_str().map(str::to_string) else {
        return;
    };
    let _ = events.send(Event {
        method,
        params: message["params"].take(),
        session: message["sessionId"].as_str().map(str::to_string),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_resolves_its_call_and_anything_else_is_an_event() {
        let pending = Mutex::new(HashMap::new());
        let (events, mut received) = mpsc::unbounded_channel();
        let (sender, mut reply) = oneshot::channel();
        pending.lock().unwrap().insert(7, sender);

        dispatch(r#"{"id":7,"result":{"ok":true}}"#, &pending, &events);
        dispatch(
            r#"{"method":"Page.loadEventFired","params":{"t":1},"sessionId":"S"}"#,
            &pending,
            &events,
        );

        assert_eq!(reply.try_recv().unwrap(), Ok(json!({ "ok": true })));
        let event = received.try_recv().unwrap();
        assert_eq!(event.method, "Page.loadEventFired");
        assert_eq!(event.session.as_deref(), Some("S"));
        assert!(pending.lock().unwrap().is_empty());
    }

    #[test]
    fn a_protocol_error_reaches_its_caller_as_text() {
        let pending = Mutex::new(HashMap::new());
        let (events, _received) = mpsc::unbounded_channel();
        let (sender, mut reply) = oneshot::channel();
        pending.lock().unwrap().insert(2, sender);

        dispatch(
            r#"{"id":2,"error":{"code":-32000,"message":"No node found"}}"#,
            &pending,
            &events,
        );

        assert_eq!(reply.try_recv().unwrap(), Err("No node found".to_string()));
    }
}
