//! JSON-RPC 2.0 over one stream each way, one message per line, with requests going both ways.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, PartialEq)]
pub(super) struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    pub(super) fn parse(error: impl std::fmt::Display) -> Self {
        Self::new(-32700, format!("not JSON: {error}"))
    }

    pub(super) fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(-32600, message)
    }

    pub(super) fn method_not_found(method: &str) -> Self {
        Self::new(-32601, format!("unknown method: {method}"))
    }

    pub(super) fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(-32602, message)
    }

    pub(super) fn internal(error: impl std::fmt::Display) -> Self {
        Self::new(-32603, error.to_string())
    }

    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Requests sent to the client, by id, each waiting for its result; `None` when it answered with an error.
type Waiting = Arc<Mutex<HashMap<u64, oneshot::Sender<Option<Value>>>>>;

/// The client end: everything written goes through one queue, so messages leave in the order they were sent.
#[derive(Clone)]
pub(super) struct Peer {
    out: mpsc::UnboundedSender<Value>,
    waiting: Waiting,
    next: Arc<AtomicU64>,
}

impl Peer {
    /// The peer, and the writer that ends once every clone of it is gone and the queue is written.
    pub(super) fn new(output: impl AsyncWrite + Unpin + Send + 'static) -> (Self, tokio::task::JoinHandle<()>) {
        let (out, mut queue) = mpsc::unbounded_channel::<Value>();
        let writer = tokio::spawn(async move {
            let mut output = output;
            while let Some(message) = queue.recv().await {
                // Compact JSON escapes every newline inside strings, so one message stays one line.
                let line = format!("{message}\n");
                if output.write_all(line.as_bytes()).await.is_err() || output.flush().await.is_err() {
                    return;
                }
            }
        });

        let peer = Self {
            out,
            waiting: Arc::default(),
            next: Arc::new(AtomicU64::new(0)),
        };

        (peer, writer)
    }

    pub(super) fn notify(&self, method: &str, params: Value) {
        let _ = self
            .out
            .send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    pub(super) fn respond(&self, id: Value, result: Result<Value, RpcError>) {
        let message = match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": error.code, "message": error.message },
            }),
        };
        let _ = self.out.send(message);
    }

    /// The client's result, or `None` when it answered with an error or went away.
    pub(super) async fn request(&self, method: &str, params: Value) -> Option<Value> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (answer, answered) = oneshot::channel();
        self.waiting.lock().unwrap().insert(id, answer);
        let _ = self
            .out
            .send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));

        answered.await.ok().flatten()
    }

    /// Hands a response from the client to the request waiting on it.
    pub(super) fn settle(&self, message: &Value) {
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            return;
        };
        if let Some(waiter) = self.waiting.lock().unwrap().remove(&id) {
            let _ = waiter.send(message.get("result").cloned());
        }
    }

    /// The client is gone: every request still waiting hears nothing.
    pub(super) fn hang_up(&self) {
        self.waiting.lock().unwrap().clear();
    }
}
