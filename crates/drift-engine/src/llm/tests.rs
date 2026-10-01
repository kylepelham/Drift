//! Adapters against a fake provider that replays recorded SSE in awkward chunk sizes.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::post;
use axum::{Json, Router};
use futures_util::StreamExt;
use serde_json::Value;

use super::anthropic::Anthropic;
use super::{Block, ChatMessage, Chunk, Credential, Error, Request, Role, StopReason};
use crate::session::types::Usage;

struct Seen {
    headers: HeaderMap,
    query: Option<String>,
    body: Value,
}

struct Fake {
    seen: Mutex<Option<Seen>>,
    reply: Mutex<(u16, String)>,
}

async fn handle(State(fake): State<Arc<Fake>>, uri: axum::http::Uri, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    *fake.seen.lock().unwrap() = Some(Seen { headers, query: uri.query().map(str::to_string), body });
    let (status, text) = fake.reply.lock().unwrap().clone();
    if status != 200 {
        return Response::builder().status(status).header("retry-after", "3").body(Body::from(text)).unwrap();
    }
    // Seven-byte chunks force every frame boundary to land mid-line somewhere.
    let chunks: Vec<Result<Vec<u8>, std::io::Error>> = text.as_bytes().chunks(7).map(|c| Ok(c.to_vec())).collect();
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(futures_util::stream::iter(chunks)))
        .unwrap()
}

async fn fake(status: u16, reply: &str) -> (Arc<Fake>, String) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fake = Arc::new(Fake { seen: Mutex::new(None), reply: Mutex::new((status, reply.into())) });
    let router = Router::new().route("/v1/messages", post(handle)).with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (fake, url)
}

pub(crate) fn request() -> Request {
    Request {
        model: "claude-sonnet-4-5".into(),
        system: "sys".into(),
        messages: vec![ChatMessage { role: Role::User, blocks: vec![Block::Text("read main".into())] }],
        tools: vec![],
        max_tokens: 100,
        thinking_budget: None,
        temperature: None,
        cache_key: None,
    }
}

#[tokio::test]
async fn anthropic_streams_thinking_text_and_tool_use() {
    let fixture = include_str!("../../fixtures/anthropic/tool_call.sse");
    let (fake, url) = fake(200, fixture).await;
    let provider = Anthropic::new(&url);
    let stream = provider.stream(&request(), &Credential::ApiKey { key: "k".into() }).await.unwrap();
    let chunks: Vec<Chunk> = stream.map(|c| c.unwrap()).collect().await;
    assert_eq!(
        chunks,
        vec![
            Chunk::Usage(Usage { input: 25, output: 1, cache_read: 0, cache_write: 10 }),
            Chunk::ReasoningStart,
            Chunk::ReasoningDelta("I should read ".into()),
            Chunk::ReasoningDelta("the file.".into()),
            Chunk::ReasoningSignature("EqQBCgIYAhIM".into()),
            Chunk::BlockStop,
            Chunk::TextStart,
            Chunk::TextDelta("Let me look".into()),
            Chunk::TextDelta(" at that.".into()),
            Chunk::BlockStop,
            Chunk::ToolUseStart { id: "toolu_01A".into(), name: "read".into() },
            Chunk::ToolInputDelta(String::new()),
            Chunk::ToolInputDelta("{\"path\": \"src/ma".into()),
            Chunk::ToolInputDelta("in.rs\"}".into()),
            Chunk::BlockStop,
            Chunk::Usage(Usage { output: 42, ..Usage::default() }),
            Chunk::Stop(StopReason::ToolUse),
        ]
    );
    let seen = fake.seen.lock().unwrap();
    let seen = seen.as_ref().unwrap();
    assert_eq!(seen.headers["x-api-key"], "k");
    assert_eq!(seen.headers["anthropic-version"], "2023-06-01");
    assert_eq!(seen.body["model"], "claude-sonnet-4-5");
}

#[tokio::test]
async fn subscription_tokens_send_the_claude_code_shape_and_unprefix_tool_names() {
    let reply = concat!(
        "event: content_block_start\n",
        "data: {\"content_block\":{\"type\":\"tool_use\",\"id\":\"t\",\"name\":\"mcp_Read\"}}\n\n",
        "event: message_stop\ndata: {}\n\n"
    );
    let (fake, url) = fake(200, reply).await;
    let credential = Credential::OAuth { access: "tok".into(), refresh: String::new(), expires_at: 0, account: None };
    let mut request = request();
    request.tools = vec![crate::llm::ToolSpec { name: "read".into(), description: "r".into(), input_schema: serde_json::json!({}) }];
    let stream = Anthropic::new(&url).stream(&request, &credential).await.unwrap();
    let chunks: Vec<Chunk> = stream.map(|c| c.unwrap()).collect().await;
    assert_eq!(chunks[0], Chunk::ToolUseStart { id: "t".into(), name: "read".into() });
    let seen = fake.seen.lock().unwrap();
    let seen = seen.as_ref().unwrap();
    assert_eq!(seen.headers["authorization"], "Bearer tok");
    assert_eq!(seen.headers["anthropic-beta"], "oauth-2025-04-20,interleaved-thinking-2025-05-14");
    assert!(seen.headers["user-agent"].to_str().unwrap().starts_with("claude-cli/"));
    assert!(seen.headers.get("x-api-key").is_none());
    assert_eq!(seen.query.as_deref(), Some("beta=true"));
    assert_eq!(seen.body["system"][1]["text"], "You are a Claude agent, built on Anthropic's Claude Agent SDK.");
    assert_eq!(seen.body["system"][2]["text"], "sys");
    assert_eq!(seen.body["tools"][0]["name"], "mcp_Read");
}

/// A server that accepts connections and never answers.
pub(crate) async fn silent_server() -> String {
    let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    url
}

#[tokio::test]
async fn a_response_that_never_begins_is_a_transport_failure() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut provider = Anthropic::new(&silent_server().await);
    provider.timeouts.headers = std::time::Duration::from_millis(200);
    let started = std::time::Instant::now();
    let Err(error) = provider.stream(&request(), &Credential::ApiKey { key: "k".into() }).await else { panic!("expected an error") };
    assert!(matches!(&error, Error::Transport(message) if message.contains("no response")), "{error:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

#[tokio::test]
async fn a_stream_that_stalls_mid_reply_fails_instead_of_hanging() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let router = Router::new().route(
        "/v1/messages",
        post(|| async {
            let start: Result<&'static str, std::io::Error> = Ok("event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":1}}}\n\n");
            let body = Body::from_stream(futures_util::stream::iter([start]).chain(futures_util::stream::pending()));
            Response::builder().header("content-type", "text/event-stream").body(body).unwrap()
        }),
    );
    let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut provider = Anthropic::new(&url);
    provider.timeouts.idle = std::time::Duration::from_millis(300);
    let mut stream = provider.stream(&request(), &Credential::ApiKey { key: "k".into() }).await.unwrap();
    assert!(matches!(stream.next().await, Some(Ok(Chunk::Usage(_)))));
    let stalled = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next()).await.expect("the idle limit ends it");
    assert!(matches!(&stalled, Some(Err(Error::Transport(message))) if message.contains("stalled")), "{stalled:?}");
}

async fn error_server(body: Body) -> String {
    let body = Arc::new(Mutex::new(Some(body)));
    let router = Router::new().route("/v1/messages", post(move || {
        let body = body.lock().unwrap().take().unwrap_or_else(Body::empty);
        async move { Response::builder().status(429).header("content-type", "application/json").body(body).unwrap() }
    }));
    let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    url
}

#[tokio::test]
async fn an_error_body_that_stalls_or_never_ends_is_cut_short() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let start: Result<&'static str, std::io::Error> = Ok("{\"error\":{\"type\":\"rate_limit_error\"");
    let stalled = Body::from_stream(futures_util::stream::iter([start]).chain(futures_util::stream::pending()));
    let mut provider = Anthropic::new(&error_server(stalled).await);
    provider.timeouts.idle = std::time::Duration::from_millis(200);
    let started = std::time::Instant::now();
    let error = tokio::time::timeout(std::time::Duration::from_secs(3), provider.stream(&request(), &Credential::ApiKey { key: "k".into() })).await.expect("bounded").err().unwrap();
    assert!(matches!(error, Error::Api { status: 429, retryable: true, .. }), "{error:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(2));

    let endless = Body::from(vec![b'x'; 1024 * 1024]);
    let provider = Anthropic::new(&error_server(endless).await);
    let Err(Error::Api { message, .. }) = provider.stream(&request(), &Credential::ApiKey { key: "k".into() }).await else { panic!("expected an error") };
    assert!(message.len() <= 64 * 1024, "{}", message.len());
}

#[tokio::test]
async fn http_errors_become_api_errors() {
    let (_, url) = fake(529, r#"{"error":{"type":"overloaded_error","message":"Overloaded"}}"#).await;
    let Err(error) = Anthropic::new(&url).stream(&request(), &Credential::ApiKey { key: "k".into() }).await else {
        panic!("expected an error");
    };
    match error {
        Error::Api { status, kind, retryable, retry_after, .. } => {
            assert_eq!((status, kind.as_str(), retryable), (529, "overloaded_error", true));
            assert_eq!(retry_after, Some(std::time::Duration::from_secs(3)), "the response's retry-after reaches the turn");
        }
        other => panic!("unexpected {other:?}"),
    }
}
