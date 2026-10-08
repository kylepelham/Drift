use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use utoipa::{IntoParams, ToSchema};

use super::error::ErrorBody;
use crate::Engine;
use crate::event::{Envelope, Replay};

/// Everything the server writes to the socket.
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum Frame {
    Control(Control),
    Event(Box<Envelope>),
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(tag = "type")]
pub enum Control {
    /// First frame. A client without a cursor, or one that knew another `instance`, hydrates and then trusts events after `seq`.
    #[serde(rename = "hello")]
    Hello {
        version: String,
        instance: String,
        seq: u64,
    },
    /// The cursor was too old to replay: hydrate again, then trust events after `seq`.
    #[serde(rename = "resync")]
    Resync { seq: u64 },
    /// How a `question.reply` sent on this socket ended; only the socket that sent it hears.
    #[serde(rename = "question.result", rename_all = "camelCase")]
    QuestionResult {
        request_id: String,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<ErrorBody>,
    },
}

#[derive(Deserialize, IntoParams)]
pub struct EventsQuery {
    /// Last `seq` the client has applied; omit on first connect.
    pub cursor: Option<u64>,
}

/// Set by a host that serves the engine to others (the remote gateway): the socket closes once it is
/// cancelled, as when the device that opened it is signed out, so nothing it holds outlives its access.
#[derive(Clone, Default)]
pub struct Lease(pub tokio_util::sync::CancellationToken);

impl Lease {
    pub fn cancel(&self) {
        self.0.cancel();
    }
}

#[utoipa::path(
    get,
    path = "/events",
    operation_id = "events",
    params(EventsQuery),
    responses((status = 101, description = "WebSocket; every message is a Frame"))
)]
pub async fn get(
    State(engine): State<Arc<Engine>>,
    Query(query): Query<EventsQuery>,
    lease: Option<axum::Extension<Lease>>,
    ws: WebSocketUpgrade,
) -> Response {
    let lease = lease.map(|axum::Extension(Lease(token))| token).unwrap_or_default();
    ws.on_upgrade(move |socket| run(engine, socket, query.cursor, lease))
}

struct Client {
    socket: WebSocket,
    last: u64,
}

impl Client {
    async fn send(&mut self, frame: &Frame) -> bool {
        let text = serde_json::to_string(frame).expect("frames are always serialisable");
        self.socket.send(Message::Text(text.into())).await.is_ok()
    }

    async fn send_event(&mut self, envelope: Envelope) -> bool {
        self.last = envelope.seq;
        self.send(&Frame::Event(Box::new(envelope))).await
    }

    async fn resync(&mut self, seq: u64) -> bool {
        self.last = seq;
        self.send(&Frame::Control(Control::Resync { seq })).await
    }

    /// Delivers a replay batch, or tells the client to hydrate if the batch is gone.
    async fn catch_up(&mut self, seq: u64, replay: Replay) -> bool {
        match replay {
            Replay::Stale => self.resync(seq).await,
            Replay::Events(events) => {
                for event in events {
                    if !self.send_event(event).await {
                        return false;
                    }
                }
                true
            }
        }
    }
}

/// Numbers each socket, so what it says it has open is its own and ends with it.
static SOCKETS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Forgets the workspace a socket had open once it closes, however it closes.
struct Opened {
    engine: Arc<Engine>,
    socket: u64,
}

impl Drop for Opened {
    fn drop(&mut self) {
        self.engine.mcp.set_open(self.socket, None);
    }
}

async fn run(engine: Arc<Engine>, socket: WebSocket, cursor: Option<u64>, lease: tokio_util::sync::CancellationToken) {
    let opened = Opened {
        engine: engine.clone(),
        socket: SOCKETS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    };
    let attached = engine.hub.attach(cursor);
    let mut rx = attached.rx;
    let mut client = Client {
        socket,
        last: cursor.unwrap_or(attached.seq),
    };
    let hello = Control::Hello {
        version: crate::VERSION.into(),
        instance: engine.hub.instance.clone(),
        seq: attached.seq,
    };
    if !client.send(&Frame::Control(hello)).await || !client.catch_up(attached.seq, attached.replay).await {
        return;
    }
    let (results, mut finished) = mpsc::unbounded_channel();
    loop {
        tokio::select! {
            () = lease.cancelled() => return,
            Some(result) = finished.recv() => if !client.send(&Frame::Control(result)).await { return },
            received = rx.recv() => match received {
                Ok(envelope) => if !client.send_event(envelope).await { return },
                Err(RecvError::Lagged(_)) => {
                    let again = engine.hub.attach(Some(client.last));
                    rx = again.rx;
                    if !client.catch_up(again.seq, again.replay).await { return }
                }
                Err(RecvError::Closed) => return,
            },
            incoming = client.socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                Some(Ok(Message::Text(text))) => handle(&engine, opened.socket, &text, &results),
                Some(Ok(_)) => {}
            },
        }
    }
}

/// Replies can ride the socket so a permission prompt never waits on a new HTTP connection.
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(tag = "type")]
pub enum Incoming {
    #[serde(rename = "permission.reply", rename_all = "camelCase")]
    PermissionReply {
        request_id: String,
        #[serde(flatten)]
        body: crate::permission::ReplyBody,
    },
    /// Answers absent means the user declined. Its outcome comes back as `question.result`.
    #[serde(rename = "question.reply", rename_all = "camelCase")]
    QuestionReply {
        request_id: String,
        answers: Option<Vec<Vec<String>>>,
    },
    /// The workspace folder this client shows now, or none: its stdio MCP servers keep running while any client has it open.
    #[serde(rename = "workspace.open", rename_all = "camelCase")]
    WorkspaceOpen { directory: Option<String> },
}

fn handle(engine: &Arc<Engine>, socket: u64, text: &str, results: &mpsc::UnboundedSender<Control>) {
    let Ok(incoming) = serde_json::from_str::<Incoming>(text) else {
        return;
    };
    match incoming {
        Incoming::PermissionReply { request_id, body } => {
            let _ = engine.permissions.reply(&engine.hub, &request_id, body);
        }
        // An async answer is saved before its card closes, which takes a moment; the socket does not wait for it.
        Incoming::QuestionReply { request_id, answers } => {
            let (engine, results) = (engine.clone(), results.clone());
            tokio::spawn(async move {
                let error = engine
                    .answer_question(&request_id, answers)
                    .await
                    .err()
                    .map(|error| super::questions::answer_error(error).body);
                let _ = results.send(Control::QuestionResult {
                    request_id,
                    ok: error.is_none(),
                    error,
                });
            });
        }
        Incoming::WorkspaceOpen { directory } => {
            engine.mcp.set_open(
                socket,
                directory.map(|directory| crate::tool::canonical(std::path::Path::new(&directory))),
            );
        }
    }
}
