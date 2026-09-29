use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::*;
use crate::event::Event;
use crate::llm::scripted::Scripted;
use crate::llm::{Chunk, Provider};
use crate::permission::{Decision, Policy, Reply, ReplyBody, Rule};
use crate::session::types::Visibility;
use crate::store::NewSession;

struct Harness {
    engine: Arc<Engine>,
    session: Session,
    provider: Scripted,
    _dir: PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self._dir);
    }
}

async fn harness() -> Harness {
    let dir = std::env::temp_dir().join(format!("drift-turn-{}", crate::random_hex(4)));
    let workspace = dir.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let engine = Engine::open_with(&dir.join("data"), crate::Options { file_credentials: true, ..Default::default() }).unwrap();
    let provider = Scripted::default();
    *engine.turns.provider_override.lock().unwrap() = Some(Provider::Scripted(provider.clone()));
    engine.credentials.set("anthropic", &Credential::ApiKey { key: "k".into() }).unwrap();
    let ws = engine.store.add_workspace(&workspace.to_string_lossy(), "ws", "").unwrap();
    let session = engine
        .store
        .create_session(NewSession { workspace_id: &ws.id, parent_id: None, visibility: Visibility::Sibling, title: "", agent: "build", model: None })
        .unwrap();
    Harness { engine, session, provider, _dir: dir }
}

fn model() -> ModelRef {
    ModelRef { provider: "anthropic".into(), model: "claude-sonnet-4-5".into() }
}

fn text(text: &str) -> Vec<Chunk> {
    vec![Chunk::Usage(Usage { input: 10, ..Usage::default() }), Chunk::TextStart, Chunk::TextDelta(text.into()), Chunk::BlockStop, Chunk::Usage(Usage { output: 3, ..Usage::default() }), Chunk::Stop(StopReason::EndTurn)]
}

fn tool_call(name: &str, input: &str) -> Vec<Chunk> {
    vec![
        Chunk::ToolUseStart { id: format!("toolu_{name}"), name: name.into() },
        Chunk::ToolInputDelta(input.into()),
        Chunk::BlockStop,
        Chunk::Stop(StopReason::ToolUse),
    ]
}

async fn until_idle(h: &Harness) {
    for _ in 0..200 {
        if !h.engine.turns.is_running(&h.session.id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("turn never finished");
}

fn prompt(text: &str) -> Prompt {
    Prompt { parts: vec![Part::Text { text: text.into() }], model: Some(model()), thinking_budget: None }
}

#[tokio::test]
async fn a_plain_reply_is_stored_costed_and_titles_the_session() {
    let h = harness().await;
    h.provider.push(text("Hello there"));
    let receipt = h.engine.submit(&h.session.id, prompt("say hello please")).await.await_ok();
    assert_eq!(receipt.message.role, Role::User);
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert_eq!(transcript.len(), 2);
    let reply = &transcript[1];
    assert_eq!(reply.info.status, MessageStatus::Done);
    assert_eq!(reply.info.usage, Usage { input: 10, output: 3, cache_read: 0, cache_write: 0 });
    assert!(reply.info.cost > 0.0);
    assert_eq!(reply.parts[0].part, Part::Text { text: "Hello there".into() });
    let session = h.engine.store.session(&h.session.id).unwrap().unwrap();
    assert_eq!(session.title, "say hello please");
    assert_eq!(session.model, Some(model()));
    let request = &h.provider.requests.lock().unwrap()[0];
    assert!(request.system.starts_with("You are Drift"));
    assert_eq!(request.tools.len(), 6);
}

#[tokio::test]
async fn tool_calls_run_and_feed_the_next_request() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "alpha\n").unwrap();
    h.provider.push(tool_call("read", r#"{"path": "a.txt"}"#)).push(text("It says alpha"));
    h.engine.submit(&h.session.id, prompt("what is in a.txt")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert_eq!(transcript.len(), 3);
    let Part::ToolCall { status, output, title, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Done);
    assert_eq!(output.as_deref(), Some("1: alpha"));
    assert_eq!(title.as_deref(), Some("a.txt"));
    assert_eq!(transcript[2].parts[0].part, Part::Text { text: "It says alpha".into() });
    let requests = h.provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(matches!(&requests[1].messages[2].blocks[0], llm::Block::ToolResult { content, .. } if content == "1: alpha"));
}

#[tokio::test]
async fn permission_denial_is_reported_to_the_model() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Deny }] });
    h.provider.push(tool_call("bash", r#"{"command": "rm -rf /"}"#)).push(text("Understood"));
    h.engine.submit(&h.session.id, prompt("wipe it")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Denied);
    let requests = h.provider.requests.lock().unwrap();
    assert!(matches!(&requests[1].messages[2].blocks[0], llm::Block::ToolResult { is_error: true, content, .. } if content.contains("denied")));
}

#[tokio::test]
async fn asks_wait_for_a_reply_and_mutations_snapshot_first() {
    let h = harness().await;
    let mut rx = h.engine.hub.attach(None).rx;
    h.provider.push(tool_call("write", r#"{"path": "new.txt", "content": "hi\n"}"#)).push(text("Written"));
    h.engine.submit(&h.session.id, prompt("make new.txt")).await.await_ok();
    let request = loop {
        let envelope = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
        if let Event::PermissionAsked { request } = envelope.event {
            break request;
        }
    };
    assert_eq!(request.tool, "write");
    assert_eq!(request.ask.kind, "edit");
    h.engine.permissions.reply(&h.engine.hub, &request.id, ReplyBody { reply: Reply::Once, pattern: None }).unwrap();
    until_idle(&h).await;
    assert_eq!(std::fs::read_to_string(h._dir.join("ws/new.txt")).unwrap(), "hi\n");
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, metadata, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Done);
    assert!(metadata.as_ref().unwrap()["snapshot"].is_string(), "{metadata:?}");
}

#[tokio::test]
async fn abort_marks_the_message_and_frees_the_session() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Allow }] });
    let sleep = if cfg!(windows) { "ping -n 10 127.0.0.1 > nul" } else { "sleep 10" };
    h.provider.push(tool_call("bash", &json!({ "command": sleep }).to_string()));
    h.engine.submit(&h.session.id, prompt("wait")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(h.engine.submit(&h.session.id, prompt("again")).await.err(), Some(TurnError::Busy));
    assert!(h.engine.abort(&h.session.id));
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Error);
    assert!(!h.engine.abort(&h.session.id));
}

#[tokio::test]
async fn retryable_provider_errors_are_retried_and_others_are_not() {
    let h = harness().await;
    h.provider
        .push_error(llm::Error::Api { status: 529, kind: "overloaded".into(), message: "busy".into(), retryable: true })
        .push(text("second time lucky"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert_eq!(transcript.len(), 3);
    assert_eq!(transcript[1].info.status, MessageStatus::Error);
    assert_eq!(transcript[2].info.status, MessageStatus::Done);

    h.provider.push_error(llm::Error::Unauthenticated);
    h.engine.submit(&h.session.id, prompt("again")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert_eq!(transcript.len(), 5);
    assert_eq!(transcript[4].info.status, MessageStatus::Error);
    assert_eq!(transcript[4].info.error.as_deref(), Some("no credentials for this provider"));
}

#[tokio::test]
async fn submit_rejects_bad_plans() {
    let h = harness().await;
    let no_model = Prompt { parts: vec![], model: None, thinking_budget: None };
    assert_eq!(h.engine.submit(&h.session.id, no_model).await.err(), Some(TurnError::NoModel));
    let unknown = Prompt { model: Some(ModelRef { provider: "anthropic".into(), model: "nope".into() }), ..prompt("x") };
    assert_eq!(h.engine.submit(&h.session.id, unknown).await.err(), Some(TurnError::UnknownModel));
    assert_eq!(h.engine.submit("ses_missing", prompt("x")).await.err(), Some(TurnError::NoSession));
    h.engine.credentials.remove("anthropic").unwrap();
    assert_eq!(h.engine.submit(&h.session.id, prompt("x")).await.err(), Some(TurnError::NoCredentials));
}

trait AwaitOk {
    fn await_ok(self) -> Receipt;
}

impl AwaitOk for Result<Receipt, TurnError> {
    fn await_ok(self) -> Receipt {
        self.unwrap_or_else(|error| panic!("submit failed: {error}"))
    }
}
