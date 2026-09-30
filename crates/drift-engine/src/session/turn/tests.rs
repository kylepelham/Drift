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
    let changes = &metadata.as_ref().unwrap()["changes"];
    assert_eq!(changes[0]["path"], "new.txt", "{metadata:?}");
    assert!(changes[0]["before"].is_null() && changes[0]["after"].is_string(), "a new file: nothing before, a blob after");
}

#[tokio::test]
async fn a_shell_call_shows_its_limit_while_running_and_fails_when_it_expires() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Allow }] });
    h.engine.set_shell_timeout(Some(Duration::from_millis(400)));
    let sleep = if cfg!(windows) { "ping -n 10 127.0.0.1" } else { "sleep 10" };
    let mut rx = h.engine.hub.attach(None).rx;
    h.provider.push(tool_call("bash", &json!({ "command": sleep }).to_string())).push(text("it was too slow"));
    h.engine.submit(&h.session.id, prompt("wait")).await.await_ok();
    let running = loop {
        let envelope = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.unwrap().unwrap();
        if let Event::PartUpdated { part } = envelope.event {
            if let Part::ToolCall { status: ToolStatus::Running, metadata, .. } = part.part {
                break metadata;
            }
        }
    };
    assert_eq!(running.unwrap()["shellTimeoutMs"], 400, "the badge has the limit while the command runs");
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, metadata, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Error);
    let metadata = metadata.as_ref().unwrap();
    assert_eq!((metadata["timedOut"].as_bool(), metadata["shellTimeoutMs"].as_u64()), (Some(true), Some(400)));
    assert!(metadata["changes"].is_array(), "what the command changed is recorded next to the timeout details");
}

#[tokio::test]
async fn abort_marks_the_message_and_frees_the_session() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Allow }] });
    let sleep = if cfg!(windows) { "ping -n 10 127.0.0.1" } else { "sleep 10" };
    h.provider.push(tool_call("bash", &json!({ "command": sleep }).to_string()));
    h.engine.submit(&h.session.id, prompt("wait")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(300)).await;
    h.engine.submit(&h.session.id, prompt("again")).await.expect("steered into the running turn");
    assert!(h.engine.abort(&h.session.id));
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Error);
    assert!(!h.engine.abort(&h.session.id));
    assert_eq!(h.provider.requests.lock().unwrap().len(), 1, "a stop is not overridden by a steered prompt");
    assert_eq!(transcript.last().unwrap().info.role, Role::User, "the steered prompt stays for the next turn");
}

/// An overload that asks for a long wait, so a test can act while the turn waits.
fn overloaded() -> llm::Error {
    asking_to_wait(Duration::from_secs(30))
}

fn asking_to_wait(wait: Duration) -> llm::Error {
    let llm::Error::Api { status, kind, message, retryable, .. } = llm::Error::api(529, "overloaded_error", "busy") else { unreachable!() };
    llm::Error::Api { status, kind, message, retryable, retry_after: Some(wait) }
}

async fn until_waiting_to_retry(h: &Harness) {
    for _ in 0..200 {
        if h.engine.turns.retry_waits.lock().unwrap().contains_key(&h.session.id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the turn never waited to retry");
}

#[tokio::test]
async fn a_retry_wait_is_announced_and_ends_with_running_again() {
    let h = harness().await;
    let mut rx = h.engine.hub.attach(None).rx;
    h.provider.push_error(overloaded()).push(text("second time lucky"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    let (attempt, message, next_at) = loop {
        let envelope = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.unwrap().unwrap();
        if let Event::SessionRetry { attempt, message, next_at, .. } = envelope.event {
            break (attempt, message, next_at);
        }
    };
    assert_eq!(attempt, 1);
    assert!(message.contains("busy"), "{message}");
    let wait = next_at - id::now_ms();
    assert!((29_000..=30_000).contains(&wait), "the provider's own wait is used: {wait}ms");
    assert!(h.engine.switch_retry_model(&h.session.id, &ModelRef { provider: "anthropic".into(), model: "claude-sonnet-4-5".into() }).await.is_ok());
    let running_again = loop {
        let envelope = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.unwrap().unwrap();
        if let Event::SessionStatusChanged { status, .. } = envelope.event {
            break status;
        }
    };
    assert_eq!(running_again, SessionStatus::Running);
    until_idle(&h).await;
}

#[tokio::test]
async fn a_turn_waiting_to_retry_can_be_moved_to_another_model_and_keeps_it() {
    let h = harness().await;
    let pinned = h.engine.catalog.read().unwrap().providers["anthropic"].models.keys().find(|id| id.as_str() != "claude-sonnet-4-5").unwrap().clone();
    let other = ModelRef { provider: "anthropic".into(), model: pinned.clone() };
    assert_eq!(h.engine.switch_retry_model(&h.session.id, &other).await, Err(TurnError::NotRetrying), "nothing is waiting yet");

    h.provider.push_error(overloaded()).push(text("answered by the other model"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_waiting_to_retry(&h).await;
    let unusable = ModelRef { provider: "openai".into(), model: "gpt-5".into() };
    assert_eq!(h.engine.switch_retry_model(&h.session.id, &unusable).await, Err(TurnError::NoCredentials), "a model without a credential is refused up front");
    let started = std::time::Instant::now();
    h.engine.switch_retry_model(&h.session.id, &other).await.unwrap();
    until_idle(&h).await;
    assert!(started.elapsed() < Duration::from_millis(900), "the switch retries at once instead of waiting out the backoff");
    let requests = h.provider.requests.lock().unwrap().clone();
    assert_eq!((requests[0].model.as_str(), requests[1].model.as_str()), ("claude-sonnet-4-5", pinned.as_str()));
    assert_eq!(h.engine.store.session(&h.session.id).unwrap().unwrap().model, Some(other), "the session keeps the model it was switched to");
}

#[tokio::test]
async fn stop_ends_a_retry_wait_at_once() {
    let h = harness().await;
    h.provider.push_error(overloaded());
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_waiting_to_retry(&h).await;
    let started = std::time::Instant::now();
    assert!(h.engine.abort(&h.session.id));
    until_idle(&h).await;
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(h.provider.requests.lock().unwrap().len(), 1, "no attempt after the stop");
}

#[tokio::test]
async fn retryable_provider_errors_are_retried_and_others_are_not() {
    let h = harness().await;
    h.provider.push_error(llm::Error::api(529, "overloaded_error", "busy")).push(text("second time lucky"));
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
async fn stop_ends_a_request_still_waiting_for_its_response() {
    let h = harness().await;
    let url = crate::llm::tests::silent_server().await;
    *h.engine.turns.provider_override.lock().unwrap() = Some(llm::Provider::Anthropic(llm::anthropic::Anthropic::new(&url)));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = std::time::Instant::now();
    assert!(h.engine.abort(&h.session.id));
    until_idle(&h).await;
    assert!(started.elapsed() < Duration::from_secs(1), "no wait for the 120 s header limit");
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert_eq!(transcript.last().unwrap().info.status, MessageStatus::Aborted);
}

#[tokio::test]
async fn an_overload_inside_the_stream_is_retried() {
    let h = harness().await;
    let partial = vec![Chunk::TextStart, Chunk::TextDelta("Let me".into())];
    h.provider.push_fail_midway(partial, llm::Error::api(llm::STREAMED, "overloaded_error", "Overloaded")).push(text("answered"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert_eq!(transcript.len(), 3, "the stream's overload was retried");
    assert_eq!(transcript[1].info.status, MessageStatus::Error);
    assert!(transcript[1].info.error.as_deref().unwrap().contains("overloaded_error"));
    assert_eq!(transcript[2].info.status, MessageStatus::Done);
}

#[tokio::test]
async fn a_provider_asking_for_too_long_a_wait_is_not_waited_on() {
    let h = harness().await;
    h.provider.push_error(asking_to_wait(MAX_REQUESTED_WAIT + Duration::from_secs(1))).push(text("never asked"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_idle(&h).await;
    assert_eq!(h.provider.requests.lock().unwrap().len(), 1, "no retry");
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert_eq!(transcript.last().unwrap().info.status, MessageStatus::Error, "the error stands for the user to see");
}

#[tokio::test]
async fn a_spent_quota_is_one_request_and_an_endless_wait_releases_the_session() {
    let h = harness().await;
    h.provider.push_error(llm::Error::api(429, "insufficient_quota", "You exceeded your current quota"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_idle(&h).await;
    assert_eq!(h.provider.requests.lock().unwrap().len(), 1, "a spent quota is not retried");

    h.provider.push_error(asking_to_wait(Duration::MAX));
    h.engine.submit(&h.session.id, prompt("again")).await.await_ok();
    until_idle(&h).await;
    assert_eq!(h.provider.requests.lock().unwrap().len(), 2, "no retry, and the session is free");
    assert!(!h.engine.turns.is_running(&h.session.id));
}

#[tokio::test]
async fn a_job_that_panics_still_releases_its_session() {
    let h = harness().await;
    assert!(h.engine.turns.claim(&h.session.id, &CancellationToken::new()));
    h.engine.spawn_job(&h.session.id, async { panic!("a bug in a job") });
    until_idle(&h).await;
    assert!(!h.engine.turns.is_running(&h.session.id));
    h.provider.push(text("still usable"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_idle(&h).await;
}

#[test]
fn backoff_doubles_with_jitter_under_a_cap_and_a_named_wait_is_used_as_is() {
    let unnamed = Retry { message: String::new(), after: None };
    for attempt in 1..=MAX_RETRIES {
        let delay = unnamed.delay(attempt);
        let nominal = RETRY_BASE.saturating_mul(1 << (attempt - 1));
        assert!(delay >= nominal.mul_f64(0.79).min(MAX_BACKOFF) && delay <= nominal.mul_f64(1.21).min(MAX_BACKOFF), "attempt {attempt}: {delay:?}");
    }
    for _ in 0..50 {
        assert!(unnamed.delay(40) <= MAX_BACKOFF, "the cap holds after jitter, however many attempts");
    }
    let named = Retry { message: String::new(), after: Some(Duration::from_millis(1500)) };
    assert_eq!(named.delay(5), Duration::from_millis(1500));
    assert!(named.allowed(MAX_RETRIES - 1) && !named.allowed(MAX_RETRIES));
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
    for _ in 0..=MAX_RETRIES {
        h.provider.push(truncated.clone());
    }
    h.engine.submit(&h.session.id, prompt("read a")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    for message in transcript.iter().skip(1) {
        assert_eq!(message.info.status, MessageStatus::Error);
        let Part::ToolCall { status, output, .. } = &message.parts[0].part else { panic!() };
        assert_eq!(*status, ToolStatus::Error, "the call must never run, and is closed rather than left pending");
        assert!(output.as_deref().is_some_and(|o| o.starts_with("Not run:")), "{output:?}");
    }
    assert_eq!(transcript.len(), 2 + MAX_RETRIES as usize, "the first attempt and every retry, then it gives up");
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
    let other = h.engine.store.create_session(NewSession { workspace_id: &h.session.workspace_id, parent_id: None, visibility: Visibility::Sibling, title: "Other", agent: "build", model: None }).unwrap();
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
async fn a_write_is_refused_when_its_files_cannot_be_recorded() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "edit".into(), pattern: "*".into(), decision: Decision::Allow }] });
    // A file where the snapshot directory must go makes every snapshot fail.
    std::fs::write(h._dir.join("data/snapshots"), "not a directory").unwrap();
    h.provider.push(tool_call("write", r#"{"path": "new.txt", "content": "x\n"}"#)).push(text("noted"));
    h.engine.submit(&h.session.id, prompt("write")).await.await_ok();
    until_idle(&h).await;
    assert!(!h._dir.join("ws/new.txt").exists(), "nothing may be written that could not be undone");
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Error);
    assert!(output.as_deref().unwrap().contains("could not record the files"));
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
    let last = transcript.last().unwrap();
    let Part::ToolCall { status, output, .. } = &last.parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Error, "a max_tokens stop dispatches nothing, and says so");
    assert!(output.as_deref().unwrap().starts_with("Not run: the reply hit its output limit"), "{output:?}");
    assert_eq!(last.info.status, MessageStatus::Done);
    assert!(last.info.error.as_deref().unwrap().starts_with(OUTPUT_LIMIT_ENDING), "the ending is visible: {:?}", last.info.error);
}

#[tokio::test]
async fn a_reply_cut_off_without_calls_still_says_so() {
    let h = harness().await;
    h.provider.push(vec![Chunk::TextStart, Chunk::TextDelta("The answer is".into()), Chunk::BlockStop, Chunk::Stop(StopReason::MaxTokens)]);
    h.engine.submit(&h.session.id, prompt("long")).await.await_ok();
    until_idle(&h).await;
    let last = h.engine.store.transcript(&h.session.id).unwrap().pop().unwrap();
    assert_eq!(last.info.status, MessageStatus::Done);
    assert!(last.info.error.as_deref().is_some_and(|e| e.starts_with(OUTPUT_LIMIT_ENDING)));
}

#[tokio::test]
async fn any_tool_result_past_the_bound_is_cut_to_its_ends_with_the_whole_on_disk() {
    let h = harness().await;
    let skill = h._dir.join("ws/.drift/skills/huge");
    std::fs::create_dir_all(&skill).unwrap();
    let body = format!("FIRST\n{}\nLAST", "guidance line\n".repeat(20_000));
    std::fs::write(skill.join("SKILL.md"), format!("---\ndescription: Huge\n---\n{body}")).unwrap();
    h.provider.push(tool_call("skill", r#"{"name": "huge"}"#)).push(text("read it"));
    h.engine.submit(&h.session.id, prompt("use the skill")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { output, metadata, .. } = &transcript[1].parts[0].part else { panic!() };
    let output = output.as_deref().unwrap();
    assert!(output.len() <= crate::tool::spool::MAX_RESULT_BYTES && output.contains("FIRST") && output.trim_end().ends_with("LAST"), "{}", output.len());
    let file = metadata.as_ref().unwrap()["resultFile"].as_str().expect("the whole result is kept");
    assert!(std::fs::read_to_string(file).unwrap().contains(&body));
    let sent = h.provider.requests.lock().unwrap()[1].clone();
    let result = sent.messages.iter().flat_map(|m| &m.blocks).find_map(|b| match b { llm::Block::ToolResult { content, .. } => Some(content.len()), _ => None }).unwrap();
    assert!(result <= crate::tool::spool::MAX_RESULT_BYTES, "the model got the bounded text");
}

fn texts_sent(request: &llm::Request) -> Vec<String> {
    request.messages.iter().flat_map(|m| &m.blocks).filter_map(|b| match b { llm::Block::Text(t) => Some(t.clone()), _ => None }).collect()
}

async fn until_call_running(h: &Harness) {
    for _ in 0..400 {
        let transcript = h.engine.store.transcript(&h.session.id).unwrap();
        if transcript.iter().flat_map(|m| &m.parts).any(|row| matches!(row.part, Part::ToolCall { status: ToolStatus::Running, .. })) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("no call started");
}

#[tokio::test]
async fn a_prompt_sent_during_a_call_reaches_the_next_request_after_its_result() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Allow }] });
    h.provider.push(tool_call("bash", r#"{"command": "sleep 1"}"#)).push(text("done, and noted"));
    h.engine.submit(&h.session.id, prompt("start")).await.await_ok();
    until_call_running(&h).await;
    let mut steer = prompt("also check the logs");
    steer.submission_id = Some("steer-1".into());
    let first = h.engine.submit(&h.session.id, steer.clone()).await.expect("a busy turn takes the prompt");
    let again = h.engine.submit(&h.session.id, steer).await.unwrap();
    assert_eq!(first.message.id, again.message.id, "the same submission is one prompt");
    until_idle(&h).await;
    let requests = h.provider.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2, "taken at the next request, not as a turn of its own");
    let last = requests[1].messages.last().unwrap();
    assert!(matches!(last.blocks[0], llm::Block::ToolResult { .. }), "the call's result comes first");
    assert!(matches!(last.blocks.last().unwrap(), llm::Block::Text(t) if t == "also check the logs"));
    assert_eq!(h.provider.responses_left(), 0);
}

#[tokio::test]
async fn a_prompt_sent_while_the_last_reply_streams_is_answered_before_the_turn_ends() {
    let h = harness().await;
    h.provider.push_slow(Duration::from_millis(600), text("first answer")).push(text("second answer"));
    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(150)).await;
    h.engine.submit(&h.session.id, prompt("two")).await.expect("steered");
    until_idle(&h).await;
    let requests = h.provider.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(texts_sent(&requests[1]), ["one", "first answer", "two"], "ordered as sent");
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert_eq!(transcript.last().unwrap().info.status, MessageStatus::Done);
}

#[tokio::test]
async fn a_prompt_sent_during_another_job_waits_and_then_runs() {
    let h = harness().await;
    assert!(h.engine.turns.claim(&h.session.id, &CancellationToken::new()));
    let engine = h.engine.clone();
    let id = h.session.id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        engine.turns.release(&id);
    });
    h.provider.push(text("after the job"));
    let started = std::time::Instant::now();
    h.engine.submit(&h.session.id, prompt("queued")).await.expect("queued behind the job");
    assert!(started.elapsed() >= Duration::from_millis(250));
    until_idle(&h).await;
    assert_eq!(h.provider.responses_left(), 0);
}

fn limits(h: &Harness, json: &str) {
    std::fs::write(h._dir.join("ws/drift.json"), format!(r#"{{ "limits": {json} }}"#)).unwrap();
}

fn read_a() -> Vec<Chunk> {
    tool_call("read", r#"{"path": "a.txt"}"#)
}

#[tokio::test]
async fn a_turn_pauses_at_its_step_limit_and_a_message_carries_on() {
    let h = harness().await;
    limits(&h, r#"{ "steps": 2 }"#);
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    std::fs::write(h._dir.join("ws/b.txt"), "b\n").unwrap();
    h.provider.push(read_a()).push(tool_call("read", r#"{"path": "b.txt"}"#)).push(read_a()).push(text("finished"));
    h.engine.submit(&h.session.id, prompt("work")).await.await_ok();
    until_idle(&h).await;
    let last = h.engine.store.transcript(&h.session.id).unwrap().pop().unwrap();
    assert_eq!(last.info.status, MessageStatus::Paused);
    assert!(last.info.error.as_deref().unwrap().starts_with("Paused after 2 steps"), "{:?}", last.info.error);
    assert_eq!(h.provider.responses_left(), 2, "no request after the limit");
    h.engine.submit(&h.session.id, prompt("carry on")).await.await_ok();
    until_idle(&h).await;
    assert_eq!(h.provider.responses_left(), 0);
    let requests = h.provider.requests.lock().unwrap().clone();
    assert!(!requests.last().unwrap().messages.iter().flat_map(|m| &m.blocks).any(|b| matches!(b, llm::Block::Text(t) if t.starts_with("Paused"))), "the pause is not replayed to the model");
}

#[tokio::test]
async fn the_same_calls_with_the_same_results_pause_the_turn() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    h.provider.push(read_a()).push(read_a()).push(read_a()).push(text("never"));
    h.engine.submit(&h.session.id, prompt("loop")).await.await_ok();
    until_idle(&h).await;
    let last = h.engine.store.transcript(&h.session.id).unwrap().pop().unwrap();
    assert_eq!(last.info.status, MessageStatus::Paused);
    assert!(last.info.error.as_deref().unwrap().contains("same calls and got the same results"), "{:?}", last.info.error);
    assert_eq!(h.provider.responses_left(), 1);
}

#[tokio::test]
async fn polling_that_waits_on_purpose_is_allowed_to_repeat() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Allow }] });
    std::fs::write(h._dir.join("ws/status.txt"), "pending\n").unwrap();
    let poll = tool_call("bash", r#"{"command": "sleep 0 && cat status.txt"}"#);
    h.provider.push(poll.clone()).push(poll.clone()).push(poll.clone()).push(poll).push(text("still pending, stopping"));
    h.engine.submit(&h.session.id, prompt("wait for it")).await.await_ok();
    until_idle(&h).await;
    let last = h.engine.store.transcript(&h.session.id).unwrap().pop().unwrap();
    assert_eq!(last.info.status, MessageStatus::Done, "four identical polls are within the polling allowance");
}

#[test]
fn only_identical_results_count_as_repeats() {
    let limits = crate::config::Limits::default();
    let call = |output: &str| vec![CallTrace { name: "read".into(), input: r#"{"path":"a"}"#.into(), output: output.into() }];
    let mut repeats = Repeats::default();
    assert_eq!(repeats.record(call("1"), &limits), None);
    assert_eq!(repeats.record(call("2"), &limits), None, "a different result is progress");
    assert_eq!(repeats.record(call("2"), &limits), None);
    assert_eq!(repeats.record(call("2"), &limits), Some(3));
    assert_eq!(repeats.record(Vec::new(), &limits), None, "a step without calls resets");
    let waiting = CallTrace { name: "bash".into(), input: r#"{"command":"Start-Sleep 5; gh run view"}"#.into(), output: "queued".into() };
    assert!(waits(&waiting));
    assert!(!waits(&CallTrace { name: "bash".into(), input: r#"{"command":"cat sleepy.txt"}"#.into(), output: String::new() }), "a word inside a name is not a wait");
}

#[test]
fn an_agent_can_have_its_own_step_limit() {
    let dir = std::env::temp_dir().join(format!("drift-limits-{}", crate::random_hex(4)));
    std::fs::create_dir_all(dir.join(".drift/agents")).unwrap();
    std::fs::write(dir.join("drift.json"), r#"{ "limits": { "steps": 50, "repeats": 4 } }"#).unwrap();
    std::fs::write(dir.join(".drift/agents/quick.md"), "---\ndescription: Quick\nsteps: 5\n---\nBe quick.").unwrap();
    let config = crate::config::Config::load_with_home(&dir, None);
    assert_eq!(config.limits_for("build"), crate::config::Limits { steps: 50, repeats: 4, polls: 30 });
    assert_eq!(config.limits_for("quick").steps, 5);
    std::fs::remove_dir_all(dir).ok();
}

fn model_with(output: u64, reasoning: bool) -> crate::llm::catalog::Model {
    let mut model = crate::llm::catalog::Catalog::bundled().providers["anthropic"].models["claude-sonnet-4-5"].clone();
    model.limit.output = output;
    model.reasoning = reasoning;
    model
}

#[test]
fn output_and_thinking_budgets_are_valid_together() {
    assert_eq!(budgets(&model_with(32_000, true), Some(32_000)), (32_000, Some(32_000 - MIN_ANSWER_TOKENS)), "never past the model's own limit");
    assert_eq!(budgets(&model_with(64_000, true), Some(32_000)), (33_024, Some(32_000)), "a budget may raise the output past our cap");
    assert_eq!(budgets(&model_with(64_000, true), Some(100_000)), (64_000, Some(64_000 - MIN_ANSWER_TOKENS)));
    assert_eq!(budgets(&model_with(64_000, true), Some(10)), (32_000, Some(MIN_THINKING_TOKENS)), "raised to the provider's minimum");
    assert_eq!(budgets(&model_with(1_500, true), Some(8_000)), (1_500, None), "no room for thinking and an answer");
    assert_eq!(budgets(&model_with(64_000, false), Some(8_000)), (32_000, None), "a model that does not reason gets no budget");
    assert_eq!(budgets(&model_with(0, false), None), (MAX_OUTPUT_TOKENS, None), "an unknown limit uses our cap");
    for (limit, wanted) in [(4_096, 4_096), (8_192, 8_000), (128_000, 127_000), (2_048, 1_024)] {
        let (max, thinking) = budgets(&model_with(limit, true), Some(wanted));
        assert!(max as u64 <= limit, "{limit}/{wanted}");
        assert!(thinking.is_none_or(|t| t + MIN_ANSWER_TOKENS <= max && t >= MIN_THINKING_TOKENS), "{limit}/{wanted}: {max} {thinking:?}");
    }
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
    assert!(metadata.is_none(), "nothing was recorded for a call that never ran");
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
    assert_eq!(child.title, "Check a.txt (@general subagent)", "general takes a task when no type is given");
    assert_eq!(h.engine.store.transcript(&child_id).unwrap().len(), 3);
    let listed = h.engine.store.sessions(crate::store::SessionFilter { workspace_id: None, archived: false, before: None, limit: 10 }).unwrap();
    assert!(listed.iter().any(|s| s.id == child_id && s.parent_id.as_deref() == Some(h.session.id.as_str())), "subagents are listed so the UI can nest them");
}

fn too_long() -> llm::Error {
    llm::Error::api(400, "invalid_request_error", "prompt is too long: fixture overflow")
}

fn task_call(transcript: &[MessageWithParts]) -> (ToolStatus, String, serde_json::Value) {
    let Part::ToolCall { status, output, metadata, .. } = &transcript[1].parts[0].part else { panic!("{:?}", transcript[1].parts) };
    (*status, output.clone().unwrap_or_default(), metadata.clone().unwrap_or_default())
}

#[tokio::test]
async fn a_subagent_that_fails_after_compacting_reports_the_failure_not_the_summary() {
    let h = harness().await;
    h.provider
        .push(tool_call("task", r#"{"description": "Doomed", "prompt": "go"}"#))
        .push_error(too_long())
        .push(text("SUBAGENT_FAIL_MARK_SUMMARY"))
        .push_error(too_long())
        .push(text("parent carries on"));
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    until_idle(&h).await;
    let (status, output, metadata) = task_call(&h.engine.store.transcript(&h.session.id).unwrap());
    assert_eq!(status, ToolStatus::Error);
    assert!(output.contains("prompt is too long") && !output.contains("SUBAGENT_FAIL_MARK_SUMMARY"), "{output}");
    assert_eq!(metadata["outcome"], "failed");
    assert!(metadata["sessionId"].is_string(), "the card still opens the failed subagent");
}

#[tokio::test]
async fn a_subagent_stopped_after_compacting_is_not_answered_by_its_summary() {
    let h = harness().await;
    h.provider
        .push(tool_call("task", r#"{"description": "Stalls", "prompt": "go"}"#))
        .push_error(too_long())
        .push(text("SUBAGENT_STOP_MARK_SUMMARY"))
        .push_stall();
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    for _ in 0..200 {
        if h.provider.responses_left() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(h.engine.abort(&h.session.id));
    until_idle(&h).await;
    let (status, output, _) = task_call(&h.engine.store.transcript(&h.session.id).unwrap());
    assert_eq!(status, ToolStatus::Error);
    assert!(!output.contains("SUBAGENT_STOP_MARK_SUMMARY"), "{output}");
}

/// A child step that says something and then calls a tool, with usage past the compaction threshold.
fn progress_then(tool: &str, input: &str) -> Vec<Chunk> {
    vec![
        Chunk::Usage(Usage { input: 980_000, ..Usage::default() }),
        Chunk::TextStart,
        Chunk::TextDelta("PROGRESS_TEXT".into()),
        Chunk::BlockStop,
        Chunk::ToolUseStart { id: "toolu_progress".into(), name: tool.into() },
        Chunk::ToolInputDelta(input.into()),
        Chunk::BlockStop,
        Chunk::Stop(StopReason::ToolUse),
    ]
}

/// Waits until the scripted queue is down to `left`, then stops only the parent's subagent.
async fn stop_the_child_when(h: &Harness, left: usize) -> String {
    for _ in 0..300 {
        if h.provider.responses_left() == left {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    let child = h.engine.store.lock().query_row("SELECT id FROM session WHERE parent_id = ?1", [&h.session.id], |r| r.get::<_, String>(0)).unwrap();
    assert!(h.engine.abort(&child), "the child is running");
    child
}

#[tokio::test]
async fn stopping_only_the_subagent_while_it_compacts_reports_stopped_not_its_progress() {
    let h = harness().await;
    h.provider
        .push(tool_call("task", r#"{"description": "Compacting", "prompt": "go"}"#))
        .push(progress_then("glob", r#"{"pattern": "*.txt"}"#))
        .push_stall()
        .push(text("parent carries on"));
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    let child = stop_the_child_when(&h, 1).await;
    until_idle(&h).await;
    let child_last = h.engine.store.transcript(&child).unwrap().last().unwrap().clone();
    assert!(child_last.info.summary && child_last.info.status == MessageStatus::Aborted, "the stop landed in compaction");
    let (status, output, metadata) = task_call(&h.engine.store.transcript(&h.session.id).unwrap());
    assert_eq!(status, ToolStatus::Error);
    assert_eq!(metadata["outcome"], "stopped");
    assert_eq!(metadata["sessionId"], child.as_str(), "the card still opens the stopped subagent");
    assert!(!output.contains("PROGRESS_TEXT"), "{output}");
}

#[tokio::test]
async fn stopping_only_the_subagent_while_its_tool_runs_reports_stopped() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Allow }] });
    let sleep = if cfg!(windows) { "ping -n 10 127.0.0.1" } else { "sleep 10" };
    let mut step = progress_then("bash", &json!({ "command": sleep }).to_string());
    step[0] = Chunk::Usage(Usage { input: 10, ..Usage::default() });
    h.provider.push(tool_call("task", r#"{"description": "Sleeps", "prompt": "go"}"#)).push(step).push(text("parent carries on"));
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    stop_the_child_when(&h, 1).await;
    until_idle(&h).await;
    let (status, output, metadata) = task_call(&h.engine.store.transcript(&h.session.id).unwrap());
    assert_eq!((status, metadata["outcome"].as_str()), (ToolStatus::Error, Some("stopped")));
    assert!(!output.contains("PROGRESS_TEXT"), "{output}");
}

#[tokio::test]
async fn a_subagent_runs_on_its_agents_pinned_model_and_actions_are_not_agents() {
    let h = harness().await;
    let pinned = h.engine.catalog.read().unwrap().providers["anthropic"].models.keys().find(|id| id.as_str() != "claude-sonnet-4-5").unwrap().clone();
    let pin = crate::config::AgentOverride::from_json(&json!({ "model": format!("anthropic/{pinned}") }));
    h.engine.set_agent_overrides(std::collections::HashMap::from([("general".to_string(), pin)]));
    h.provider
        .push(tool_call("task", r#"{"description": "Pinned", "prompt": "go"}"#))
        .push(text("child done"))
        .push(tool_call("task", r#"{"description": "Nope", "prompt": "go", "subagent_type": "title"}"#))
        .push(text("parent done"));
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    until_idle(&h).await;
    let requests = h.provider.requests.lock().unwrap().clone();
    assert_eq!(requests[0].model, "claude-sonnet-4-5", "the parent keeps the model it was prompted with");
    assert_eq!(requests[1].model, pinned, "the subagent runs on the general agent's pin");
    assert!(requests[1].system.contains("delegated job"), "and with the general agent's prompt");
    assert!(requests[0].system.contains("# Subagents"), "the parent is told which subagents exist");
    assert!(!requests[1].system.contains("# Subagents"), "a subagent cannot delegate, so it is not told");
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, .. } = &transcript[2].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Error);
    assert!(output.as_deref().unwrap().contains("engine action"));
}

#[tokio::test]
async fn aborting_the_parent_aborts_a_running_child() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Allow }] });
    let sleep = if cfg!(windows) { "ping -n 10 127.0.0.1" } else { "sleep 10" };
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
