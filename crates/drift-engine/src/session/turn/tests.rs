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

pub(crate) struct Harness {
    pub(crate) engine: Arc<Engine>,
    pub(crate) session: Session,
    pub(crate) provider: Scripted,
    pub(crate) _dir: PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self._dir);
    }
}

pub(crate) async fn harness() -> Harness {
    let _ = rustls::crypto::ring::default_provider().install_default();
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
        .create_session(NewSession { workspace_id: &ws.id, parent_id: None, visibility: Visibility::Sibling, title: "Test", agent: "build", model: None })
        .unwrap();
    Harness { engine, session, provider, _dir: dir }
}

pub(crate) fn model() -> ModelRef {
    ModelRef { provider: "anthropic".into(), model: "claude-sonnet-4-5".into() }
}

pub(crate) fn text(text: &str) -> Vec<Chunk> {
    vec![Chunk::Usage(Usage { input: 10, ..Usage::default() }), Chunk::TextStart, Chunk::TextDelta(text.into()), Chunk::BlockStop, Chunk::Usage(Usage { output: 3, ..Usage::default() }), Chunk::Stop(StopReason::EndTurn)]
}

pub(crate) fn tool_call(name: &str, input: &str) -> Vec<Chunk> {
    vec![
        Chunk::ToolUseStart { id: format!("toolu_{name}"), name: name.into() },
        Chunk::ToolInputDelta(input.into()),
        Chunk::BlockStop,
        Chunk::Stop(StopReason::ToolUse),
    ]
}

pub(crate) async fn until_idle(h: &Harness) {
    for _ in 0..200 {
        if !h.engine.turns.is_running(&h.session.id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("turn never finished");
}

pub(crate) fn prompt(text: &str) -> Prompt {
    Prompt { parts: vec![Part::Text { text: text.into() }], model: Some(model()), thinking_budget: None, submission_id: None }
}

#[tokio::test]
async fn a_plain_reply_is_stored_and_costed() {
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
    assert_eq!(session.model, Some(model()));
    let request = &h.provider.requests.lock().unwrap()[0];
    assert!(request.system.starts_with("You are Drift"));
    assert_eq!(request.tools.len(), 12);
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
    let no_model = Prompt { parts: vec![], model: None, thinking_budget: None, submission_id: None };
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

#[tokio::test]
async fn calls_keep_the_models_order_across_a_write() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "edit".into(), pattern: "*".into(), decision: Decision::Allow }] });
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    h.provider
        .push(vec![
            Chunk::ToolUseStart { id: "t1".into(), name: "read".into() },
            Chunk::ToolInputDelta(r#"{"path": "a.txt"}"#.into()),
            Chunk::BlockStop,
            Chunk::ToolUseStart { id: "t2".into(), name: "write".into() },
            Chunk::ToolInputDelta(r#"{"path": "c.txt", "content": "c\n"}"#.into()),
            Chunk::BlockStop,
            Chunk::ToolUseStart { id: "t3".into(), name: "read".into() },
            Chunk::ToolInputDelta(r#"{"path": "c.txt"}"#.into()),
            Chunk::BlockStop,
            Chunk::Stop(StopReason::ToolUse),
        ])
        .push(text("done"));
    h.engine.submit(&h.session.id, prompt("go")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let outputs: Vec<(String, ToolStatus, String)> = transcript[1]
        .parts
        .iter()
        .filter_map(|row| match &row.part {
            Part::ToolCall { name, status, output, .. } => Some((name.clone(), *status, output.clone().unwrap_or_default())),
            _ => None,
        })
        .collect();
    assert_eq!(outputs[0].0, "read");
    assert_eq!(outputs[2], ("read".into(), ToolStatus::Done, "1: c".into()), "a read issued after a write must see the write");
}
#[tokio::test]
async fn a_stream_that_ends_without_a_stop_reason_runs_no_tools() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "alpha\n").unwrap();
    let truncated = vec![Chunk::ToolUseStart { id: "t1".into(), name: "read".into() }, Chunk::ToolInputDelta(r#"{"path": "a.txt"}"#.into()), Chunk::BlockStop];
    h.provider.push(truncated.clone()).push(truncated.clone()).push(truncated);
    h.engine.submit(&h.session.id, prompt("read a")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    for message in transcript.iter().skip(1) {
        assert_eq!(message.info.status, MessageStatus::Error);
        let Part::ToolCall { status, .. } = &message.parts[0].part else { panic!() };
        assert_eq!(*status, ToolStatus::Pending, "the call must never run");
    }
    assert_eq!(transcript.len(), 1 + MAX_ATTEMPTS as usize);
}

#[tokio::test]
async fn failed_admission_releases_the_session_and_submission_ids_replay() {
    let h = harness().await;
    h.provider.push(text("ok")).push(text("again"));
    let mut first = prompt("hello");
    first.submission_id = Some("sub_1".into());
    let receipt = h.engine.submit(&h.session.id, first.clone()).await.await_ok();
    let replay = h.engine.submit(&h.session.id, first).await.await_ok();
    assert_eq!(replay.message.id, receipt.message.id, "same submission id returns the same receipt");
    until_idle(&h).await;
    assert_eq!(h.engine.store.transcript(&h.session.id).unwrap().len(), 2);

    let other = h.engine.store.create_session(NewSession { workspace_id: &h.session.workspace_id, parent_id: None, visibility: Visibility::Sibling, title: "", agent: "build", model: None }).unwrap();
    let mut reused = prompt("x");
    reused.submission_id = Some("sub_1".into());
    assert_eq!(h.engine.submit(&other.id, reused).await.err(), Some(TurnError::SubmissionReused));

    // Break admission: drop the session row under the plan so the insert fails, then confirm no reservation remains.
    let doomed = h.engine.store.create_session(NewSession { workspace_id: &h.session.workspace_id, parent_id: None, visibility: Visibility::Sibling, title: "", agent: "build", model: None }).unwrap();
    h.engine.store.lock().execute("CREATE TRIGGER block BEFORE INSERT ON part BEGIN SELECT RAISE(ABORT, 'no parts'); END", []).unwrap();
    let failed = h.engine.submit(&doomed.id, prompt("boom")).await;
    assert!(matches!(failed, Err(TurnError::Store(_))), "{failed:?}");
    h.engine.store.lock().execute("DROP TRIGGER block", []).unwrap();
    assert!(!h.engine.turns.is_running(&doomed.id), "a failed admission must not leave the session busy");
    assert!(h.engine.store.transcript(&doomed.id).unwrap().is_empty(), "no half-written prompt");
    h.provider.push(text("fine"));
    h.engine.submit(&doomed.id, prompt("retry")).await.await_ok();
    until_idle(&h).await;
}

#[tokio::test]
async fn concurrent_turns_refresh_an_expired_token_once() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let app = axum::Router::new().route(
        "/token",
        axum::routing::post(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(100)).await;
                axum::Json(json!({ "access_token": "fresh", "refresh_token": "r2", "expires_in": 3600 }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    std::env::set_var("DRIFT_ANTHROPIC_TOKEN_URL", format!("http://{}/token", listener.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let h = harness().await;
    h.engine.credentials.set("anthropic", &Credential::OAuth { access: "stale".into(), refresh: "r1".into(), expires_at: 1, account: None }).unwrap();
    let other = h.engine.store.create_session(NewSession { workspace_id: &h.session.workspace_id, parent_id: None, visibility: Visibility::Sibling, title: "", agent: "build", model: None }).unwrap();
    h.provider.push(text("a")).push(text("b"));
    let (first, second) = tokio::join!(h.engine.submit(&h.session.id, prompt("one")), h.engine.submit(&other.id, prompt("two")));
    first.await_ok();
    second.await_ok();
    until_idle(&h).await;
    std::env::remove_var("DRIFT_ANTHROPIC_TOKEN_URL");
    assert_eq!(hits.load(Ordering::SeqCst), 1, "one refresh for two turns");
    let stored = h.engine.credentials.get("anthropic").unwrap();
    assert!(matches!(stored, Credential::OAuth { access, refresh, .. } if access == "fresh" && refresh == "r2"));
}

#[tokio::test]
async fn submission_ids_survive_a_restart_and_reject_a_different_payload() {
    let h = harness().await;
    h.provider.push(text("ok"));
    let mut first = prompt("hello");
    first.submission_id = Some("sub_durable".into());
    let receipt = h.engine.submit(&h.session.id, first.clone()).await.await_ok();
    until_idle(&h).await;

    let reopened = Engine::open_with(&h._dir.join("data"), crate::Options { file_credentials: true, ..Default::default() }).unwrap();
    *reopened.turns.provider_override.lock().unwrap() = Some(Provider::Scripted(h.provider.clone()));
    let replay = reopened.submit(&h.session.id, first).await.await_ok();
    assert_eq!(replay.message.id, receipt.message.id);
    assert_eq!(reopened.store.transcript(&h.session.id).unwrap().len(), 2, "no second prompt after restart");

    let mut changed = prompt("different text");
    changed.submission_id = Some("sub_durable".into());
    assert_eq!(reopened.submit(&h.session.id, changed).await.err(), Some(TurnError::SubmissionReused));
}

#[tokio::test]
async fn a_write_is_refused_when_the_snapshot_cannot_be_taken() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "edit".into(), pattern: "*".into(), decision: Decision::Allow }] });
    // A file where the snapshot directory must go makes every snapshot fail.
    std::fs::write(h._dir.join("data/snapshots"), "not a directory").unwrap();
    h.provider.push(tool_call("write", r#"{"path": "new.txt", "content": "x\n"}"#)).push(text("noted"));
    h.engine.submit(&h.session.id, prompt("write")).await.await_ok();
    until_idle(&h).await;
    assert!(!h._dir.join("ws/new.txt").exists(), "nothing may be written without a snapshot");
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Error);
    assert!(output.as_deref().unwrap().contains("could not snapshot"));
}

#[tokio::test]
async fn a_call_that_cannot_be_recorded_does_not_run() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "edit".into(), pattern: "*".into(), decision: Decision::Allow }] });
    h.provider.push(tool_call("write", r#"{"path": "new.txt", "content": "x\n"}"#)).push(text("noted"));
    h.engine.store.lock().execute("CREATE TRIGGER block BEFORE UPDATE ON part BEGIN SELECT RAISE(ABORT, 'disk full'); END", []).unwrap();
    h.engine.submit(&h.session.id, prompt("write")).await.await_ok();
    until_idle(&h).await;
    h.engine.store.lock().execute("DROP TRIGGER block", []).unwrap();
    assert!(!h._dir.join("ws/new.txt").exists(), "a write whose start could not be recorded must not happen");
}

#[tokio::test]
async fn malformed_call_arguments_and_max_tokens_stop_dispatch() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    h.provider
        .push(vec![Chunk::ToolUseStart { id: "t1".into(), name: "read".into() }, Chunk::ToolInputDelta(r#"{"path": "a.tx"#.into()), Chunk::BlockStop, Chunk::Stop(StopReason::ToolUse)])
        .push(text("ok"));
    h.engine.submit(&h.session.id, prompt("read")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Error);
    assert!(output.as_deref().unwrap().contains("not valid JSON"));

    h.provider.push(vec![Chunk::ToolUseStart { id: "t2".into(), name: "read".into() }, Chunk::ToolInputDelta(r#"{"path": "a.txt"}"#.into()), Chunk::BlockStop, Chunk::Stop(StopReason::MaxTokens)]);
    h.engine.submit(&h.session.id, prompt("again")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, .. } = &transcript.last().unwrap().parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Pending, "a max_tokens stop dispatches nothing");
}

#[tokio::test]
async fn workspace_config_shapes_the_turn() {
    let h = harness().await;
    let ws = h._dir.join("ws");
    std::fs::write(ws.join("drift.json"), r#"{ "permissions": [{ "kind": "bash", "pattern": "*", "decision": "deny" }] }"#).unwrap();
    std::fs::create_dir_all(ws.join(".drift/skills/tidy")).unwrap();
    std::fs::write(ws.join(".drift/skills/tidy/SKILL.md"), "---\ndescription: Tidies\n---\nTidy up.").unwrap();

    // drift.json denies bash without asking.
    h.provider.push(tool_call("bash", r#"{"command": "echo hi"}"#)).push(text("denied"));
    h.engine.submit(&h.session.id, prompt("run")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Denied);
    let system = h.provider.requests.lock().unwrap()[0].system.clone();
    assert!(system.contains("- tidy: Tidies"), "skills are listed in the system prompt");

    // A plan session sees only read-only tools and the plan prompt.
    h.engine.store.update_session(&h.session.id, None, None, Some("plan")).unwrap();
    h.provider.push(text("planned"));
    h.engine.submit(&h.session.id, prompt("plan it")).await.await_ok();
    until_idle(&h).await;
    let request = h.provider.requests.lock().unwrap().last().unwrap().clone();
    let names: Vec<&str> = request.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(!names.contains(&"write") && !names.contains(&"bash") && names.contains(&"read"), "{names:?}");
    assert!(request.system.contains("# Plan mode"));
}

#[tokio::test]
async fn a_configured_formatter_runs_after_a_write() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "edit".into(), pattern: "*".into(), decision: Decision::Allow }] });
    let (program, rest) = if cfg!(windows) { ("cmd", r#""/c", "echo tidy> $FILE""#) } else { ("sh", r#""-c", "echo tidy > $FILE""#) };
    let config = format!(r#"{{ "formatters": {{ "tidy": {{ "command": ["{program}", {rest}], "extensions": [".txt"] }} }} }}"#);
    std::fs::write(h._dir.join("ws/drift.json"), config).unwrap();
    h.provider.push(tool_call("write", r#"{"path": "note.txt", "content": "raw\n"}"#)).push(text("written"));
    h.engine.submit(&h.session.id, prompt("write")).await.await_ok();
    until_idle(&h).await;
    assert!(std::fs::read_to_string(h._dir.join("ws/note.txt")).unwrap().starts_with("tidy"));
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { metadata, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(metadata.as_ref().unwrap()["formatted"][0], "tidy: note.txt");
}

#[tokio::test]
async fn a_call_to_a_tool_the_run_did_not_offer_is_refused_before_anything_happens() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "edit".into(), pattern: "*".into(), decision: Decision::Allow }] });
    h.engine.store.update_session(&h.session.id, None, None, Some("plan")).unwrap();
    h.provider.push(tool_call("write", r#"{"path": "plan-mutated.txt", "content": "x\n"}"#)).push(text("noted"));
    h.engine.submit(&h.session.id, prompt("write it anyway")).await.await_ok();
    until_idle(&h).await;
    assert!(!h._dir.join("ws/plan-mutated.txt").exists(), "plan mode must not write");
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, metadata, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Error);
    assert!(output.as_deref().unwrap().contains("not available in this session"));
    assert!(metadata.is_none(), "no snapshot was taken");
}

#[tokio::test]
async fn a_task_runs_a_hidden_child_and_returns_its_reply() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "alpha\n").unwrap();
    // Parent: task call. Child: read a.txt, then reply. Parent: final text.
    h.provider
        .push(tool_call("task", r#"{"description": "Check a.txt", "prompt": "What is in a.txt?"}"#))
        .push(tool_call("read", r#"{"path": "a.txt"}"#))
        .push(text("a.txt contains alpha"))
        .push(text("The subagent says alpha"));
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, metadata, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Done);
    assert_eq!(output.as_deref(), Some("a.txt contains alpha"));
    let child_id = metadata.as_ref().unwrap()["sessionId"].as_str().unwrap().to_string();
    let child = h.engine.store.session(&child_id).unwrap().unwrap();
    assert_eq!(child.parent_id.as_deref(), Some(h.session.id.as_str()));
    assert_eq!(child.visibility, Visibility::Hidden);
    assert_eq!(child.title, "Check a.txt (@build subagent)");
    assert_eq!(h.engine.store.transcript(&child_id).unwrap().len(), 3);
    let listed = h.engine.store.sessions(crate::store::SessionFilter { workspace_id: None, archived: false, before: None, limit: 10 }).unwrap();
    assert!(listed.iter().any(|s| s.id == child_id && s.parent_id.as_deref() == Some(h.session.id.as_str())), "subagents are listed so the UI can nest them");
}

#[tokio::test]
async fn a_subagent_runs_on_its_agents_pinned_model_and_actions_are_not_agents() {
    let h = harness().await;
    let pinned = h.engine.catalog.read().unwrap().providers["anthropic"].models.keys().find(|id| id.as_str() != "claude-sonnet-4-5").unwrap().clone();
    let pin = crate::config::AgentOverride::from_json(&json!({ "model": format!("anthropic/{pinned}") }));
    h.engine.set_agent_overrides(std::collections::HashMap::from([("build".to_string(), pin)]));
    h.provider
        .push(tool_call("task", r#"{"description": "Pinned", "prompt": "go"}"#))
        .push(text("child done"))
        .push(tool_call("task", r#"{"description": "Nope", "prompt": "go", "subagent_type": "title"}"#))
        .push(text("parent done"));
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    until_idle(&h).await;
    let requests = h.provider.requests.lock().unwrap().clone();
    assert_eq!(requests[0].model, "claude-sonnet-4-5", "the parent keeps the model it was prompted with");
    assert_eq!(requests[1].model, pinned, "the subagent runs on the build agent's pin");
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, .. } = &transcript[2].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Error);
    assert!(output.as_deref().unwrap().contains("engine action"));
}

#[tokio::test]
async fn aborting_the_parent_aborts_a_running_child() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Allow }] });
    let sleep = if cfg!(windows) { "ping -n 10 127.0.0.1 > nul" } else { "sleep 10" };
    h.provider.push(tool_call("task", r#"{"description": "Wait", "prompt": "wait"}"#)).push(tool_call("bash", &json!({ "command": sleep }).to_string()));
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(600)).await;
    let children = h.engine.store.lock().query_row("SELECT id FROM session WHERE parent_id = ?1", [&h.session.id], |r| r.get::<_, String>(0)).unwrap();
    assert!(h.engine.turns.is_running(&children));
    assert!(h.engine.abort(&h.session.id));
    until_idle(&h).await;
    for _ in 0..100 {
        if !h.engine.turns.is_running(&children) { break }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!h.engine.turns.is_running(&children), "the child must stop with its parent");
}


#[tokio::test]
async fn subagents_are_not_offered_delegation_and_cannot_call_it() {
    let h = harness().await;
    h.provider
        .push(tool_call("task", r#"{"description": "Nest", "prompt": "try to spawn"}"#))
        .push(tool_call("task", r#"{"description": "Sneaky", "prompt": "nest"}"#))
        .push(text("could not"))
        .push(text("done"));
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    until_idle(&h).await;
    let requests = h.provider.requests.lock().unwrap().clone();
    let names = |i: usize| requests[i].tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
    assert!(names(0).contains(&"task".to_string()) && !names(0).contains(&"spawn_thread".to_string()));
    for tool in crate::tool::task::DELEGATION {
        assert!(!names(1).iter().any(|n| n == tool), "subagent was offered {tool}");
    }
    let count: i64 = h.engine.store.lock().query_row("SELECT COUNT(*) FROM session WHERE title LIKE 'Sneaky%'", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 0);
    let child = h.engine.store.lock().query_row("SELECT id FROM session WHERE parent_id = ?1", [&h.session.id], |r| r.get::<_, String>(0)).unwrap();
    let Part::ToolCall { status, .. } = &h.engine.store.transcript(&child).unwrap()[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Error);
}
