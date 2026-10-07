use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

use crate::event::Event;
use crate::mcp::ServerConfig;
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

    fn delete(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.delete(self.url(path)).bearer_auth(&self.engine.token)
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
async fn tools_lists_every_builtin_an_agent_can_name_once() {
    let h = harness().await;
    let body: Value = h.get("/tools").send().await.unwrap().json().await.unwrap();
    let names: Vec<&str> = body.as_array().unwrap().iter().map(|tool| tool["name"].as_str().unwrap()).collect();
    for expected in ["read", "edit", "write", "apply_patch", "bash", "grep", "glob", "task"] {
        assert_eq!(names.iter().filter(|name| **name == expected).count(), 1, "{expected} in {names:?}");
    }
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

#[tokio::test]
async fn a_question_reply_on_the_socket_hears_how_it_ended() {
    let h = harness().await;
    let mut socket = h.ws("").await;
    next_json(&mut socket).await;
    let reply = json!({ "type": "question.reply", "requestId": "que_gone", "answers": [["yes"]] });
    socket.send(Message::Text(reply.to_string().into())).await.unwrap();
    let result = until(&mut socket, "question.result").await;
    assert_eq!(result["requestId"], "que_gone");
    assert_eq!(result["ok"], false);
    assert_eq!(result["error"]["code"], "not_found");
    assert!(result.get("seq").is_none(), "a reply's result is not an event");
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
        .json(&json!({ "workspaceId": ws_id, "title": "Test", "model": { "provider": "anthropic", "model": "claude-sonnet-4-5" } }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    (ws_id, created["id"].as_str().unwrap().to_string())
}

#[tokio::test]
async fn a_prompt_carrying_a_screenshot_past_axums_default_limit_is_admitted() {
    let h = harness().await;
    assert_eq!(h.put("/providers/anthropic/key").json(&json!({ "key": "sk-test" })).send().await.unwrap().status(), 204);
    let (_, session_id) = session_with_model(&h).await;
    // A 2.5 MB screenshot is 3.4 MB once encoded; axum's own 2 MB default refused it mid-upload.
    use base64::Engine as _;
    let data = format!("data:text/plain;base64,{}", base64::engine::general_purpose::STANDARD.encode(vec![b'x'; 2_525_283]));
    let prompt = json!({ "parts": [{ "type": "text", "text": "look" }, { "type": "file", "mime": "text/plain", "name": "shot.txt", "url": data }] });
    let status = h.post(&format!("/sessions/{session_id}/turns")).json(&prompt).send().await.unwrap().status();
    assert_eq!(status, 202);
    let too_big = vec![b' '; super::MAX_REQUEST_BYTES + 1];
    let status = h.post(&format!("/sessions/{session_id}/turns")).header("content-type", "application/json").body(too_big).send().await.map(|response| response.status());
    assert!(status.is_err() || status.unwrap() == 413, "past the limit it is still refused");
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
    // Workspace writes run by default; this test is about the ask, so a rule asks for it.
    h.engine.permissions.set_policy(crate::permission::Policy { rules: vec![crate::permission::Rule { kind: "edit".into(), pattern: "*".into(), decision: crate::permission::Decision::Ask }] });
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
    // A prompt sent while the turn runs is taken by it, not refused.
    let steered = h.post(&format!("/sessions/{session_id}/turns")).json(&json!({ "parts": [{ "type": "text", "text": "and say so" }] })).send().await.unwrap().status();
    assert_eq!(steered, 202);

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
    assert_eq!(messages.len(), 4, "both prompts, the call and the reply: one turn answered both");
    assert_eq!(messages.iter().filter(|m| m["role"] == "user").count(), 2);
    let call = messages.iter().find(|m| m["parts"][0]["type"] == "tool_call").unwrap();
    assert_eq!(call["parts"][0]["status"], "done");
    assert_eq!(messages[3]["parts"][0]["text"], "Wrote it");
    let listed: Value = h.get("/sessions").send().await.unwrap().json().await.unwrap();
    assert_eq!(listed[0]["id"], session_id.as_str());
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
async fn the_archive_purge_never_deletes_a_session_that_was_restored() {
    let h = harness().await;
    let (_, session_id) = session_with_model(&h).await;
    let purge = || h.delete(&format!("/sessions/{session_id}?archived=true")).send();
    assert_eq!(purge().await.unwrap().status(), 409, "never archived");
    let archive = |archived: bool| h.patch(&format!("/sessions/{session_id}")).json(&json!({ "archived": archived })).send();
    archive(true).await.unwrap();
    archive(false).await.unwrap();
    let refused: Value = purge().await.unwrap().json().await.unwrap();
    assert_eq!(refused["code"], "active", "restored, so kept");
    assert!(h.engine.store.session(&session_id).unwrap().is_some());
    archive(true).await.unwrap();
    assert_eq!(purge().await.unwrap().status(), 204);
    assert_eq!(purge().await.unwrap().status(), 404);
}

#[tokio::test]
async fn a_removed_workspaces_purge_deletes_its_conversations_and_history_and_nothing_in_use() {
    let h = harness().await;
    let (ws_id, session_id) = session_with_model(&h).await;
    let archived = h.engine.store.create_session(crate::store::NewSession { workspace_id: &ws_id, parent_id: None, visibility: crate::session::types::Visibility::Sibling, title: "old", agent: "build", model: None }).unwrap();
    h.engine.store.set_session_archived(&archived.id, true).unwrap();
    let history = h._dir.0.join("snapshots").join(format!("ws-{ws_id}"));
    std::fs::create_dir_all(&history).unwrap();
    let purge = || h.post(&format!("/workspaces/{ws_id}/purge")).send();
    let refused: Value = purge().await.unwrap().json().await.unwrap();
    assert_eq!(refused["code"], "in_use", "a workspace on the sidebar keeps everything");
    assert!(h.engine.store.session(&session_id).unwrap().is_some());
    h.engine.store.lock().execute("UPDATE workspace SET removed_at = 1 WHERE id = ?1", [&ws_id]).unwrap();
    let mut socket = h.ws("").await;
    let purged: Value = purge().await.unwrap().json().await.unwrap();
    assert_eq!(purged["deleted"], 2, "archived ones too");
    assert!(h.engine.store.session(&session_id).unwrap().is_none() && h.engine.store.session(&archived.id).unwrap().is_none());
    assert!(!history.exists(), "its undo history goes with them");
    assert!(until(&mut socket, "session.deleted").await["sessionId"].as_str().is_some(), "the sidebar hears each one go");
    assert_eq!(purge().await.unwrap().status(), 200, "a repeat finds nothing left and still succeeds");
    assert_eq!(h.post("/workspaces/nobody/purge").send().await.unwrap().status(), 404);
}

#[tokio::test]
async fn deleting_a_session_removes_it_and_its_messages() {
    let h = harness().await;
    let (_, session_id) = session_with_model(&h).await;
    let mut socket = h.ws("").await;
    assert_eq!(h.delete(&format!("/sessions/{session_id}")).send().await.unwrap().status(), 204);
    assert_eq!(h.get(&format!("/sessions/{session_id}")).send().await.unwrap().status(), 404);
    assert_eq!(h.delete(&format!("/sessions/{session_id}")).send().await.unwrap().status(), 404);
    let deleted = until(&mut socket, "session.deleted").await;
    assert_eq!(deleted["sessionId"], session_id);
}

#[tokio::test]
async fn mcp_servers_connect_as_soon_as_they_are_saved_and_their_tools_reach_the_model() {
    use crate::llm::catalog::ToolProfile;
    let h = harness().await;
    let mut socket = h.ws("").await;
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/echo-server.cjs");
    let dir = h._dir.0.join("ws");
    std::fs::create_dir_all(&dir).unwrap();
    let ws = h.engine.store.add_workspace(&dir.to_string_lossy(), "ws", "").unwrap();
    let here = crate::tool::canonical(&dir);
    let saved: Value = h.put(&format!("/mcp/echo?workspace={}", ws.id)).json(&json!({ "type": "stdio", "command": "node", "args": [script] })).send().await.unwrap().json().await.unwrap();
    assert_eq!(saved["state"], "connected", "nothing to approve: {}", saved["error"]);
    assert_eq!(saved["tools"][0]["name"], "echo");
    assert!(saved.get("hash").is_none() && saved.get("approved").is_none());
    assert_eq!(until(&mut socket, "mcp.updated").await["server"]["name"], "echo");
    let names: Vec<String> = h.engine.tool_specs(ToolProfile::Edit, Some(&here)).into_iter().map(|s| s.name).collect();
    assert!(names.contains(&"echo_shout".to_string()));
    assert!(!h.engine.tool_specs(ToolProfile::Edit, None).iter().any(|s| s.name == "echo_shout"), "a stdio server serves only the workspace it runs in");
    h.engine.mcp.disconnect("echo", &h.engine.store, &h.engine.hub).await;
    let back: Value = h.post("/mcp/echo/connect").send().await.unwrap().json().await.unwrap();
    assert_eq!(back["state"], "connected", "with no workspace named, it connects again where it ran");
    h.put("/mcp/fresh").json(&json!({ "type": "stdio", "command": "node", "args": [script] })).send().await.unwrap();
    assert_eq!(h.post("/mcp/fresh/connect").send().await.unwrap().status(), 409, "a stdio server running nowhere needs a workspace");
    h.delete("/mcp/fresh").send().await.unwrap();

    let listed: Value = h.get("/mcp").send().await.unwrap().json().await.unwrap();
    assert_eq!(listed[0]["state"], "connected");
    let off: Value = h.put("/mcp/echo/enabled").json(&json!({ "enabled": false })).send().await.unwrap().json().await.unwrap();
    assert_eq!(off["state"], "disabled");
    assert!(!h.engine.tool_specs(ToolProfile::Edit, Some(&here)).iter().any(|s| s.name == "echo_shout"));
    assert_eq!(h.delete("/mcp/echo").send().await.unwrap().status(), 204);
    assert_eq!(h.put("/mcp/bad name").json(&json!({ "type": "stdio", "command": "x" })).send().await.unwrap().status(), 400);
}

#[tokio::test]
async fn mcp_secrets_go_in_but_never_out_and_no_save_or_rename_replaces_another_server() {
    let h = harness().await;
    let save = |name: &str, query: &str, body: Value| h.put(&format!("/mcp/{name}{query}")).json(&body).send();
    // Saving connects, so every server here points at a closed local port or a missing program, and fails at once.
    let secret = json!({ "type": "http", "url": "http://127.0.0.1:9/mcp", "headers": { "Authorization": "Bearer secret-token" } });
    let saved = save("docs", "?create=true", secret.clone()).await.unwrap().text().await.unwrap();
    assert!(saved.contains("Authorization") && !saved.contains("secret-token"), "{saved}");
    let listed = h.get("/mcp").send().await.unwrap().text().await.unwrap();
    assert!(!listed.contains("secret-token"));
    assert_eq!(save("docs", "?create=true", secret.clone()).await.unwrap().status(), 409, "adding never replaces");

    let kept = json!({ "type": "http", "url": "http://127.0.0.1:9/v2", "headers": { "Authorization": null } });
    assert_eq!(save("docs", "", kept).await.unwrap().status(), 200);
    let ServerConfig::Http { headers, .. } = h.engine.store.mcp_server("docs").unwrap().unwrap().config else { panic!() };
    assert_eq!(headers["Authorization"], "Bearer secret-token", "a null value keeps the saved secret");
    let unknown = json!({ "type": "http", "url": "http://127.0.0.1:9", "headers": { "X-Key": null } });
    let refused: Value = save("docs", "", unknown).await.unwrap().json().await.unwrap();
    assert_eq!(refused["code"], "secret");

    save("other", "", json!({ "type": "stdio", "command": "definitely-not-a-program" })).await.unwrap();
    let rename = |from: &str, to: &str| h.post(&format!("/mcp/{from}/rename")).json(&json!({ "to": to })).send();
    let onto_taken: Value = rename("docs", "other").await.unwrap().json().await.unwrap();
    assert_eq!(onto_taken["code"], "taken");
    assert!(matches!(h.engine.store.mcp_server("other").unwrap().unwrap().config, ServerConfig::Stdio { .. }), "the other server is untouched");
    assert_eq!(rename("docs", "docs2").await.unwrap().status(), 200);
    assert!(h.engine.store.mcp_server("docs").unwrap().is_none());
    let ServerConfig::Http { headers, .. } = h.engine.store.mcp_server("docs2").unwrap().unwrap().config else { panic!() };
    assert_eq!(headers["Authorization"], "Bearer secret-token", "the secret moves with it");
}

#[tokio::test]
async fn a_workspaces_kept_grants_are_listed_and_revoked() {
    let h = harness().await;
    let (ws_id, session_id) = session_with_model(&h).await;
    h.engine.bind_permissions(&session_id, &ws_id);
    let request = crate::permission::new_request(&session_id, "m", "c", "bash", crate::tool::Ask::shell(crate::tool::command::Dialect::Bash, "cargo build", "cargo build"));
    let asking = { let engine = h.engine.clone(); tokio::spawn(async move { engine.permissions.check(&engine.hub, &crate::permission::Policy::default(), request, &Default::default()).await }) };
    while h.engine.permissions.pending().is_empty() { tokio::time::sleep(std::time::Duration::from_millis(5)).await; }
    let id = h.engine.permissions.pending()[0].id.clone();
    h.post(&format!("/permissions/{id}/reply")).json(&json!({ "reply": "always" })).send().await.unwrap();
    assert_eq!(asking.await.unwrap(), crate::permission::Outcome::Allowed);
    let listed: Value = h.get(&format!("/workspaces/{ws_id}/permission-grants")).send().await.unwrap().json().await.unwrap();
    assert_eq!(listed, json!([{ "grant": "subcommand", "prefix": "cargo build" }]));
    assert_eq!(h.post(&format!("/workspaces/{ws_id}/permission-grants/revoke")).json(&listed[0]).send().await.unwrap().status(), 204);
    assert_eq!(h.post(&format!("/workspaces/{ws_id}/permission-grants/revoke")).json(&listed[0]).send().await.unwrap().status(), 404);
    assert_eq!(h.delete(&format!("/workspaces/{ws_id}/permission-grants")).send().await.unwrap().status(), 204);
    assert_eq!(h.get("/workspaces/nope/permission-grants").send().await.unwrap().status(), 404);
    assert_eq!(h.delete("/workspaces/nope/permission-grants").send().await.unwrap().status(), 404, "the same for every route");
    assert_eq!(h.post("/workspaces/nope/permission-grants/revoke").json(&listed[0]).send().await.unwrap().status(), 404);

    let grant = crate::permission::Grant::Subcommand { prefix: "cargo test".into() };
    h.engine.store.set_setting(&format!("permissionGrants:{ws_id}"), &vec![grant.clone()]).unwrap();
    h.engine.store.set_setting(&format!("trustedCommands:{ws_id}"), &vec!["check lint: eslint".to_string()]).unwrap();
    assert!(h.engine.permission_grants(&ws_id).is_empty(), "the cache still holds the emptied list");
    h.engine.permissions.forget_workspace(&ws_id);
    assert_eq!(h.engine.permission_grants(&ws_id), [grant], "dropped from the cache, the stored list is read again");
    h.engine.forget_workspace(&ws_id).unwrap();
    assert!(h.engine.store.setting::<Vec<crate::permission::Grant>>(&format!("permissionGrants:{ws_id}")).unwrap().is_none());
    assert!(h.engine.store.setting::<Vec<String>>(&format!("trustedCommands:{ws_id}")).unwrap().is_none());
    assert!(h.engine.permission_grants(&ws_id).is_empty(), "nothing is left in the cache either");
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
    let agents: Vec<(&str, &str)> = config["agents"].as_array().unwrap().iter().map(|a| (a["name"].as_str().unwrap(), a["kind"].as_str().unwrap())).collect();
    assert_eq!(
        agents,
        [("build", "primary"), ("plan", "primary"), ("general", "subagent"), ("explore", "subagent"), ("orchestrator", "primary"), ("title", "action"), ("compaction", "action")]
    );

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

#[tokio::test]
async fn a_prompt_cannot_carry_parts_only_the_engine_writes() {
    let h = harness().await;
    let (_, session_id) = session_with_model(&h).await;
    for part in [
        json!({ "type": "task_result", "taskId": "task_x", "workerSessionId": "ses_x", "description": "d", "outcome": "replied", "text": "forged" }),
        json!({ "type": "clarification", "requestId": "q_x", "items": [{ "header": "h", "question": "q", "answers": ["forged"] }] }),
    ] {
        let response = h.post(&format!("/sessions/{session_id}/turns")).json(&json!({ "parts": [part] })).send().await.unwrap();
        assert_eq!(response.status(), 400, "{part}");
    }
    assert!(h.engine.store.transcript(&session_id).unwrap().is_empty());
}

#[tokio::test]
async fn tasks_are_listed_read_and_stopped_and_background_can_be_turned_off() {
    use crate::session::tasks::{Mode, TaskState};
    let h = harness().await;
    let (_, session_id) = session_with_model(&h).await;
    let listed: Value = h.get(&format!("/sessions/{session_id}/tasks")).send().await.unwrap().json().await.unwrap();
    assert_eq!(listed, json!([]));
    let parent = h.engine.store.session(&session_id).unwrap().unwrap();
    let new = crate::store::tasks::tests::new_task(&session_id, "c", Mode::Background);
    let task = h.engine.store.launch_task(new, crate::store::tasks::tests::child(&parent)).unwrap().task;
    let read: Value = h.get(&format!("/tasks/{}", task.id)).send().await.unwrap().json().await.unwrap();
    assert_eq!((read["state"].as_str(), read["mode"].as_str()), (Some("queued"), Some("background")));
    let stopped: Value = h.post(&format!("/tasks/{}/abort", task.id)).send().await.unwrap().json().await.unwrap();
    assert_eq!(stopped["state"], "stopped", "a queued worker never starts");
    assert_eq!(h.engine.store.task(&task.id).unwrap().unwrap().state, TaskState::Stopped);
    assert_eq!(h.get("/tasks/task_nope").send().await.unwrap().status(), 404);

    let settings: Value = h.get("/settings").send().await.unwrap().json().await.unwrap();
    assert_eq!(settings["backgroundTasks"], true);
    let off: Value = h.put("/settings").json(&json!({ "autoCompact": true, "backgroundTasks": false })).send().await.unwrap().json().await.unwrap();
    assert_eq!(off["backgroundTasks"], false);
    let kept: Value = h.put("/settings").json(&json!({ "autoCompact": false })).send().await.unwrap().json().await.unwrap();
    assert_eq!((kept["autoCompact"].as_bool(), kept["backgroundTasks"].as_bool()), (Some(false), Some(false)), "left out, it stays");
}


#[tokio::test]
async fn base_prompts_are_replaced_for_every_model_or_one_family_and_the_shared_rules_stay() {
    use crate::llm::scripted::Scripted;
    use crate::llm::{Chunk, Provider, StopReason};
    let h = harness().await;
    let provider = Scripted::default();
    let reply = || vec![Chunk::TextStart, Chunk::TextDelta("ok".into()), Chunk::BlockStop, Chunk::Stop(StopReason::EndTurn)];
    provider.push(reply()).push(reply()).push(reply());
    *h.engine.turns.provider_override.lock().unwrap() = Some(Provider::Scripted(provider.clone()));
    h.put("/providers/anthropic/key").json(&json!({ "key": "k" })).send().await.unwrap();
    let (_, session_id) = session_with_model(&h).await;
    let listed: Value = h.get("/prompts").send().await.unwrap().json().await.unwrap();
    let ids: Vec<&str> = listed["prompts"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["all", "codex", "claude", "gemini", "default"]);
    assert!(listed["prompts"][2]["default"].as_str().unwrap().starts_with("You are Drift") && listed["shared"].as_str().unwrap().contains("<system-reminder>"));
    let turn = |text: &'static str| {
        let h = &h;
        let session_id = session_id.clone();
        async move {
            h.post(&format!("/sessions/{session_id}/turns")).json(&json!({ "parts": [{ "type": "text", "text": text }] })).send().await.unwrap();
            for _ in 0..200 {
                if !h.engine.turns.is_running(&session_id) { break }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
    };
    let system = |n: usize| provider.requests.lock().unwrap()[n].system.clone();
    assert_eq!(h.put("/prompts/all").json(&json!({ "text": "Every model works my way." })).send().await.unwrap().status(), 200);
    turn("one").await;
    assert!(system(0).starts_with("Every model works my way.\n\n# Tools") && system(0).contains("<system-reminder>"), "{}", system(0));
    h.put("/prompts/claude").json(&json!({ "text": "Claude works this way." })).send().await.unwrap();
    turn("two").await;
    assert!(system(1).starts_with("Claude works this way."), "a family's own wins over the one for every model");
    let reset: Value = h.delete("/prompts/claude").send().await.unwrap().json().await.unwrap();
    assert!(reset["prompts"][2].get("custom").is_none());
    h.delete("/prompts/all").send().await.unwrap();
    turn("three").await;
    assert!(system(2).starts_with("You are Drift"), "reset, Drift's own is back");
    assert_eq!(h.put("/prompts/claude").json(&json!({ "text": "  " })).send().await.unwrap().status(), 400);
    assert_eq!(h.put("/prompts/nope").json(&json!({ "text": "x" })).send().await.unwrap().status(), 404);
}
#[tokio::test]
async fn settings_rules_apply_at_once_survive_a_restart_and_refuse_what_could_never_match() {
    let h = harness().await;
    assert_eq!(h.get("/permission-rules").send().await.unwrap().json::<Value>().await.unwrap(), json!([]));
    let rules = json!([{ "kind": "bash", "pattern": "git push*", "decision": "deny" }, { "kind": "webfetch", "pattern": "*", "decision": "ask" }]);
    let saved: Value = h.put("/permission-rules").json(&rules).send().await.unwrap().json().await.unwrap();
    assert_eq!(saved, rules);
    let push = crate::tool::Ask::new("bash", "git push origin", "");
    assert_eq!(h.engine.permissions.decide_now("s", &crate::permission::Policy::default(), &push), crate::permission::Decision::Deny, "the next call follows them");
    let reopened = Engine::open_with(&h._dir.0, crate::Options { file_credentials: true, ..Default::default() }).unwrap();
    assert_eq!(serde_json::to_value(reopened.permission_rules()).unwrap(), rules, "kept across a restart");
    for bad in [json!([{ "kind": "Bash!", "pattern": "*", "decision": "deny" }]), json!([{ "kind": "bash", "pattern": " ", "decision": "deny" }]), json!([{ "kind": "read", "pattern": "a[", "decision": "deny" }])] {
        assert_eq!(h.put("/permission-rules").json(&bad).send().await.unwrap().status(), 400, "{bad}");
    }
    assert_eq!(h.get("/permission-rules").send().await.unwrap().json::<Value>().await.unwrap(), rules, "a refused save changes nothing");
}
#[tokio::test]
async fn a_socket_a_host_leases_closes_when_the_lease_is_cancelled() {
    let h = harness().await;
    let lease = tokio_util::sync::CancellationToken::new();
    let held = lease.clone();
    let router = super::router(h.engine.clone()).layer(axum::middleware::from_fn(move |mut request: axum::extract::Request, next: axum::middleware::Next| {
        request.extensions_mut().insert(super::Lease(held.clone()));
        next.run(request)
    }));
    let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/events?token={}", h.engine.token)).await.unwrap();
    assert!(matches!(socket.next().await, Some(Ok(Message::Text(_)))), "hello arrives while the lease holds");
    lease.cancel();
    let ended = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match socket.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "the socket closed once the host took the lease back");
}
#[tokio::test]
async fn a_workspace_a_client_has_open_keeps_its_mcp_servers_until_its_socket_closes() {
    use futures_util::SinkExt;
    let h = harness().await;
    let dir = h._dir.0.join("open-ws");
    std::fs::create_dir_all(&dir).unwrap();
    let here = crate::tool::canonical(&dir);
    let mut socket = h.ws("").await;
    socket.send(Message::Text(json!({ "type": "workspace.open", "directory": dir }).to_string().into())).await.unwrap();
    for _ in 0..100 {
        if h.engine.mcp.is_open(&here) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(h.engine.mcp.is_open(&here), "the socket's workspace is open");
    socket.send(Message::Close(None)).await.unwrap();
    drop(socket);
    for _ in 0..100 {
        if !h.engine.mcp.is_open(&here) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(!h.engine.mcp.is_open(&here), "and no longer once the socket closes");
}