use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

use crate::event::Event;
use crate::store::Workspace;
use crate::{listen, Engine, Server};

struct Harness {
    engine: Arc<Engine>,
    server: Server,
    http: reqwest::Client,
    _dir: TempDir,
}

struct TempDir(std::path::PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn harness() -> Harness {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).unwrap();
    let dir = std::env::temp_dir().join(format!("drift-engine-test-{}", u64::from_le_bytes(bytes)));
    let engine = Engine::open_with(&dir, crate::Options { file_credentials: true, ..Default::default() }).unwrap();
    let server = listen(engine.clone(), SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .await
        .unwrap();
    Harness {
        engine,
        server,
        http: reqwest::Client::new(),
        _dir: TempDir(dir),
    }
}

impl Harness {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.server.url())
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.get(self.url(path)).bearer_auth(&self.engine.token)
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.post(self.url(path)).bearer_auth(&self.engine.token)
    }

    async fn ws(&self, query: &str) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
        let url = format!(
            "ws://{}/events?token={}{query}",
            self.server.addr, self.engine.token
        );
        tokio_tungstenite::connect_async(url).await.unwrap().0
    }

    fn workspace(&self, name: &str) -> Workspace {
        Workspace {
            id: name.into(),
            path: format!("C:/{name}"),
            name: name.into(),
            icon: String::new(),
            last_used: 0,
        }
    }
}

async fn next_json<S>(socket: &mut S) -> Value
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let message = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
        .await
        .expect("frame within timeout")
        .unwrap()
        .unwrap();
    serde_json::from_str(message.to_text().unwrap()).unwrap()
}

#[tokio::test]
async fn health_reports_version() {
    let h = harness().await;
    let body: Value = h.get("/health").send().await.unwrap().json().await.unwrap();
    assert_eq!(body["version"], crate::VERSION);
}

#[tokio::test]
async fn requests_without_token_are_rejected() {
    let h = harness().await;
    let status = h.http.get(h.url("/health")).send().await.unwrap().status();
    assert_eq!(status, 401);
    let status = h
        .http
        .get(h.url("/health"))
        .bearer_auth("wrong")
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 401);
}

#[tokio::test]
async fn openapi_lists_every_route() {
    let h = harness().await;
    let doc: Value = h.get("/openapi.json").send().await.unwrap().json().await.unwrap();
    let paths = doc["paths"].as_object().unwrap();
    for route in ["/health", "/workspaces", "/events"] {
        assert!(paths.contains_key(route), "missing {route}");
    }
    assert!(doc["components"]["schemas"]["Frame"].is_object());
}

#[tokio::test]
async fn creating_a_workspace_lists_it_and_publishes_an_event() {
    let h = harness().await;
    let mut socket = h.ws("").await;
    let hello = next_json(&mut socket).await;
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["seq"], 0);

    let created: Value = h
        .post("/workspaces")
        .json(&json!({ "path": "C:/repo", "name": "repo" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["name"], "repo");

    let listed: Value = h.get("/workspaces").send().await.unwrap().json().await.unwrap();
    assert_eq!(listed[0]["id"], created["id"]);

    let event = next_json(&mut socket).await;
    assert_eq!(event["type"], "workspace.created");
    assert_eq!(event["seq"], 1);
    assert_eq!(event["workspace"]["id"], created["id"]);
}

#[tokio::test]
async fn reconnecting_with_a_cursor_replays_missed_events() {
    let h = harness().await;
    for name in ["a", "b", "c"] {
        h.engine.hub.publish(Event::WorkspaceCreated { workspace: h.workspace(name) });
    }
    let mut socket = h.ws("&cursor=1").await;
    let hello = next_json(&mut socket).await;
    assert_eq!(hello["seq"], 3);
    assert_eq!(next_json(&mut socket).await["seq"], 2);
    assert_eq!(next_json(&mut socket).await["seq"], 3);

    h.engine.hub.publish(Event::WorkspaceCreated { workspace: h.workspace("d") });
    assert_eq!(next_json(&mut socket).await["seq"], 4);
}

#[tokio::test]
async fn stale_cursor_gets_resync() {
    let h = harness().await;
    let engine = Engine::open_with(&h._dir.0.join("stale"), crate::Options { event_history: 2, file_credentials: true }).unwrap();
    let server = listen(engine.clone(), SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .await
        .unwrap();
    for name in ["a", "b", "c"] {
        engine.hub.publish(Event::WorkspaceCreated { workspace: h.workspace(name) });
    }
    let url = format!("ws://{}/events?token={}&cursor=0", server.addr, engine.token);
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    assert_eq!(next_json(&mut socket).await["type"], "hello");
    let resync = next_json(&mut socket).await;
    assert_eq!(resync["type"], "resync");
    assert_eq!(resync["seq"], 3);
    server.stop();
}

#[tokio::test]
async fn client_close_ends_the_stream() {
    let h = harness().await;
    let mut socket = h.ws("").await;
    next_json(&mut socket).await;
    socket.send(Message::Close(None)).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(h.engine.hub.publish(Event::WorkspaceCreated { workspace: h.workspace("x") }), 1);
}
