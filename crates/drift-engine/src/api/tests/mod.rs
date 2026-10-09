use crate::event::Event;
use crate::store::Workspace;
use crate::{Engine, Server, listen};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio_tungstenite::tungstenite::Message;

mod events;
mod http;
mod mcp;
mod permissions;
mod sessions;
mod settings;

struct Harness {
    engine: Arc<Engine>,
    server: Server,
    http: reqwest::Client,
    directory: TempDir,
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
    let directory = std::env::temp_dir().join(format!("drift-engine-test-{}", u64::from_le_bytes(bytes)));
    let engine = Engine::open_with(
        &directory,
        crate::Options {
            file_credentials: true,
            ..Default::default()
        },
    )
    .unwrap();
    let server = listen(engine.clone(), SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .await
        .unwrap();

    Harness {
        engine,
        server,
        http: reqwest::Client::new(),
        directory: TempDir(directory),
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

    fn patch(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.patch(self.url(path)).bearer_auth(&self.engine.token)
    }

    fn put(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.put(self.url(path)).bearer_auth(&self.engine.token)
    }

    fn delete(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.delete(self.url(path)).bearer_auth(&self.engine.token)
    }

    async fn ws(
        &self,
        query: &str,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
        let url = format!("ws://{}/events?token={}{query}", self.server.addr, self.engine.token);

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

async fn json_response(request: reqwest::RequestBuilder) -> Value {
    request.send().await.unwrap().json().await.unwrap()
}

async fn response_status(request: reqwest::RequestBuilder) -> reqwest::StatusCode {
    request.send().await.unwrap().status()
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

async fn until<S>(socket: &mut S, kind: &str) -> Value
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let frame = next_json(socket).await;
        if frame["type"] == kind {
            return frame;
        }
    }
}

async fn session_with_model(harness: &Harness) -> (String, String) {
    let workspace = harness.directory.0.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let workspace = json_response(
        harness
            .post("/workspaces")
            .json(&json!({ "path": workspace.to_string_lossy(), "name": "ws" })),
    )
    .await;
    let workspace_id = workspace["id"].as_str().unwrap().to_string();
    let session = json_response(
        harness
            .post("/sessions")
            .json(&json!({ "workspaceId": workspace_id, "title": "Test",
        "model": { "provider": "anthropic", "model": "claude-sonnet-4-5" } })),
    )
    .await;

    (workspace_id, session["id"].as_str().unwrap().to_string())
}

fn script_replies(harness: &Harness, text: &str, count: usize) -> crate::llm::scripted::Scripted {
    use crate::llm::scripted::Scripted;
    use crate::llm::{Chunk, Provider, StopReason};

    let provider = Scripted::default();
    for _ in 0..count {
        provider.push(vec![
            Chunk::TextStart,
            Chunk::TextDelta(text.into()),
            Chunk::BlockStop,
            Chunk::Stop(StopReason::EndTurn),
        ]);
    }
    *harness.engine.turns.provider_override.lock().unwrap() = Some(Provider::Scripted(provider.clone()));

    provider
}

async fn submit_and_wait(harness: &Harness, session_id: &str, text: &str) {
    harness
        .post(&format!("/sessions/{session_id}/turns"))
        .json(&json!({ "parts": [{ "type": "text", "text": text }] }))
        .send()
        .await
        .unwrap();

    for _ in 0..200 {
        if !harness.engine.turns.is_running(session_id) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
