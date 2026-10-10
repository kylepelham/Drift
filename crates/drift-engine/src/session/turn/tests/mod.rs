use crate::event::Event;
use crate::llm::scripted::Scripted;
use crate::llm::{Chunk, Provider};
use crate::permission::{Decision, Policy, Reply, ReplyBody, Rule};
use crate::store::NewSession;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

use super::*;

mod admission_receipts;
mod agents;
mod attachments;
mod auto_accept;
mod check_history;
mod check_reports;
mod conversation;
mod credentials;
mod file_history;
mod grants;
mod images;
mod loop_limits;
mod orchestrator;
mod permissions;
mod plugin_hooks;
mod pricing;
mod project_commands;
mod retry_waits;
mod routes;
mod steering;
mod streaming;
mod subagent_endings;
mod subagents;
mod tool_catalog;
mod tool_order;
mod tool_validation;
mod unreadable_images;
mod variants;

pub(crate) struct Harness {
    pub(crate) engine: Arc<Engine>,
    pub(crate) session: Session,
    pub(crate) provider: Scripted,
    pub(crate) _dir: PathBuf,
}

pub(crate) struct ToolView<'a> {
    pub(crate) name: &'a str,
    pub(crate) status: ToolStatus,
    pub(crate) output: Option<&'a str>,
    pub(crate) title: Option<&'a str>,
    pub(crate) input: &'a serde_json::Value,
    pub(crate) metadata: Option<&'a ToolMetadata>,
}

trait AwaitOk {
    fn await_ok(self) -> Receipt;
}

impl AwaitOk for Result<Receipt, TurnError> {
    fn await_ok(self) -> Receipt {
        self.unwrap_or_else(|error| panic!("submit failed: {error}"))
    }
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
    let engine = Engine::open_with(
        &dir.join("data"),
        crate::Options {
            file_credentials: true,
            ..Default::default()
        },
    )
    .unwrap();

    let provider = Scripted::default();
    *engine.turns.provider_override.lock().unwrap() = Some(Provider::Scripted(provider.clone()));
    engine
        .credentials
        .set("anthropic", &Credential::ApiKey { key: "k".into() })
        .unwrap();
    let workspace = engine
        .store
        .add_workspace(&workspace.to_string_lossy(), "ws", "")
        .unwrap();
    let session = engine
        .store
        .create_session(NewSession {
            workspace_id: &workspace.id,
            parent_id: None,
            visibility: Visibility::Sibling,
            title: "Test",
            agent: "build",
            model: None,
        })
        .unwrap();

    Harness {
        engine,
        session,
        provider,
        _dir: dir,
    }
}

pub(crate) fn model() -> ModelRef {
    ModelRef {
        provider: "anthropic".into(),
        model: "claude-sonnet-4-5".into(),
    }
}

pub(crate) fn text(text: &str) -> Vec<Chunk> {
    vec![
        Chunk::Usage(Usage {
            input: 10,
            ..Usage::default()
        }),
        Chunk::TextStart,
        Chunk::TextDelta(text.into()),
        Chunk::BlockStop,
        Chunk::Usage(Usage {
            output: 3,
            ..Usage::default()
        }),
        Chunk::Stop(StopReason::EndTurn),
    ]
}

pub(crate) fn tool_call(name: &str, input: &str) -> Vec<Chunk> {
    vec![
        Chunk::ToolUseStart {
            id: format!("toolu_{name}"),
            name: name.into(),
        },
        Chunk::ToolInputDelta(input.into()),
        Chunk::BlockStop,
        Chunk::Stop(StopReason::ToolUse),
    ]
}

pub(crate) async fn until_idle(h: &Harness) {
    // Windows process startup can take seconds under load; finished turns return immediately.
    until_session_idle(h, &h.session.id).await;
}

async fn until_session_idle(h: &Harness, session_id: &str) {
    for _ in 0..1000 {
        if !h.engine.turns.is_running(session_id) {
            return;
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("turn never finished");
}

pub(crate) fn prompt(text: &str) -> Prompt {
    Prompt {
        parts: vec![Part::Text { text: text.into() }],
        model: Some(model()),
        variant: None,
        agent: None,
        submission_id: None,
    }
}

fn sibling_session(h: &Harness, title: &str) -> Session {
    h.engine
        .store
        .create_session(NewSession {
            workspace_id: &h.session.workspace_id,
            parent_id: None,
            visibility: Visibility::Sibling,
            title,
            agent: "build",
            model: None,
        })
        .unwrap()
}

fn reply_permission(h: &Harness, request_id: &str, reply: Reply) {
    let body = ReplyBody {
        reply,
        pattern: None,
        message: None,
    };
    h.engine.permissions.reply(&h.engine.hub, request_id, body).unwrap();
}

fn rule(h: &Harness, kind: &str, pattern: &str, decision: Decision) {
    let rule = Rule {
        kind: kind.into(),
        pattern: pattern.into(),
        decision,
    };
    h.engine.permissions.set_policy(Policy { rules: vec![rule] });
}

/// Default permissions need no approval; tests of the approval flow explicitly request one.
pub(crate) fn asks_for(h: &Harness, kind: &str) {
    rule(h, kind, "*", Decision::Ask);
}

async fn next_ask(events: &mut tokio::sync::broadcast::Receiver<crate::event::Envelope>) -> crate::permission::Request {
    loop {
        let envelope = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("an ask")
            .unwrap();
        if let Event::PermissionAsked { request } = envelope.event {
            return request;
        }
    }
}

/// A call's chunks without the reply's stop, so more can follow it.
fn call_block(id: &str, name: &str, input: &str) -> Vec<Chunk> {
    vec![
        Chunk::ToolUseStart {
            id: id.into(),
            name: name.into(),
        },
        Chunk::ToolInputDelta(input.into()),
        Chunk::BlockStop,
    ]
}

pub(crate) fn tool(row: &PartRow) -> ToolView<'_> {
    let Part::ToolCall {
        name,
        status,
        output,
        title,
        input,
        metadata,
        ..
    } = &row.part
    else {
        panic!("{:?}", row.part)
    };

    ToolView {
        name,
        status: *status,
        output: output.as_deref(),
        title: title.as_deref(),
        input,
        metadata: metadata.as_deref(),
    }
}

fn transcript(h: &Harness) -> Vec<MessageWithParts> {
    h.engine.store.transcript(&h.session.id).unwrap()
}

fn session(h: &Harness) -> Session {
    h.engine.store.session(&h.session.id).unwrap().unwrap()
}

fn reopen(h: &Harness) -> Arc<Engine> {
    let engine = Engine::open_with(
        &h._dir.join("data"),
        crate::Options {
            file_credentials: true,
            ..Default::default()
        },
    )
    .unwrap();
    *engine.turns.provider_override.lock().unwrap() = Some(Provider::Scripted(h.provider.clone()));

    engine
}

fn texts_sent(request: &Request) -> Vec<String> {
    request
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| match block {
            llm::Block::Text(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

async fn until_call_running(h: &Harness) {
    for _ in 0..400 {
        let running = transcript(h).iter().flat_map(|message| &message.parts).any(|row| {
            matches!(
                row.part,
                Part::ToolCall {
                    status: ToolStatus::Running,
                    ..
                }
            )
        });
        if running {
            return;
        }

        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("no call started");
}

/// An overload that asks for a long wait, so a test can act while the turn waits.
fn overloaded() -> llm::Error {
    asking_to_wait(Duration::from_secs(30))
}

fn asking_to_wait(wait: Duration) -> llm::Error {
    let llm::Error::Api {
        status,
        kind,
        message,
        retryable,
        ..
    } = llm::Error::api(529, "overloaded_error", "busy")
    else {
        unreachable!()
    };

    llm::Error::Api {
        status,
        kind,
        message,
        retryable,
        retry_after: Some(wait),
    }
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

/// A turn busy with its first step while `switch` is sent; returns every request it made.
async fn switched_mid_turn(h: &Harness, switch: Prompt) -> Vec<Request> {
    h.provider
        .push_slow(
            Duration::from_millis(400),
            tool_call("read", r#"{"path": "missing.txt"}"#),
        )
        .push(text("carried on"));
    h.engine.submit(&h.session.id, prompt("start")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.engine
        .submit(&h.session.id, switch)
        .await
        .expect("taken by the running turn");
    until_idle(h).await;

    h.provider.requests.lock().unwrap().clone()
}

fn model_with(output: u64, reasoning: bool) -> Model {
    let mut model = crate::llm::catalog::Catalog::bundled().providers["anthropic"].models["claude-sonnet-4-5"].clone();
    model.limit.output = output;
    model.reasoning = reasoning;

    model
}

fn mutate_model(h: &Harness, change: impl FnOnce(&mut Model)) {
    let mut catalog = h.engine.catalog.write().unwrap();
    let model = catalog
        .providers
        .get_mut("anthropic")
        .unwrap()
        .models
        .get_mut("claude-sonnet-4-5")
        .unwrap();
    change(model);
}

fn limits(h: &Harness, json: &str) {
    std::fs::write(h._dir.join("ws/drift.json"), format!(r#"{{ "limits": {json} }}"#)).unwrap();
}

fn read_a() -> Vec<Chunk> {
    tool_call("read", r#"{"path": "a.txt"}"#)
}

/// For tests of check and formatter behavior, a rule allows the project's commands without approval.
fn allow_edits_and_project_commands(h: &Harness) {
    let allow = |kind: &str| Rule {
        kind: kind.into(),
        pattern: "*".into(),
        decision: Decision::Allow,
    };
    h.engine.permissions.set_policy(Policy {
        rules: vec![allow("edit"), allow("project-commands")],
    });
}

fn two_writes(first: (&str, &str), second: (&str, &str)) -> Vec<Chunk> {
    let call = |id, (path, content): (&str, &str)| {
        call_block(id, "write", &json!({ "path": path, "content": content }).to_string())
    };

    [
        call("toolu_first", first),
        call("toolu_second", second),
        vec![Chunk::Stop(StopReason::ToolUse)],
    ]
    .concat()
}

fn call_outputs(h: &Harness, message: usize) -> Vec<(String, serde_json::Value)> {
    transcript(h)[message]
        .parts
        .iter()
        .filter_map(|row| match &row.part {
            Part::ToolCall { output, metadata, .. } => Some((
                output.clone().unwrap_or_default(),
                serde_json::to_value(metadata).unwrap(),
            )),
            _ => None,
        })
        .collect()
}

fn task_call(messages: &[MessageWithParts]) -> (ToolStatus, String, serde_json::Value) {
    let call = tool(&messages[1].parts[0]);

    (
        call.status,
        call.output.unwrap_or_default().into(),
        serde_json::to_value(call.metadata).unwrap(),
    )
}

fn child_id(h: &Harness) -> String {
    h.engine
        .store
        .lock()
        .query_row("SELECT id FROM session WHERE parent_id = ?1", [&h.session.id], |row| {
            row.get(0)
        })
        .unwrap()
}

fn echo_server() -> crate::mcp::ServerConfig {
    crate::mcp::ServerConfig::Stdio {
        command: "node".into(),
        args: vec![concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/echo-server.cjs").into()],
        env: Default::default(),
        cwd: None,
        timeout_seconds: None,
    }
}
