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

    fn patch(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.patch(self.url(path)).bearer_auth(&self.engine.token)
    }

    fn put(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.put(self.url(path)).bearer_auth(&self.engine.token)
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
    for route in ["/health", "/workspaces", "/sessions", "/sessions/{id}/turns", "/providers", "/permissions/{id}/reply", "/events"] {
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

async fn session_with_model(h: &Harness) -> (String, String) {
    let workspace = h._dir.0.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let ws: Value = h.post("/workspaces").json(&json!({ "path": workspace.to_string_lossy(), "name": "ws" })).send().await.unwrap().json().await.unwrap();
    let ws_id = ws["id"].as_str().unwrap().to_string();
    let created: Value = h
        .post("/sessions")
        .json(&json!({ "workspaceId": ws_id, "model": { "provider": "anthropic", "model": "claude-sonnet-4-5" } }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    (ws_id, created["id"].as_str().unwrap().to_string())
}

#[tokio::test]
async fn a_full_turn_over_http_and_ws_with_a_permission_reply_on_the_socket() {
    use crate::llm::scripted::Scripted;
    use crate::llm::{Chunk, Provider, StopReason};
    let h = harness().await;
    let provider = Scripted::default();
    provider
        .push(vec![
            Chunk::ToolUseStart { id: "t1".into(), name: "write".into() },
            Chunk::ToolInputDelta(r#"{"path":"out.txt","content":"done\n"}"#.into()),
            Chunk::BlockStop,
            Chunk::Stop(StopReason::ToolUse),
        ])
        .push(vec![Chunk::TextStart, Chunk::TextDelta("Wrote it".into()), Chunk::BlockStop, Chunk::Stop(StopReason::EndTurn)]);
    *h.engine.turns.provider_override.lock().unwrap() = Some(Provider::Scripted(provider));
    let status = h.put("/providers/anthropic/key").json(&json!({ "key": "sk-test" })).send().await.unwrap().status();
    assert_eq!(status, 204);
    let providers: Value = h.get("/providers").send().await.unwrap().json().await.unwrap();
    let anthropic = providers.as_array().unwrap().iter().find(|p| p["id"] == "anthropic").unwrap();
    assert_eq!(anthropic["connected"], true);
    assert_eq!(anthropic["credential"], "keychain");

    let mut socket = h.ws("").await;
    let (_, session_id) = session_with_model(&h).await;
    let response = h.post(&format!("/sessions/{session_id}/turns")).json(&json!({ "parts": [{ "type": "text", "text": "write out.txt" }] })).send().await.unwrap();
    assert_eq!(response.status(), 202);
    let receipt: Value = response.json().await.unwrap();
    assert_eq!(receipt["message"]["role"], "user");
    let busy = h.post(&format!("/sessions/{session_id}/turns")).json(&json!({ "parts": [] })).send().await.unwrap().status();
    assert_eq!(busy, 409);

    let running = until(&mut socket, "session.status").await;
    assert_eq!(running["status"], "running");
    let asked = until(&mut socket, "permission.asked").await;
    let request_id = asked["request"]["id"].as_str().unwrap();
    assert_eq!(asked["request"]["tool"], "write");
    let pending: Value = h.get("/permissions").send().await.unwrap().json().await.unwrap();
    assert_eq!(pending[0]["id"], request_id);
    let reply = json!({ "type": "permission.reply", "requestId": request_id, "reply": "once" }).to_string();
    socket.send(Message::Text(reply.into())).await.unwrap();

    let idle = until(&mut socket, "session.status").await;
    assert_eq!(idle["status"], "idle");
    assert_eq!(std::fs::read_to_string(h._dir.0.join("ws/out.txt")).unwrap(), "done\n");

    let messages: Value = h.get(&format!("/sessions/{session_id}/messages")).send().await.unwrap().json().await.unwrap();
    let messages = messages.as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1]["parts"][0]["type"], "tool_call");
    assert_eq!(messages[1]["parts"][0]["status"], "done");
    assert_eq!(messages[2]["parts"][0]["text"], "Wrote it");
    let listed: Value = h.get("/sessions").send().await.unwrap().json().await.unwrap();
    assert_eq!(listed[0]["title"], "write out.txt");
}

#[tokio::test]
async fn sessions_can_be_renamed_archived_and_paged() {
    let h = harness().await;
    let (ws_id, session_id) = session_with_model(&h).await;
    let renamed: Value = h.patch(&format!("/sessions/{session_id}")).json(&json!({ "title": "Renamed" })).send().await.unwrap().json().await.unwrap();
    assert_eq!(renamed["title"], "Renamed");
    let archived: Value = h.patch(&format!("/sessions/{session_id}")).json(&json!({ "archived": true })).send().await.unwrap().json().await.unwrap();
    assert!(archived["archivedAt"].is_number());
    let live: Value = h.get(&format!("/sessions?workspace={ws_id}")).send().await.unwrap().json().await.unwrap();
    assert_eq!(live.as_array().unwrap().len(), 0);
    let gone: Value = h.get("/sessions?archived=true").send().await.unwrap().json().await.unwrap();
    assert_eq!(gone[0]["id"], session_id);
    assert_eq!(h.get("/sessions/ses_nope").send().await.unwrap().status(), 404);
    let no_model = h.post(&format!("/sessions/{session_id}/turns")).json(&json!({ "parts": [], "model": null })).send().await.unwrap();
    assert_eq!(no_model.status(), 401);
    let body: Value = no_model.json().await.unwrap();
    assert_eq!(body["code"], "credentials");
}

#[tokio::test]
async fn oauth_start_hands_back_a_url_and_bad_callbacks_are_rejected() {
    let h = harness().await;
    let started: Value = h.post("/providers/anthropic/oauth").json(&json!({ "mode": "max" })).send().await.unwrap().json().await.unwrap();
    assert!(started["url"].as_str().unwrap().starts_with("https://claude.ai/oauth/authorize?"));
    let state = started["state"].as_str().unwrap();
    assert!(h.engine.oauth.lock().unwrap().contains_key(state));
    let bad = h.post("/providers/anthropic/oauth/callback").json(&json!({ "input": "nonsense" })).send().await.unwrap();
    assert_eq!(bad.status(), 400);
    let unknown = h.post("/providers/anthropic/oauth/callback").json(&json!({ "input": "code#wrongstate" })).send().await.unwrap();
    assert_eq!(unknown.status(), 400);
    assert_eq!(h.post("/providers/openai/oauth").json(&json!({ "mode": "max" })).send().await.unwrap().status(), 404);
    let codex: Value = h.post("/providers/openai/oauth").json(&json!({ "mode": "chatgpt" })).send().await.unwrap().json().await.unwrap();
    assert_eq!(codex["method"], "auto");
    assert!(codex["url"].as_str().unwrap().starts_with("https://auth.openai.com/oauth/authorize?"));
    let missing = h.post("/providers/openai/oauth/callback").json(&json!({})).send().await.unwrap();
    assert_eq!(missing.status(), 400);
}

#[tokio::test]
async fn browser_origins_get_cors_headers_and_preflight_needs_no_token() {
    let h = harness().await;
    let preflight = h
        .http
        .request(reqwest::Method::OPTIONS, h.url("/sessions"))
        .header("origin", "http://localhost:5180")
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "authorization,content-type")
        .send()
        .await
        .unwrap();
    assert_eq!(preflight.status(), 200);
    assert_eq!(preflight.headers()["access-control-allow-origin"], "http://localhost:5180");
    let allowed = h.get("/health").header("origin", "tauri://localhost").send().await.unwrap();
    assert_eq!(allowed.headers()["access-control-allow-origin"], "tauri://localhost");
    let denied = h.get("/health").header("origin", "https://evil.com").send().await.unwrap();
    assert!(denied.headers().get("access-control-allow-origin").is_none());
}

#[tokio::test]
async fn deleting_a_session_removes_it_and_its_messages() {
    let h = harness().await;
    let (_, session_id) = session_with_model(&h).await;
    let mut socket = h.ws("").await;
    assert_eq!(h.http.delete(h.url(&format!("/sessions/{session_id}"))).bearer_auth(&h.engine.token).send().await.unwrap().status(), 204);
    assert_eq!(h.get(&format!("/sessions/{session_id}")).send().await.unwrap().status(), 404);
    assert_eq!(h.http.delete(h.url(&format!("/sessions/{session_id}"))).bearer_auth(&h.engine.token).send().await.unwrap().status(), 404);
    let deleted = until(&mut socket, "session.deleted").await;
    assert_eq!(deleted["sessionId"], session_id);
}

#[tokio::test]
async fn mcp_servers_are_saved_approved_connected_and_their_tools_reach_the_model() {
    use crate::llm::catalog::ToolProfile;
    let h = harness().await;
    let mut socket = h.ws("").await;
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/echo-server.cjs");
    let saved: Value = h.put("/mcp/echo").json(&json!({ "type": "stdio", "command": "node", "args": [script] })).send().await.unwrap().json().await.unwrap();
    assert_eq!(saved["state"], "needs_approval");
    assert_eq!(until(&mut socket, "mcp.updated").await["server"]["name"], "echo");
    let approved: Value = h.post("/mcp/echo/approve").send().await.unwrap().json().await.unwrap();
    assert_eq!(approved["state"], "connected");
    assert_eq!(approved["tools"][0]["name"], "echo");
    let names: Vec<String> = h.engine.tools.specs(ToolProfile::Edit).into_iter().map(|s| s.name).collect();
    assert!(names.contains(&"echo_shout".to_string()));

    let listed: Value = h.get("/mcp").send().await.unwrap().json().await.unwrap();
    assert_eq!(listed[0]["state"], "connected");
    let off: Value = h.put("/mcp/echo/enabled").json(&json!({ "enabled": false })).send().await.unwrap().json().await.unwrap();
    assert_eq!(off["state"], "disabled");
    assert!(!h.engine.tools.specs(ToolProfile::Edit).iter().any(|s| s.name == "echo_shout"));
    assert_eq!(h.http.delete(h.url("/mcp/echo")).bearer_auth(&h.engine.token).send().await.unwrap().status(), 204);
    assert_eq!(h.put("/mcp/bad name").json(&json!({ "type": "stdio", "command": "x" })).send().await.unwrap().status(), 400);
}

#[tokio::test]
async fn workspace_config_and_commands_are_served() {
    use crate::llm::scripted::Scripted;
    use crate::llm::{Chunk, Provider, StopReason};
    let h = harness().await;
    let provider = Scripted::default();
    provider.push(vec![Chunk::TextStart, Chunk::TextDelta("ran".into()), Chunk::BlockStop, Chunk::Stop(StopReason::EndTurn)]);
    *h.engine.turns.provider_override.lock().unwrap() = Some(Provider::Scripted(provider.clone()));
    h.put("/providers/anthropic/key").json(&json!({ "key": "k" })).send().await.unwrap();
    let (ws_id, session_id) = session_with_model(&h).await;
    let ws = h._dir.0.join("ws");
    std::fs::create_dir_all(ws.join(".drift/commands")).unwrap();
    std::fs::write(ws.join(".drift/commands/test.md"), "---\ndescription: Runs tests\n---\nRun tests for $ARGUMENTS").unwrap();

    let config: Value = h.get(&format!("/workspaces/{ws_id}/config")).send().await.unwrap().json().await.unwrap();
    assert_eq!(config["commands"][0]["name"], "test");
    assert_eq!(config["agents"].as_array().unwrap().len(), 2);

    let ran = h.post(&format!("/sessions/{session_id}/command")).json(&json!({ "name": "test", "arguments": "the parser" })).send().await.unwrap();
    assert_eq!(ran.status(), 202);
    for _ in 0..100 {
        if !h.engine.turns.is_running(&session_id) { break }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let sent = provider.requests.lock().unwrap()[0].messages[0].blocks.clone();
    assert_eq!(sent, vec![crate::llm::Block::Text("Run tests for the parser".into())]);
    assert_eq!(h.post(&format!("/sessions/{session_id}/command")).json(&json!({ "name": "nope" })).send().await.unwrap().status(), 404);
    let patched: Value = h.patch(&format!("/sessions/{session_id}")).json(&json!({ "agent": "plan" })).send().await.unwrap().json().await.unwrap();
    assert_eq!(patched["agent"], "plan");
}

