//! A WebDriver BiDi connection: one WebSocket to Firefox, commands matched to their replies by id, and events
//! handed on in the order they arrive.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

const CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// Full-page screenshots are the largest messages.
const MAX_MESSAGE: usize = 128 * 1024 * 1024;

type Reply = Result<Value, String>;

/// Something Firefox reported without being asked.
pub(crate) struct Event {
    pub(crate) method: String,
    pub(crate) params: Value,
}

pub(crate) struct Bidi {
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>,
    outgoing: mpsc::UnboundedSender<Message>,
}

impl Bidi {
    /// Connects to `ws://127.0.0.1:<port>/session`; the receiver gets every event until the socket closes.
    pub(crate) async fn connect(port: u16) -> std::io::Result<(Arc<Self>, mpsc::UnboundedReceiver<Event>)> {
        let stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE))
            .max_frame_size(Some(MAX_MESSAGE));
        let url = format!("ws://127.0.0.1:{port}/session");
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
            answering.lock().unwrap().clear();
        });

        let bidi = Arc::new(Self {
            next_id: AtomicU64::new(1),
            pending,
            outgoing,
        });

        Ok((bidi, received))
    }

    pub(crate) async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = json!({ "id": id, "method": method, "params": params });

        let (sender, reply) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, sender);
        if self.outgoing.send(Message::Text(message.to_string().into())).is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err("Firefox closed its connection".into());
        }

        let answer = tokio::time::timeout(CALL_TIMEOUT, reply).await;
        self.pending.lock().unwrap().remove(&id);

        match answer {
            Err(_) => Err(format!("Firefox did not answer `{method}` within 60 s")),
            Ok(Err(_)) => Err("Firefox closed its connection".into()),
            Ok(Ok(reply)) => reply,
        }
    }
}

fn dispatch(text: &str, pending: &Mutex<HashMap<u64, oneshot::Sender<Reply>>>, events: &mpsc::UnboundedSender<Event>) {
    let Ok(mut message) = serde_json::from_str::<Value>(text) else {
        return;
    };

    if message["type"] == "event" {
        let _ = events.send(Event {
            method: message["method"].as_str().unwrap_or_default().to_string(),
            params: message["params"].take(),
        });
        return;
    }

    let Some(sender) = message["id"]
        .as_u64()
        .and_then(|id| pending.lock().unwrap().remove(&id))
    else {
        return;
    };
    let reply = if message["type"] == "error" {
        let detail = message["message"].as_str().unwrap_or_default();
        let code = message["error"].as_str().unwrap_or("error");
        Err(if detail.is_empty() {
            code.to_string()
        } else {
            detail.lines().next().unwrap_or(detail).to_string()
        })
    } else {
        Ok(message["result"].take())
    };
    let _ = sender.send(reply);
}

/// A BiDi remote value as plain JSON: strings, numbers, lists and objects, with anything else described.
pub(crate) fn plain(value: &Value) -> Value {
    match value["type"].as_str() {
        Some("string" | "boolean") => value["value"].clone(),
        Some("number") => match &value["value"] {
            Value::String(special) => Value::String(special.clone()),
            number => number.clone(),
        },
        Some("null" | "undefined") | None => Value::Null,
        Some("array" | "set") => Value::Array(value["value"].as_array().into_iter().flatten().map(plain).collect()),
        Some("object" | "map") => {
            let entries = value["value"].as_array().into_iter().flatten();
            let object = entries
                .filter_map(|entry| {
                    let key = entry[0]
                        .as_str()
                        .map(str::to_string)
                        .or_else(|| entry[0]["value"].as_str().map(str::to_string))?;
                    Some((key, plain(&entry[1])))
                })
                .collect();
            Value::Object(object)
        }
        Some(other) => Value::String(format!("[{other}]")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_and_events_are_told_apart() {
        let pending = Mutex::new(HashMap::new());
        let (events, mut received) = mpsc::unbounded_channel();
        let (sender, mut reply) = oneshot::channel();
        pending.lock().unwrap().insert(3, sender);

        dispatch(
            r#"{"type":"error","id":3,"error":"no such frame","message":"Gone\nstack"}"#,
            &pending,
            &events,
        );
        dispatch(
            r#"{"type":"event","method":"log.entryAdded","params":{"text":"hi"}}"#,
            &pending,
            &events,
        );

        assert_eq!(reply.try_recv().unwrap(), Err("Gone".to_string()));
        assert_eq!(received.try_recv().unwrap().method, "log.entryAdded");
    }

    #[test]
    fn remote_values_become_plain_json() {
        let remote = json!({ "type": "object", "value": [
            ["name", { "type": "string", "value": "Ada" }],
            ["tags", { "type": "array", "value": [{ "type": "number", "value": 2 }] }],
            ["gone", { "type": "undefined" }]
        ]});

        assert_eq!(plain(&remote), json!({ "name": "Ada", "tags": [2], "gone": null }));
    }
}
