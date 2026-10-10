//! A local Responses server that speaks both transports and records what each connection received.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use serde_json::json;

use super::*;
use crate::llm::openai::OpenAi;
use crate::llm::{Block, ChatMessage, Chunk, Credential, Request, Role};

mod connections;
mod continuation;
mod fallback;
mod live;

/// How the server behaves, and what it saw.
#[derive(Default)]
struct Server {
    /// Answer the first request with a tool call rather than text.
    tool_first: bool,
    /// Send part of a reply, then close the socket.
    partial: bool,
    /// Accept requests but never answer them.
    stall: bool,
    /// Ping the client after each reply, as a quiet connection would.
    ping_idle: bool,
    /// Refuse the next continuation as if the server had forgotten it.
    lose_previous: AtomicBool,
    /// Close the socket after the next reply.
    close_after_reply: AtomicBool,

    connections: AtomicUsize,
    /// Sockets that ended from the client's side.
    closed: AtomicUsize,
    posts: AtomicUsize,
    pongs: AtomicUsize,
    frames: Mutex<Vec<Received>>,
}

/// One `response.create` the server received.
struct Received {
    headers: HeaderMap,
    body: Value,
}

impl Server {
    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    fn posts(&self) -> usize {
        self.posts.load(Ordering::SeqCst)
    }

    fn body(&self, index: usize) -> Value {
        self.frames.lock().unwrap()[index].body.clone()
    }

    fn requests(&self) -> usize {
        self.frames.lock().unwrap().len()
    }
}

struct Fixture {
    provider: OpenAi,
    server: Arc<Server>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Fixture {
    /// The first slot the provider's pool holds.
    fn slot(&self) -> Slot {
        let entries = self.provider.websockets.entries.lock().unwrap();
        entries.values().next().unwrap().slot.clone()
    }
}

/// Starts the server; `refuse` answers every upgrade with that status instead.
async fn fixture(server: Server, refuse: Option<StatusCode>) -> Fixture {
    let server = Arc::new(server);

    let upgrades = server.clone();
    let upgrade = move |headers: HeaderMap, ws: WebSocketUpgrade| {
        let server = upgrades.clone();
        async move { accept(ws, headers, server, refuse) }
    };

    let posts = server.clone();
    let post = move || {
        let server = posts.clone();
        async move { answer_over_sse(&server) }
    };

    let app = axum::Router::new().route("/responses", axum::routing::get(upgrade).post(post));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    // Each fixture gets its own pool, so tests never share connections.
    let mut provider = OpenAi::new(&base);
    provider.websockets = Arc::default();

    Fixture { provider, server, task }
}

fn accept(ws: WebSocketUpgrade, headers: HeaderMap, server: Arc<Server>, refuse: Option<StatusCode>) -> Response {
    if let Some(status) = refuse {
        let error = json!({ "error": { "code": "handshake_error", "message": "upgrade refused" } });
        return (status, axum::Json(error)).into_response();
    }

    ws.on_upgrade(move |socket| serve(socket, headers, server))
}

fn answer_over_sse(server: &Server) -> Response {
    server.posts.fetch_add(1, Ordering::SeqCst);

    (
        [("content-type", "text/event-stream")],
        "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{}}}\n\n",
    )
        .into_response()
}

async fn serve(mut socket: WebSocket, headers: HeaderMap, server: Arc<Server>) {
    server.connections.fetch_add(1, Ordering::SeqCst);

    while let Some(Ok(frame)) = socket.recv().await {
        let Message::Text(text) = frame else {
            if matches!(frame, Message::Pong(_)) {
                server.pongs.fetch_add(1, Ordering::SeqCst);
            }
            continue;
        };

        // Record the request; its position names the response it gets.
        let body: Value = serde_json::from_str(&text).unwrap();
        let continues = body.get("previous_response_id").is_some();
        let number = {
            let mut frames = server.frames.lock().unwrap();
            frames.push(Received {
                headers: headers.clone(),
                body,
            });
            frames.len()
        };

        if server.stall {
            continue;
        }

        if continues && server.lose_previous.swap(false, Ordering::SeqCst) {
            let error = json!({ "code": "previous_response_not_found", "message": "evicted" });
            event(&mut socket, json!({ "type": "error", "status": 400, "error": error })).await;
            continue;
        }

        if !reply(&mut socket, number, &server).await {
            return;
        }
    }

    server.closed.fetch_add(1, Ordering::SeqCst);
}

/// Answers one request; false when the socket was closed.
async fn reply(socket: &mut WebSocket, number: usize, server: &Server) -> bool {
    if server.partial {
        event(
            socket,
            json!({ "type": "response.output_item.added", "item": message() }),
        )
        .await;
        event(
            socket,
            json!({ "type": "response.output_text.delta", "delta": "partial" }),
        )
        .await;
        let _ = socket.send(Message::Close(None)).await;
        return false;
    }

    answer(socket, number, server.tool_first && number == 1).await;

    if server.close_after_reply.swap(false, Ordering::SeqCst) {
        let _ = socket.send(Message::Close(None)).await;
        return false;
    }

    if server.ping_idle {
        let _ = socket.send(Message::Ping(vec![1, 2].into())).await;
    }

    true
}

async fn answer(socket: &mut WebSocket, number: usize, tool: bool) {
    let id = format!("resp_{number}");
    let item = if tool { call() } else { message() };

    event(socket, json!({ "type": "response.created", "response": { "id": id } })).await;
    event(socket, json!({ "type": "response.output_item.added", "item": item })).await;
    if !tool {
        event(socket, json!({ "type": "response.output_text.delta", "delta": "ok" })).await;
    }
    event(socket, json!({ "type": "response.output_item.done", "item": item })).await;

    let usage = json!({ "input_tokens": 10, "output_tokens": 2, "input_tokens_details": { "cached_tokens": 3 } });
    let response = json!({ "id": id, "output": [item], "usage": usage });
    event(socket, json!({ "type": "response.completed", "response": response })).await;
}

async fn event(socket: &mut WebSocket, value: Value) {
    socket.send(Message::Text(value.to_string().into())).await.unwrap();
}

fn message() -> Value {
    let content = json!([{ "type": "output_text", "text": "ok", "annotations": [], "logprobs": [] }]);
    json!({ "type": "message", "id": "msg", "status": "completed", "role": "assistant", "content": content })
}

fn call() -> Value {
    json!({
        "type": "function_call", "id": "fc", "status": "completed",
        "call_id": "call", "name": "read", "arguments": "{\"path\": \"a.txt\"}"
    })
}

fn request() -> Request {
    Request {
        model: "gpt-6-sol".into(),
        system: "Answer briefly.".into(),
        messages: vec![text(Role::User, "hello")],
        tools: vec![],
        max_tokens: 100,
        reasoning: None,
        temperature: None,
        cache_key: Some("session".into()),
        no_tool_calls: false,
        verbosity: None,
        show_thinking: false,
        top_p: None,
        top_k: None,
        mode: None,
    }
}

/// The request after the model answered "ok" and the user wrote again.
fn continued(mut request: Request) -> Request {
    request.messages.push(text(Role::Assistant, "ok"));
    request.messages.push(text(Role::User, "again"));
    request
}

fn text(role: Role, text: &str) -> ChatMessage {
    ChatMessage {
        role,
        blocks: vec![Block::Text(text.into())],
    }
}

fn key() -> Credential {
    Credential::ApiKey { key: "key".into() }
}

async fn collect(provider: &OpenAi, request: &Request, credential: &Credential) -> Vec<Result<Chunk, Error>> {
    provider.stream(request, credential).await.unwrap().collect().await
}

/// Waits briefly for something a background task does.
async fn eventually(condition: impl AsyncFn() -> bool) {
    for _ in 0..100 {
        if condition().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
