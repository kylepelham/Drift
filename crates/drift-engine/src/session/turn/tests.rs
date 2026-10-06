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
    // Shell-spawning turns can take seconds on a loaded Windows runner; a passing turn returns at once regardless.
    for _ in 0..1000 {
        if !h.engine.turns.is_running(&h.session.id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("turn never finished");
}

pub(crate) fn prompt(text: &str) -> Prompt {
    Prompt { parts: vec![Part::Text { text: text.into() }], model: Some(model()), variant: None, agent: None, submission_id: None }
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
    assert_eq!(request.tools.len(), 14);
}

#[tokio::test]
async fn steering_uses_the_admitted_agent_and_model_generation() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "a").unwrap();
    h.provider.push_slow(Duration::from_millis(500), tool_call("read", r#"{"path":"a.txt"}"#)).push(text("done"));
    h.engine.submit(&h.session.id, prompt("start")).await.await_ok();
    std::fs::create_dir_all(h._dir.join("ws/.drift/agents")).unwrap();
    std::fs::write(h._dir.join("ws/.drift/agents/late.md"), "---\ndescription: Added mid-turn\n---\nA later agent.").unwrap();
    let late = Prompt { agent: Some("late".into()), ..prompt("late selection") };
    assert!(matches!(h.engine.submit(&h.session.id, late.clone()).await, Err(TurnError::UnknownAgent)));
    let mut new_model = h.engine.catalog.read().unwrap().model("anthropic", "claude-sonnet-4-5").unwrap().clone();
    new_model.id = "new-mid-turn".into();
    h.engine.catalog.write().unwrap().providers.get_mut("anthropic").unwrap().models.insert(new_model.id.clone(), new_model);
    let model = ModelRef { provider: "anthropic".into(), model: "new-mid-turn".into() };
    assert!(matches!(h.engine.submit(&h.session.id, Prompt { model: Some(model), ..prompt("new model") }).await, Err(TurnError::UnknownModel)));
    until_idle(&h).await;
    h.provider.push(text("later turn"));
    h.engine.submit(&h.session.id, late).await.await_ok();
    until_idle(&h).await;
    assert_eq!(h.engine.store.session(&h.session.id).unwrap().unwrap().agent, "late");
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

/// A call's chunks without the reply's stop, so more can follow it.
fn call_block(id: &str, name: &str, input: &str) -> Vec<Chunk> {
    vec![Chunk::ToolUseStart { id: id.into(), name: name.into() }, Chunk::ToolInputDelta(input.into()), Chunk::BlockStop]
}

#[tokio::test]
async fn a_read_starts_while_the_reply_still_streams() {
    let h = harness().await;
    let file = h._dir.join("ws/a.txt");
    std::fs::write(&file, "before\n").unwrap();
    let rest = [call_block("t2", "glob", r#"{"pattern": "*.txt"}"#), vec![Chunk::Stop(StopReason::ToolUse)]].concat();
    h.provider.push_paused(call_block("t1", "read", r#"{"path": "a.txt"}"#), Duration::from_millis(600), rest).push(text("done"));
    h.engine.submit(&h.session.id, prompt("read a")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Written while the reply is still streaming: a read that waited for the reply's end would see it.
    std::fs::write(&file, "after\n").unwrap();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!((*status, output.as_deref()), (ToolStatus::Done, Some("1: before")), "the read ran as soon as its call closed");
    assert!(h.engine.turns.files_for(&h.engine.store, &h.session.id).was_read(&crate::tool::canonical(&file)), "its result reached the model, so it counts as read");
}

#[tokio::test]
async fn an_early_read_of_a_reply_that_fails_counts_for_nothing() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "secret plan\n").unwrap();
    h.provider.push_fail_midway(call_block("t1", "read", r#"{"path": "a.txt"}"#), llm::Error::Transport("connection reset".into()));
    h.engine.submit(&h.session.id, prompt("read a")).await.await_ok();
    until_idle(&h).await;
    let file = crate::tool::canonical(&h._dir.join("ws/a.txt"));
    assert!(!h.engine.turns.files_for(&h.engine.store, &h.session.id).was_read(&file), "the model never saw it, so an edit must still read first");
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, .. } = &transcript[1].parts[0].part else { panic!() };
    assert!(*status != ToolStatus::Done && !output.as_deref().unwrap_or_default().contains("secret"), "{output:?}");
}

#[tokio::test]
async fn permission_denial_is_reported_to_the_model() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "rm *".into(), decision: Decision::Deny }] });
    h.provider.push(tool_call("bash", r#"{"command": "rm -rf /"}"#)).push(text("Understood"));
    h.engine.submit(&h.session.id, prompt("wipe it")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Denied);
    let requests = h.provider.requests.lock().unwrap();
    assert!(matches!(&requests[1].messages[2].blocks[0], llm::Block::ToolResult { is_error: true, content, .. } if content == "A permission rule forbids this call."), "a rule, not the user");
}

/// Workspace edits, shell lines inside the workspace, fetches and MCP calls run without asking by default; tests of the asking itself say so.
pub(crate) fn asks_for(h: &Harness, kind: &str) {
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: kind.into(), pattern: "*".into(), decision: Decision::Ask }] });
}

async fn next_ask(rx: &mut tokio::sync::broadcast::Receiver<crate::event::Envelope>) -> crate::permission::Request {
    loop {
        let envelope = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("an ask").unwrap();
        if let Event::PermissionAsked { request } = envelope.event {
            return request;
        }
    }
}

#[tokio::test]
async fn an_edit_holds_the_previewed_file_while_approval_is_pending() {
    let h = harness().await;
    asks_for(&h, "edit");
    let file = h._dir.join("ws/a.txt");
    std::fs::write(&file, "one\none\n").unwrap();
    h.provider.push(tool_call("read", r#"{"path":"a.txt"}"#)).push(text("read"));
    h.engine.submit(&h.session.id, prompt("read a")).await.await_ok();
    until_idle(&h).await;
    let mut rx = h.engine.hub.attach(None).rx;
    h.provider.push(tool_call("edit", r#"{"path":"a.txt","old_string":"one","new_string":"two","replace_all":true}"#)).push(text("done"));
    h.engine.submit(&h.session.id, prompt("edit a")).await.await_ok();
    let ask = next_ask(&mut rx).await;
    assert!(ask.ask.diff.as_deref().is_some_and(|diff| diff.contains("+two")));
    let other_file = file.clone();
    let other = tokio::spawn(async move {
        let _held = crate::tool::lock::files(std::slice::from_ref(&other_file)).await;
        tokio::fs::read_to_string(other_file).await.unwrap()
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!other.is_finished(), "a competing writer waits until the approved edit is recorded");
    h.engine.permissions.reply(&h.engine.hub, &ask.id, ReplyBody { reply: Reply::Once, pattern: None, message: None }).unwrap();
    until_idle(&h).await;
    assert_eq!(other.await.unwrap(), "two\ntwo\n");
}

#[tokio::test]
async fn explicit_rules_restrict_default_allowed_tools() {
    for (kind, input) in [
        ("read", r#"{"path":"a.txt"}"#),
        ("grep", r#"{"pattern":"secret"}"#),
        ("glob", r#"{"pattern":"*.txt"}"#),
        ("skill", r#"{"name":"private"}"#),
        ("task", r#"{"description":"inspect","prompt":"inspect files","subagent_type":"explore"}"#),
        // Tools that declare no kinds are judged under their own name, so their own rule hides them too.
        ("todowrite", r#"{"todos":[]}"#),
        ("question", r#"{"questions":[]}"#),
    ] {
        let h = harness().await;
        std::fs::write(h._dir.join("ws/a.txt"), "secret contents").unwrap();
        h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: kind.into(), pattern: "*".into(), decision: Decision::Deny }] });
        h.provider.push(tool_call(kind, input)).push(text("done"));
        h.engine.submit(&h.session.id, prompt("inspect")).await.await_ok();
        until_idle(&h).await;
        let offered: Vec<String> = h.provider.requests.lock().unwrap()[0].tools.iter().map(|tool| tool.name.clone()).collect();
        assert!(!offered.contains(&kind.to_string()), "a tool every call of which is denied is not offered: {kind}");
        let transcript = h.engine.store.transcript(&h.session.id).unwrap();
        let Part::ToolCall { status, .. } = &transcript[1].parts[0].part else { panic!() };
        assert_ne!(*status, ToolStatus::Done, "and a call to it anyway does not run: {kind}");
        assert_eq!(h.engine.store.session_tree(&h.session.id).unwrap().len(), 1, "no task starts when delegation is denied");
    }
}

#[test]
fn a_tool_is_denied_outright_only_when_nothing_before_the_blanket_rule_lets_a_call_through() {
    let rules = |list: &[(&str, &str, Decision)]| Policy { rules: list.iter().map(|(kind, pattern, decision)| Rule { kind: kind.to_string(), pattern: pattern.to_string(), decision: *decision }).collect() };
    let compiled = |list: &[(&str, &str, Decision)]| crate::permission::Compiled::new(rules(list).rules);
    assert!(compiled(&[("edit", "*", Decision::Deny)]).denies_all("edit"), "opencode's edit: deny");
    assert!(compiled(&[("*", "*", Decision::Deny)]).denies_all("bash"));
    assert!(compiled(&[("bash", "rm *", Decision::Deny), ("bash", "*", Decision::Deny)]).denies_all("bash"));
    assert!(!compiled(&[("bash", "git *", Decision::Allow), ("bash", "*", Decision::Deny)]).denies_all("bash"), "git still runs");
    assert!(!compiled(&[("bash", "*", Decision::Ask)]).denies_all("bash"));
    assert!(!compiled(&[("bash", "rm *", Decision::Deny)]).denies_all("bash"), "only part of it");
}

#[tokio::test]
async fn agent_permissions_and_default_variant_are_applied_to_the_turn() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "private contents").unwrap();
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind:"read".into(), pattern:"a.txt".into(), decision:Decision::Ask }] });
    let mut rx = h.engine.hub.attach(None).rx;
    h.provider.push(tool_call("read", r#"{"path":"a.txt"}"#)).push(text("read"));
    h.engine.submit(&h.session.id, prompt("read")).await.await_ok();
    let ask = next_ask(&mut rx).await;
    h.engine.permissions.reply(&h.engine.hub, &ask.id, ReplyBody { reply: Reply::Always, pattern:None, message:None }).unwrap();
    until_idle(&h).await;
    h.engine.set_agent_overrides(std::collections::HashMap::from([("build".into(), crate::config::AgentOverride::from_json(&json!({
        "permissions":[{"kind":"read","pattern":"a.txt","decision":"deny"}], "variant":"high"
    })))]));
    h.provider.push(tool_call("read", r#"{"path":"a.txt"}"#)).push(text("denied"));
    h.engine.submit(&h.session.id, prompt("read under restricted agent")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert!(transcript.iter().flat_map(|m| &m.parts).any(|row| matches!(&row.part, Part::ToolCall { status:ToolStatus::Denied,.. })), "agent denial overrides an earlier always grant");
    assert!(h.provider.requests.lock().unwrap().last().unwrap().reasoning.is_some(), "the configured default variant reaches the provider");
    h.provider.push(text("low"));
    h.engine.submit(&h.session.id, Prompt { variant:Some(Some("low".into())), ..prompt("explicit level") }).await.await_ok();
    until_idle(&h).await;
    assert_eq!(h.engine.store.session(&h.session.id).unwrap().unwrap().variant.as_deref(), Some("low"));
}

#[tokio::test]
async fn ordinary_reads_allow_by_default_but_explicit_ask_requires_approval() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "read me").unwrap();
    h.provider.push(tool_call("read", r#"{"path":"a.txt"}"#)).push(text("done"));
    h.engine.submit(&h.session.id, prompt("read")).await.await_ok();
    until_idle(&h).await;
    assert!(h.engine.permissions.pending().is_empty());
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "read".into(), pattern: "a.txt".into(), decision: Decision::Ask }] });
    let mut rx = h.engine.hub.attach(None).rx;
    h.provider.push(tool_call("read", r#"{"path":"a.txt"}"#)).push(text("done"));
    h.engine.submit(&h.session.id, prompt("read again")).await.await_ok();
    let ask = next_ask(&mut rx).await;
    assert_eq!(ask.ask.kind, "read");
    h.engine.permissions.reply(&h.engine.hub, &ask.id, ReplyBody { reply: Reply::Once, pattern: None, message: None }).unwrap();
    until_idle(&h).await;
}

#[tokio::test]
async fn a_refusal_tells_the_model_what_the_user_said_and_the_turn_goes_on() {
    let h = harness().await;
    asks_for(&h, "bash");
    let mut rx = h.engine.hub.attach(None).rx;
    h.provider.push(tool_call("bash", r#"{"command": "rm -rf build"}"#)).push(text("Using cargo clean instead"));
    h.engine.submit(&h.session.id, prompt("clean up")).await.await_ok();
    let ask = next_ask(&mut rx).await;
    let body = ReplyBody { reply: Reply::Deny, pattern: None, message: Some("use cargo clean".into()) };
    h.engine.permissions.reply(&h.engine.hub, &ask.id, body).unwrap();
    until_idle(&h).await;
    let requests = h.provider.requests.lock().unwrap().clone();
    let result = requests[1].messages.iter().flat_map(|m| &m.blocks).find_map(|b| match b { llm::Block::ToolResult { content, .. } => Some(content.clone()), _ => None }).unwrap();
    assert_eq!(result, "The user denied permission for this call. They said: use cargo clean");
}

#[tokio::test]
async fn deny_and_stop_ends_the_turn() {
    let h = harness().await;
    asks_for(&h, "bash");
    let mut rx = h.engine.hub.attach(None).rx;
    h.provider.push(tool_call("bash", r#"{"command": "rm -rf build"}"#)).push(text("never asked"));
    h.engine.submit(&h.session.id, prompt("clean up")).await.await_ok();
    let ask = next_ask(&mut rx).await;
    h.engine.permissions.reply(&h.engine.hub, &ask.id, ReplyBody { reply: Reply::Stop, pattern: None, message: None }).unwrap();
    until_idle(&h).await;
    assert_eq!(h.provider.requests.lock().unwrap().len(), 1, "no request after the stop");
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Denied);
    assert!(output.as_deref().unwrap().contains("stopped the turn"));
}

#[tokio::test]
async fn a_stop_mid_batch_leaves_no_call_pending() {
    let h = harness().await;
    asks_for(&h, "bash");
    let mut rx = h.engine.hub.attach(None).rx;
    let call = |id: &str, command: &str| vec![Chunk::ToolUseStart { id: id.into(), name: "bash".into() }, Chunk::ToolInputDelta(format!(r#"{{"command": "{command}"}}"#)), Chunk::BlockStop];
    h.provider.push([call("t1", "rm -rf build"), call("t2", "rm -rf dist"), vec![Chunk::Stop(StopReason::ToolUse)]].concat());
    h.engine.submit(&h.session.id, prompt("clean up")).await.await_ok();
    let ask = next_ask(&mut rx).await;
    h.engine.permissions.reply(&h.engine.hub, &ask.id, ReplyBody { reply: Reply::Stop, pattern: None, message: None }).unwrap();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, .. } = &transcript[1].parts[1].part else { panic!() };
    assert_eq!(*status, ToolStatus::Error, "the call queued behind the stop is closed, not left pending");
    assert!(output.as_deref().unwrap().starts_with("Not run: the turn was stopped"), "{output:?}");
}

#[tokio::test]
async fn a_refused_reply_says_so_and_runs_nothing() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    h.provider.push(vec![Chunk::ToolUseStart { id: "t1".into(), name: "read".into() }, Chunk::ToolInputDelta(r#"{"path": "a.txt"}"#.into()), Chunk::BlockStop, Chunk::Stop(StopReason::Refused)]);
    h.engine.submit(&h.session.id, prompt("read")).await.await_ok();
    until_idle(&h).await;
    let last = h.engine.store.transcript(&h.session.id).unwrap().pop().unwrap();
    assert_eq!((last.info.status, last.info.error.as_deref()), (MessageStatus::Done, Some(super::REFUSED_ENDING)), "never a silent end");
    assert_eq!(last.info.ending, Some(crate::session::types::Ending::Refused), "typed, so the UI never reads the wording");
    let Part::ToolCall { status, .. } = &last.parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Error);
}

#[tokio::test]
async fn a_reply_that_fills_the_context_window_compacts_and_asks_again() {
    let h = harness().await;
    h.provider
        .push(vec![Chunk::TextStart, Chunk::TextDelta("half an ans".into()), Chunk::BlockStop, Chunk::Stop(StopReason::ContextFull)])
        .push(text("SUMMARY"))
        .push(text("the whole answer"));
    h.engine.submit(&h.session.id, prompt("long")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert!(transcript.iter().any(|m| m.info.summary), "compacted");
    assert_eq!(transcript.last().unwrap().parts.iter().find_map(|p| match &p.part { Part::Text { text } => Some(text.as_str()), _ => None }), Some("the whole answer"));
    let cut = transcript.iter().find(|m| m.parts.iter().any(|p| matches!(&p.part, Part::Text { text } if text == "half an ans"))).unwrap();
    assert_eq!(cut.info.status, MessageStatus::Error, "the cut reply is never replayed");
}

#[tokio::test]
async fn a_patch_in_a_real_turn_keeps_its_display_diff_beside_undos_record() {
    let h = harness().await;
    h.engine.catalog.write().unwrap().providers.get_mut("anthropic").unwrap().models.get_mut("claude-sonnet-4-5").unwrap().profile = crate::llm::catalog::ToolProfile::ApplyPatch;
    let patch = json!({ "patch": "*** Begin Patch\n*** Add File: new.txt\n+fresh\n*** End Patch\n" }).to_string();
    h.provider.push(tool_call("apply_patch", &patch)).push(text("patched"));
    h.engine.submit(&h.session.id, prompt("add new.txt")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, metadata: Some(metadata), output, .. } = &transcript[1].parts[0].part else { panic!("{:?}", transcript[1].parts) };
    assert_eq!(*status, ToolStatus::Done, "{output:?}");
    assert!(metadata["changes"][0]["path"].is_string(), "undo's record: {metadata}");
    assert_eq!(metadata["fileChanges"][0]["relativePath"], "new.txt", "the display record survives undo's merge: {metadata}");
    assert!(metadata["fileChanges"][0]["patch"].as_str().unwrap().contains("+fresh"));
}

#[tokio::test]
async fn always_holds_for_the_workspace_across_sessions_and_restarts_and_settles_asks_it_covers() {
    let h = harness().await;
    asks_for(&h, "bash");
    let mut rx = h.engine.hub.attach(None).rx;
    let other = h.engine.store.create_session(NewSession { workspace_id: &h.session.workspace_id, parent_id: None, visibility: Visibility::Sibling, title: "Other", agent: "build", model: None }).unwrap();
    h.provider.push(tool_call("bash", r#"{"command": "cargo --version"}"#)).push(tool_call("bash", r#"{"command": "cargo --version"}"#)).push(text("one")).push(text("two"));
    h.engine.submit(&h.session.id, prompt("first")).await.await_ok();
    let first = next_ask(&mut rx).await;
    h.engine.submit(&other.id, prompt("second")).await.await_ok();
    let second = next_ask(&mut rx).await;
    assert_ne!(first.session_id, second.session_id);
    h.engine.permissions.reply(&h.engine.hub, &first.id, ReplyBody { reply: Reply::Always, pattern: None, message: None }).unwrap();
    until_idle(&h).await;
    for _ in 0..300 {
        if !h.engine.turns.is_running(&other.id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(h.engine.permissions.pending().is_empty(), "the other session's waiting ask was answered by the same grant");
    assert_eq!(h.provider.responses_left(), 0);
    let reopened = Engine::open_with(&h._dir.join("data"), crate::Options { file_credentials: true, ..Default::default() }).unwrap();
    reopened.bind_permissions("ses_later", &h.session.workspace_id);
    let ask = crate::tool::Ask::shell(crate::tool::command::Dialect::Bash, "cargo --version", "");
    assert_eq!(reopened.permissions.decide_now("ses_later", &Policy::default(), &ask), Decision::Allow, "kept for the workspace across a restart");
    reopened.bind_permissions("ses_elsewhere", "another-workspace");
    assert_eq!(reopened.permissions.decide_now("ses_elsewhere", &Policy::default(), &ask), Decision::Ask, "and only for that workspace");
    let deny = Policy { rules: vec![Rule { kind: "bash".into(), pattern: "cargo *".into(), decision: Decision::Deny }] };
    assert_eq!(reopened.permissions.decide_now("ses_later", &deny, &ask), Decision::Deny, "a deny rule added later beats the kept grant");
    let grants = reopened.permission_grants(&h.session.workspace_id);
    assert_eq!(grants.len(), 1, "{grants:?}");
    assert!(reopened.revoke_permission_grant(&h.session.workspace_id, Some(&grants[0])));
    assert!(!reopened.revoke_permission_grant(&h.session.workspace_id, Some(&grants[0])), "already gone");
    assert_eq!(reopened.permissions.decide_now("ses_later", &Policy::default(), &ask), Decision::Ask, "revoked, it asks again");
    let after_restart = Engine::open_with(&h._dir.join("data"), crate::Options { file_credentials: true, ..Default::default() }).unwrap();
    assert!(after_restart.permission_grants(&h.session.workspace_id).is_empty(), "the revoke was stored");
}

#[tokio::test]
async fn a_subagent_runs_under_its_parents_approvals() {
    let h = harness().await;
    asks_for(&h, "bash");
    let mut rx = h.engine.hub.attach(None).rx;
    h.provider
        .push(tool_call("bash", r#"{"command": "cargo --version"}"#))
        .push(tool_call("task", r#"{"description": "Check", "prompt": "check the toolchain"}"#))
        .push(tool_call("bash", r#"{"command": "cargo --version"}"#))
        .push(text("child done"))
        .push(text("parent done"));
    h.engine.submit(&h.session.id, prompt("check")).await.await_ok();
    let ask = next_ask(&mut rx).await;
    h.engine.permissions.reply(&h.engine.hub, &ask.id, ReplyBody { reply: Reply::Always, pattern: None, message: None }).unwrap();
    until_idle(&h).await;
    assert_eq!(h.provider.responses_left(), 0, "the child's same command ran without asking");
    assert!(h.engine.permissions.pending().is_empty());
}

#[tokio::test]
async fn asks_wait_for_a_reply_and_mutations_snapshot_first() {
    let h = harness().await;
    asks_for(&h, "edit");
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
    h.engine.permissions.reply(&h.engine.hub, &request.id, ReplyBody { reply: Reply::Once, pattern: None, message: None }).unwrap();
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
    assert!(h.engine.switch_retry_model(&h.session.id, &ModelRef { provider: "anthropic".into(), model: "claude-sonnet-4-5".into() }, None).await.is_ok());
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
    assert_eq!(h.engine.switch_retry_model(&h.session.id, &other, None).await, Err(TurnError::NotRetrying), "nothing is waiting yet");

    h.provider.push_error(overloaded()).push(text("answered by the other model"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_waiting_to_retry(&h).await;
    let unusable = ModelRef { provider: "openai".into(), model: "gpt-5".into() };
    assert_eq!(h.engine.switch_retry_model(&h.session.id, &unusable, None).await, Err(TurnError::NoCredentials), "a model without a credential is refused up front");
    let started = std::time::Instant::now();
    h.engine.switch_retry_model(&h.session.id, &other, Some(Some("high".into()))).await.unwrap();
    until_idle(&h).await;
    assert!(started.elapsed() < Duration::from_millis(900), "the switch retries at once instead of waiting out the backoff");
    let requests = h.provider.requests.lock().unwrap().clone();
    assert_eq!((requests[0].model.as_str(), requests[1].model.as_str()), ("claude-sonnet-4-5", pinned.as_str()));
    let session = h.engine.store.session(&h.session.id).unwrap().unwrap();
    assert_eq!(session.model, Some(other), "the session keeps the model it was switched to");
    assert_eq!(session.variant.as_deref(), Some("high"), "and the variant chosen with it");
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

    h.provider.push_error(llm::Error::Unauthenticated("invalid x-api-key".into()));
    h.engine.submit(&h.session.id, prompt("again")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert_eq!(transcript.len(), 5, "a refused key is not retried");
    assert_eq!(transcript[4].info.status, MessageStatus::Error);
    assert_eq!(transcript[4].info.error.as_deref(), Some("the provider refused the credentials: invalid x-api-key"));
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
    let no_model = Prompt { parts: vec![], model: None, variant: None, agent: None, submission_id: None };
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

/// One fake token endpoint at a time, since they share `DRIFT_ANTHROPIC_TOKEN_URL`.
static TOKEN_ENDPOINT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A slow Anthropic token endpoint answering every refresh with `status` and `body`; counts the refreshes.
async fn token_endpoint(status: u16, body: serde_json::Value) -> (tokio::sync::MutexGuard<'static, ()>, Arc<std::sync::atomic::AtomicUsize>) {
    let held = TOKEN_ENDPOINT.lock().await;
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = hits.clone();
    let app = axum::Router::new().route(
        "/token",
        axum::routing::post(move || {
            let (counter, body) = (counter.clone(), body.clone());
            async move {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(100)).await;
                (axum::http::StatusCode::from_u16(status).unwrap(), axum::Json(body))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    std::env::set_var("DRIFT_ANTHROPIC_TOKEN_URL", format!("http://{}/token", listener.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (held, hits)
}

fn signed_in(h: &Harness, access: &str) {
    let live = Credential::OAuth { access: access.into(), refresh: "r1".into(), expires_at: crate::id::now_ms() + 3_600_000, account: None };
    h.engine.credentials.set("anthropic", &live).unwrap();
}

#[tokio::test]
async fn a_refused_sign_in_is_renewed_once_and_the_request_sent_again() {
    use std::sync::atomic::Ordering;
    let (_held, hits) = token_endpoint(200, json!({ "access_token": "fresh", "refresh_token": "r2", "expires_in": 3600 })).await;
    let h = harness().await;
    signed_in(&h, "revoked");
    h.provider.push_error(llm::Error::Unauthenticated("OAuth token has expired.".into())).push(text("a")).push(text("b"));
    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;
    h.engine.submit(&h.session.id, prompt("two")).await.await_ok();
    until_idle(&h).await;
    std::env::remove_var("DRIFT_ANTHROPIC_TOKEN_URL");
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert_eq!(transcript.len(), 4, "the refusal leaves no failed reply behind");
    assert!(transcript.iter().all(|m| m.info.error.is_none()));
    assert_eq!(hits.load(Ordering::SeqCst), 1, "renewed once; the next turn uses the stored token");
    assert!(matches!(h.engine.credentials.get("anthropic").unwrap(), Credential::OAuth { access, .. } if access == "fresh"));
}

#[tokio::test]
async fn a_sign_in_that_cannot_be_renewed_says_it_expired() {
    let (_held, _) = token_endpoint(400, json!({ "error": "invalid_grant", "error_description": "Refresh token revoked" })).await;
    let h = harness().await;
    signed_in(&h, "revoked");
    h.provider.push_error(llm::Error::Unauthenticated("OAuth token has expired.".into()));
    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;
    std::env::remove_var("DRIFT_ANTHROPIC_TOKEN_URL");
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let error = transcript[1].info.error.clone().unwrap();
    assert!(error.starts_with("the provider refused the credentials: OAuth token has expired."), "{error}");
    assert!(error.contains("the sign-in has expired and could not be renewed; sign in again under Settings > Providers"), "{error}");
    assert!(!error.contains("no credentials"), "{error}");
}

#[tokio::test]
async fn concurrent_turns_refresh_an_expired_token_once() {
    use std::sync::atomic::Ordering;
    let (_held, hits) = token_endpoint(200, json!({ "access_token": "fresh", "refresh_token": "r2", "expires_in": 3600 })).await;
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
async fn an_edit_waits_while_another_writer_holds_the_file() {
    let h = harness().await;
    let file = h._dir.join("ws/a.txt");
    std::fs::write(&file, "one\ntwo\n").unwrap();
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "edit".into(), pattern: "*".into(), decision: Decision::Allow }] });
    h.provider.push(tool_call("read", r#"{"path": "a.txt"}"#)).push(tool_call("edit", r#"{"path": "a.txt", "old_string": "one", "new_string": "ONE"}"#)).push(text("done"));
    let held = crate::tool::lock::files(std::slice::from_ref(&file)).await;
    h.engine.submit(&h.session.id, prompt("edit a")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Another session's writer, holding the file, changes another line meanwhile.
    std::fs::write(&file, "one\nTWO\n").unwrap();
    assert!(h.engine.turns.is_running(&h.session.id), "the edit waits its turn");
    drop(held);
    until_idle(&h).await;
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "ONE\nTWO\n", "it starts from the other writer's bytes, so neither change is lost");
}

#[tokio::test]
async fn a_file_read_before_a_restart_may_be_edited_after_it() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "one\n").unwrap();
    h.provider.push(tool_call("read", r#"{"path": "a.txt"}"#)).push(text("read it"));
    h.engine.submit(&h.session.id, prompt("read a")).await.await_ok();
    until_idle(&h).await;

    let reopened = Engine::open_with(&h._dir.join("data"), crate::Options { file_credentials: true, ..Default::default() }).unwrap();
    *reopened.turns.provider_override.lock().unwrap() = Some(Provider::Scripted(h.provider.clone()));
    reopened.permissions.set_policy(Policy { rules: vec![Rule { kind: "edit".into(), pattern: "*".into(), decision: Decision::Allow }] });
    h.provider.push(tool_call("edit", r#"{"path": "a.txt", "old_string": "one", "new_string": "two"}"#)).push(text("edited"));
    reopened.submit(&h.session.id, prompt("edit a")).await.await_ok();
    for _ in 0..500 {
        if !reopened.turns.is_running(&h.session.id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(std::fs::read_to_string(h._dir.join("ws/a.txt")).unwrap(), "two\n", "the read was kept across the restart");
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
async fn a_command_whose_tree_cannot_be_captured_still_runs_and_says_so() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Allow }] });
    // A file where the snapshot directory must go makes every capture fail, as a whole drive would.
    std::fs::write(h._dir.join("data/snapshots"), "not a directory").unwrap();
    h.provider.push(tool_call("bash", &json!({ "command": "touch made.txt" }).to_string())).push(text("noted"));
    h.engine.submit(&h.session.id, prompt("write")).await.await_ok();
    until_idle(&h).await;
    assert!(h._dir.join("ws/made.txt").exists(), "a shell's tree is only observed, so a missing record does not stop it");
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, metadata, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Done);
    assert!(output.as_deref().unwrap().contains("could not record what this command changed"), "{output:?}");
    assert!(metadata.as_ref().unwrap()["historyError"].is_string());
    let note = metadata.as_ref().unwrap()["historyError"].as_str().unwrap();
    assert_eq!(metadata.as_ref().unwrap()["notes"], json!([note]), "listed apart, so the UI shows it under the call");
}

#[tokio::test]
async fn stop_ends_a_capture_that_has_not_finished() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Allow }] });
    let workspace = h._dir.join("ws");
    h.engine.snapshots.bind(&h.session.workspace_id, &workspace);
    // Another capture of this workspace holding its index stands in for one that takes minutes.
    let lock = h.engine.snapshots.lock_for(&workspace);
    let held = lock.lock().await;
    h.provider.push(tool_call("bash", &json!({ "command": "touch made.txt" }).to_string())).push(text("unused"));
    h.engine.submit(&h.session.id, prompt("write")).await.await_ok();
    for _ in 0..400 {
        if h.engine.store.transcript(&h.session.id).unwrap().iter().flat_map(|m| &m.parts).any(|row| matches!(row.part, Part::ToolCall { .. })) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(h.engine.abort(&h.session.id));
    until_idle(&h).await;
    drop(held);
    assert!(!h._dir.join("ws/made.txt").exists());
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!((*status, output.as_deref()), (ToolStatus::Error, Some("Aborted while recording the files first.")));
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
async fn a_tool_named_in_the_wrong_case_runs_as_the_offered_tool() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "alpha\n").unwrap();
    h.provider.push(tool_call("Read", r#"{"path": "a.txt"}"#)).push(text("ok"));
    h.engine.submit(&h.session.id, prompt("read")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { name, status, output, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!((name.as_str(), *status), ("read", ToolStatus::Done), "{output:?}");
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
    assert!(output.as_deref().unwrap().contains("not valid JSON (EOF while parsing"), "the parser's complaint is quoted: {output:?}");
    let next = h.provider.requests.lock().unwrap()[1].clone();
    let replayed: Vec<&crate::llm::Block> = next.messages.iter().flat_map(|m| &m.blocks).filter(|b| matches!(b, crate::llm::Block::ToolUse { .. } | crate::llm::Block::ToolResult { .. })).collect();
    assert!(matches!(replayed[..], [crate::llm::Block::ToolUse { input, .. }, crate::llm::Block::ToolResult { is_error: true, .. }] if *input == serde_json::json!({})), "the model sees its broken call and why: {replayed:?}");

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
async fn arguments_that_do_not_fit_the_schema_are_refused_before_the_call_runs() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    h.provider.push(tool_call("read", r#"{"path": "a.txt", "limit": "20"}"#)).push(text("ok"));
    h.engine.submit(&h.session.id, prompt("read")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Error);
    assert!(output.as_deref().unwrap().contains("`limit` must be integer, not a string"), "{output:?}");
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
    assert_eq!(last.info.ending, Some(crate::session::types::Ending::Length));
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

#[tokio::test]
async fn a_conversation_holding_parts_this_build_cannot_read_still_runs_and_never_sends_them() {
    let h = harness().await;
    h.provider.push(text("first answer")).push(text("second answer"));
    h.engine.submit(&h.session.id, prompt("first")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    for (n, message) in transcript.iter().enumerate() {
        h.engine.store.lock().execute(
            "INSERT INTO part(id, message_id, session_id, json) VALUES(?1, ?2, ?3, ?4)",
            rusqlite::params![format!("prt_z{n}"), message.info.id, h.session.id, r#"{"type":"snapshot","snapshot":"SECRET_HASH"}"#],
        ).unwrap();
    }
    h.engine.submit(&h.session.id, prompt("second")).await.await_ok();
    until_idle(&h).await;
    let sent = texts_sent(&h.provider.requests.lock().unwrap()[1]);
    assert_eq!(sent, ["first", "first answer", "second"], "the rest of the history goes as before");
    assert_eq!(h.engine.store.transcript(&h.session.id).unwrap().len(), 4);
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

fn mention(path: &std::path::Path) -> Part {
    let path = path.to_string_lossy().replace('\\', "/");
    let url = if path.starts_with('/') { format!("file://{path}") } else { format!("file:///{path}") };
    Part::File { mime: "text/plain".into(), name: "mention".into(), url, path: None }
}

fn with_files(text: &str, files: Vec<Part>) -> Prompt {
    let mut prompt = prompt(text);
    prompt.parts.extend(files);
    prompt
}

async fn sent_text(h: &Harness, prompt: Prompt) -> String {
    h.provider.push(text("ok"));
    h.engine.submit(&h.session.id, prompt).await.await_ok();
    until_idle(h).await;
    texts_sent(h.provider.requests.lock().unwrap().last().unwrap()).join("\n")
}

#[tokio::test]
async fn a_mentioned_workspace_file_is_read_into_the_prompt() {
    let h = harness().await;
    let ws = h._dir.join("ws");
    std::fs::write(ws.join("notes.md"), "remember the milk\n").unwrap();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("src/lib.rs"), "").unwrap();
    let sent = sent_text(&h, with_files("see @notes.md and @src", vec![mention(&ws.join("notes.md")), mention(&ws.join("src"))])).await;
    assert!(sent.contains("<file path=\"notes.md\">\nremember the milk"), "{sent}");
    assert!(sent.contains("<file path=\"src\">\nlib.rs"), "a directory lists its entries: {sent}");
    let stored: Vec<Option<String>> = h.engine.store.transcript(&h.session.id).unwrap()[0].parts.iter().filter_map(|row| match &row.part { Part::File { path, .. } => Some(path.clone()), _ => None }).collect();
    assert_eq!(stored, [Some("notes.md".to_string()), Some("src".to_string())], "a mention remembers the file it was, for the client to open");
    let forged = Part::File { mime: "text/plain".into(), name: "x".into(), url: "data:text/plain;base64,eA==".into(), path: Some("../secret".into()) };
    h.provider.push(text("ok"));
    h.engine.submit(&h.session.id, with_files("pasted", vec![forged])).await.await_ok();
    until_idle(&h).await;
    let last_prompt = h.engine.store.transcript(&h.session.id).unwrap().into_iter().rev().find(|m| m.info.role == Role::User).unwrap();
    assert!(last_prompt.parts.iter().all(|row| !matches!(&row.part, Part::File { path: Some(_), .. })), "a client cannot claim a part is a mention");
}

#[tokio::test]
async fn a_mention_counts_as_read_only_once_its_prompt_is_admitted() {
    let h = harness().await;
    let notes = h._dir.join("ws/notes.md");
    std::fs::write(&notes, "milk\n").unwrap();
    let audio = Part::File { mime: "audio/wav".into(), name: "memo.wav".into(), url: "data:audio/wav;base64,UklGRg==".into(), path: None };
    assert!(h.engine.submit(&h.session.id, with_files("see", vec![mention(&notes), audio])).await.is_err());
    assert!(h.engine.store.read_files(&h.session.id).unwrap().is_empty(), "a refused prompt showed the model nothing");
    sent_text(&h, with_files("see", vec![mention(&notes)])).await;
    assert_eq!(h.engine.store.read_files(&h.session.id).unwrap().len(), 1);
}

#[tokio::test]
async fn a_mentioned_secret_or_outside_file_is_not_read_without_a_rule() {
    let h = harness().await;
    let ws = h._dir.join("ws");
    std::fs::write(ws.join(".env"), "API_KEY=hunter2\n").unwrap();
    let outside = h._dir.join("outside.txt");
    std::fs::write(&outside, "far away\n").unwrap();
    let sent = sent_text(&h, with_files("look", vec![mention(&ws.join(".env")), mention(&outside)])).await;
    assert!(!sent.contains("hunter2") && !sent.contains("far away"), "{sent}");
    assert!(sent.contains("@.env was mentioned but not read: it may hold secrets. Use the read tool"), "{sent}");
    assert!(sent.contains("it is outside the workspace"), "{sent}");

    let resolved = crate::tool::canonical(&outside).to_string_lossy().into_owned();
    std::fs::write(ws.join("drift.json"), serde_json::json!({ "permissions": [{ "kind": "read", "pattern": resolved, "decision": "allow" }] }).to_string()).unwrap();
    let allowed = sent_text(&h, with_files("again", vec![mention(&outside)])).await;
    assert!(allowed.contains("far away"), "a rule that allows the read lets the mention in: {allowed}");
}

#[tokio::test]
async fn files_a_model_cannot_take_are_refused_not_dropped() {
    let h = harness().await;
    let image = Part::File { mime: "image/png".into(), name: "shot.png".into(), url: "data:image/png;base64,iVBORw0KGgo=".into(), path: None };
    h.engine.catalog.write().unwrap().providers.get_mut("anthropic").unwrap().models.get_mut("claude-sonnet-4-5").unwrap().attachment = false;
    let refused = h.engine.submit(&h.session.id, with_files("look", vec![image.clone()])).await.unwrap_err();
    assert!(matches!(&refused, TurnError::Attachment(m) if m.contains("cannot read images") && m.contains("shot.png")), "{refused:?}");
    assert!(!h.engine.turns.is_running(&h.session.id), "a refused prompt leaves the session free");
    let audio = Part::File { mime: "audio/wav".into(), name: "memo.wav".into(), url: "data:audio/wav;base64,UklGRg==".into(), path: None };
    assert!(matches!(h.engine.submit(&h.session.id, with_files("hear", vec![audio])).await, Err(TurnError::Attachment(_))));
    let remote = Part::File { mime: "text/plain".into(), name: "remote".into(), url: "https://example.com/a.txt".into(), path: None };
    assert!(matches!(h.engine.submit(&h.session.id, with_files("fetch", vec![remote])).await, Err(TurnError::Attachment(_))));
    assert!(h.provider.requests.lock().unwrap().is_empty());

    let note = Part::File { mime: "text/plain".into(), name: "note.txt".into(), url: "data:text/plain;base64,aGVsbG8gdGhlcmU=".into(), path: None };
    assert!(sent_text(&h, with_files("read this", vec![note])).await.contains("hello there"), "text travels as text");
}

#[tokio::test]
async fn a_pdf_goes_whole_to_a_model_that_reads_pdfs_and_is_refused_by_one_that_does_not() {
    let h = harness().await;
    let pdf = Part::File { mime: "application/pdf".into(), name: "spec.pdf".into(), url: "data:application/pdf;base64,JVBERi0xLjcK".into(), path: None };
    let set_pdf = |reads: bool| h.engine.catalog.write().unwrap().providers.get_mut("anthropic").unwrap().models.get_mut("claude-sonnet-4-5").unwrap().pdf = reads;
    set_pdf(false);
    let refused = h.engine.submit(&h.session.id, with_files("read", vec![pdf.clone()])).await.unwrap_err();
    assert!(matches!(&refused, TurnError::Attachment(m) if m.contains("cannot read PDFs") && m.contains("spec.pdf")), "{refused:?}");
    let fake = Part::File { mime: "application/pdf".into(), name: "spec.pdf".into(), url: "data:application/pdf;base64,aGVsbG8=".into(), path: None };
    set_pdf(true);
    assert!(matches!(h.engine.submit(&h.session.id, with_files("read", vec![fake])).await, Err(TurnError::Attachment(m)) if m.contains("not one")));
    h.provider.push(text("read it"));
    h.engine.submit(&h.session.id, with_files("read", vec![pdf])).await.await_ok();
    until_idle(&h).await;
    let request = h.provider.requests.lock().unwrap().last().unwrap().clone();
    assert!(request.messages[0].blocks.iter().any(|block| matches!(block, crate::llm::Block::Pdf { base64 } if base64 == "JVBERi0xLjcK")), "{:?}", request.messages[0].blocks);
}

#[tokio::test]
async fn malformed_attachments_are_refused_before_admission() {
    let h = harness().await;
    for (mime, url, why) in [
        ("text/plain", "data:text/plain;base64,@@not base64@@", "could not be decoded"),
        ("text/plain", "data:text/plain;base64,/w==", "could not be decoded"),
        ("image/png", "data:image/png;base64,***", "not valid base64"),
        ("image/png", "data:image/png,rawbytes", "not valid base64"),
        ("image/png", "data:image/jpeg;base64,iVBORw0KGgo=", "its data is image/jpeg"),
    ] {
        let part = Part::File { mime: mime.into(), name: "bad".into(), url: url.into(), path: None };
        let refused = h.engine.submit(&h.session.id, with_files("look", vec![part])).await.unwrap_err();
        assert!(matches!(&refused, TurnError::Attachment(m) if m.contains(why)), "{url}: {refused:?}");
    }
    assert!(h.engine.store.transcript(&h.session.id).unwrap().is_empty(), "nothing was admitted");
    assert!(h.provider.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_file_read_in_through_a_mention_can_be_edited_straight_away() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "edit".into(), pattern: "*".into(), decision: Decision::Allow }] });
    let ws = h._dir.join("ws");
    std::fs::write(ws.join("notes.md"), "old line\n").unwrap();
    h.provider.push(tool_call("edit", r#"{"path": "notes.md", "old_string": "old line", "new_string": "new line"}"#)).push(text("edited"));
    h.engine.submit(&h.session.id, with_files("fix @notes.md", vec![mention(&ws.join("notes.md"))])).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Done, "{output:?}");
    assert_eq!(std::fs::read_to_string(ws.join("notes.md")).unwrap(), "new line\n");
}

#[tokio::test]
async fn a_steered_image_is_judged_against_the_model_the_next_request_runs_on() {
    let h = harness().await;
    let other = {
        let mut catalog = h.engine.catalog.write().unwrap();
        let models = &mut catalog.providers.get_mut("anthropic").unwrap().models;
        models.get_mut("claude-sonnet-4-5").unwrap().attachment = false;
        models.values().find(|m| m.id != "claude-sonnet-4-5" && m.attachment).unwrap().id.clone()
    };
    h.provider.push_slow(Duration::from_millis(600), text("done")).push(text("seen it"));
    h.engine.submit(&h.session.id, prompt("slow")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let image = Part::File { mime: "image/png".into(), name: "shot.png".into(), url: "data:image/png;base64,iVBORw0KGgo=".into(), path: None };
    let mut steered = with_files("look at this", vec![image.clone()]);
    steered.model = None;
    let refused = h.engine.submit(&h.session.id, steered).await.unwrap_err();
    assert!(matches!(&refused, TurnError::Attachment(m) if m.contains("cannot read images")), "the running model decides: {refused:?}");
    let mut elsewhere = with_files("look at this", vec![image]);
    elsewhere.model = Some(ModelRef { provider: "anthropic".into(), model: other.clone() });
    h.engine.submit(&h.session.id, elsewhere).await.expect("the model it switches to reads images");
    until_idle(&h).await;
    assert_eq!(h.provider.requests.lock().unwrap().last().unwrap().model, other, "answered on the model it named");
}

#[tokio::test]
async fn a_call_id_the_provider_repeats_is_renamed_so_every_call_keeps_its_own() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    std::fs::write(h._dir.join("ws/b.txt"), "b\n").unwrap();
    let call = |path: &str| vec![Chunk::ToolUseStart { id: "functions.read:0".into(), name: "read".into() }, Chunk::ToolInputDelta(format!(r#"{{"path": "{path}"}}"#)), Chunk::BlockStop, Chunk::Stop(StopReason::ToolUse)];
    h.provider.push(call("a.txt")).push(call("b.txt")).push(text("done"));
    h.engine.submit(&h.session.id, prompt("read both")).await.await_ok();
    until_idle(&h).await;
    let ids: Vec<String> = h.engine.store.transcript(&h.session.id).unwrap().iter().flat_map(|m| &m.parts).filter_map(|row| match &row.part { Part::ToolCall { call_id, .. } => Some(call_id.clone()), _ => None }).collect();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], "functions.read:0", "a fresh id is kept as the provider sent it");
    assert!(ids[1] != ids[0] && ids[1].starts_with("call_"), "{ids:?}");
    let last = h.provider.requests.lock().unwrap().last().unwrap().clone();
    let results: Vec<&str> = last.messages.iter().flat_map(|m| &m.blocks).filter_map(|b| match b { crate::llm::Block::ToolResult { call_id, .. } => Some(call_id.as_str()), _ => None }).collect();
    assert_eq!(results, [ids[0].as_str(), ids[1].as_str()], "each result answers its own call");
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
    h.provider.push(read_a()).push(text("WRAPPED: read a, b still to do")).push(tool_call("read", r#"{"path": "b.txt"}"#)).push(text("finished"));
    h.engine.submit(&h.session.id, prompt("work")).await.await_ok();
    until_idle(&h).await;
    let mut transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let last = transcript.pop().unwrap();
    assert_eq!(last.info.status, MessageStatus::Paused);
    assert!(last.info.error.as_deref().unwrap().starts_with("Paused after 2 steps"), "{:?}", last.info.error);
    assert!(format!("{:?}", transcript.last().unwrap().parts).contains("WRAPPED"), "the last allowed step writes up what was done");
    {
        let requests = h.provider.requests.lock().unwrap();
        assert!(!requests[0].no_tool_calls && requests[1].no_tool_calls, "the last allowed step has tools off");
        assert!(format!("{:?}", requests[1].messages.last()).contains("last step this turn allows"));
    }
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
    h.provider.push(read_a()).push(read_a()).push(read_a()).push(text("WRAPPED: stuck on a.txt")).push(text("never"));
    h.engine.submit(&h.session.id, prompt("loop")).await.await_ok();
    until_idle(&h).await;
    let mut transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let last = transcript.pop().unwrap();
    assert_eq!(last.info.status, MessageStatus::Paused);
    assert!(last.info.error.as_deref().unwrap().contains("same calls and got the same results"), "{:?}", last.info.error);
    assert!(format!("{:?}", transcript.last().unwrap().parts).contains("WRAPPED"));
    assert!(h.provider.requests.lock().unwrap()[3].no_tool_calls);
    assert_eq!(h.provider.responses_left(), 1);
}

#[tokio::test]
async fn a_subagent_at_its_step_limit_hands_back_what_it_found() {
    let h = harness().await;
    h.engine.store.update_session(&h.session.id, None, Some(&model()), None).unwrap();
    std::fs::create_dir_all(h._dir.join("ws/.drift/agents")).unwrap();
    std::fs::write(h._dir.join("ws/.drift/agents/scout.md"), "---\nmode: subagent\nsteps: 2\n---\nScout.").unwrap();
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    h.provider
        .push(tool_call("task", r#"{"description": "scout", "prompt": "look around", "subagent_type": "scout"}"#))
        .push(read_a())
        .push(text("FINDINGS: a.txt holds a"))
        .push(text("thanks"));
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    until_idle(&h).await;
    let task = &h.engine.store.tasks_of(&h.session.id).unwrap()[0];
    assert_eq!(task.state, crate::session::tasks::TaskState::Failed, "a write-up is not a finished answer: {task:?}");
    let parent = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { metadata, .. } = &parent[1].parts[0].part else { panic!() };
    assert_eq!(metadata.as_ref().unwrap()["outcome"], "incomplete");
    let last = format!("{:?}", h.provider.requests.lock().unwrap().last().unwrap().messages);
    assert!(last.contains("FINDINGS") && last.contains("reached its step or repeat limit"), "the parent gets the write-up, marked partial");
}

#[tokio::test]
async fn a_wrap_up_that_calls_a_tool_anyway_runs_nothing() {
    let h = harness().await;
    limits(&h, r#"{ "steps": 1 }"#);
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    h.provider.push(read_a());
    h.engine.submit(&h.session.id, prompt("work")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, .. } = &transcript[1].parts[0].part else { panic!("{:?}", transcript[1].parts) };
    assert_eq!(*status, ToolStatus::Error, "{transcript:#?}");
    assert!(output.as_deref().unwrap().contains("tools were off"), "{output:?}");
    assert!(!h.engine.store.read_files(&h.session.id).unwrap_or_default().iter().any(|path| path.ends_with("a.txt")), "not even an early read ran");
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

#[tokio::test]
async fn every_step_of_a_conversation_carries_the_same_cache_key() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    h.provider.push(tool_call("read", r#"{"path": "a.txt"}"#)).push(text("read it"));
    h.engine.submit(&h.session.id, prompt("read a")).await.await_ok();
    until_idle(&h).await;
    let keys: Vec<Option<String>> = h.provider.requests.lock().unwrap().iter().map(|r| r.cache_key.clone()).collect();
    assert_eq!(keys, [Some(h.session.id.clone()), Some(h.session.id.clone())]);
}

#[test]
fn local_routes_wait_longer_and_drift_json_can_set_any_routes_limits() {
    use crate::llm::http::Timeouts;
    assert_eq!(Timeouts::for_route("ollama").headers, Duration::from_secs(600));
    assert_eq!(Timeouts::for_route("lmstudio").idle, Duration::from_secs(600));
    assert_eq!(Timeouts::for_route("anthropic"), Timeouts::default());
    let _ = rustls::crypto::ring::default_provider().install_default();
    let ollama = crate::llm::provider_for("ollama", None).unwrap();
    assert_eq!(ollama.timeouts().unwrap().headers, Duration::from_secs(600), "the route's own defaults apply when it is built");

    let dir = std::env::temp_dir().join(format!("drift-timeouts-{}", crate::random_hex(4)));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("drift.json"), r#"{ "timeouts": { "ollama": { "headersSeconds": 1800 }, "anthropic": { "idleSeconds": 60 } } }"#).unwrap();
    let config = crate::config::Config::load_with_home(&dir, None);
    assert_eq!(config.route_timeouts("ollama"), Timeouts { headers: Duration::from_secs(1800), idle: Duration::from_secs(600) });
    assert_eq!(config.route_timeouts("anthropic"), Timeouts { headers: Duration::from_secs(120), idle: Duration::from_secs(60) });
    std::fs::remove_dir_all(dir).ok();
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

#[tokio::test]
async fn an_edit_that_would_grow_a_file_past_what_undo_keeps_is_refused_before_it_writes() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "edit".into(), pattern: "*".into(), decision: Decision::Allow }] });
    let original = "x".repeat(50_000);
    std::fs::write(h._dir.join("ws/big.txt"), &original).unwrap();
    let edit = json!({ "path": "big.txt", "old_string": "x", "new_string": "y".repeat(256), "replace_all": true }).to_string();
    h.provider.push(tool_call("read", r#"{"path": "big.txt"}"#)).push(tool_call("edit", &edit)).push(text("tried"));
    h.engine.submit(&h.session.id, prompt("expand it")).await.await_ok();
    until_idle(&h).await;
    assert_eq!(std::fs::read_to_string(h._dir.join("ws/big.txt")).unwrap(), original, "nothing was written");
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, .. } = &transcript[2].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Error);
    assert!(output.as_deref().unwrap().contains("over the 10 MB undo can keep"), "{output:?}");
}

/// A turn busy with its first step while `switch` is sent; returns every request it made.
async fn switched_mid_turn(h: &Harness, switch: Prompt) -> Vec<Request> {
    h.provider.push_slow(Duration::from_millis(400), tool_call("read", r#"{"path": "missing.txt"}"#)).push(text("carried on"));
    h.engine.submit(&h.session.id, prompt("start")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.engine.submit(&h.session.id, switch).await.expect("taken by the running turn");
    until_idle(h).await;
    h.provider.requests.lock().unwrap().clone()
}

#[tokio::test]
async fn a_model_named_mid_turn_powers_its_next_request_and_the_conversation_carries_on() {
    let h = harness().await;
    let other = h.engine.catalog.read().unwrap().providers["anthropic"].models.keys().find(|id| *id != "claude-sonnet-4-5").unwrap().clone();
    let requests = switched_mid_turn(&h, Prompt { model: Some(ModelRef { provider: "anthropic".into(), model: other.clone() }), ..prompt("try the other one") }).await;
    assert_eq!(requests.iter().map(|r| r.model.as_str()).collect::<Vec<_>>(), ["claude-sonnet-4-5", other.as_str()], "one turn, the second request on the new model");
    assert!(format!("{:?}", requests[1].messages).contains("try the other one"));
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert_eq!(transcript.last().unwrap().info.model.as_ref().map(|m| m.model.as_str()), Some(other.as_str()), "the reply records the model that wrote it");
    let unknown = Prompt { model: Some(ModelRef { provider: "anthropic".into(), model: "no-such-model".into() }), ..prompt("x") };
    h.provider.push_slow(Duration::from_millis(300), text("busy"));
    h.engine.submit(&h.session.id, prompt("again")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h.engine.submit(&h.session.id, unknown).await.err(), Some(TurnError::UnknownModel), "a bad choice fails for the sender");
    until_idle(&h).await;
}

#[tokio::test]
async fn leaving_plan_tells_the_model_once_that_it_may_now_change_files() {
    let h = harness().await;
    let reminded = |request: &crate::llm::Request| format!("{:?}", request.messages).contains("switched from the plan agent to the build agent");
    h.provider.push(text("the plan")).push(text("building")).push(text("more"));
    h.engine.submit(&h.session.id, Prompt { agent: Some("plan".into()), ..prompt("plan it") }).await.await_ok();
    until_idle(&h).await;
    h.engine.submit(&h.session.id, Prompt { agent: Some("build".into()), ..prompt("go") }).await.await_ok();
    until_idle(&h).await;
    h.engine.submit(&h.session.id, prompt("and more")).await.await_ok();
    until_idle(&h).await;
    let requests = h.provider.requests.lock().unwrap().clone();
    assert_eq!(requests.iter().map(reminded).collect::<Vec<_>>(), [false, true, true], "from the turn that left plan on");
    let last = format!("{:?}", requests[2].messages);
    assert_eq!(last.matches("switched from the plan agent").count(), 1, "once, on the prompt that left plan, so the cached prefix stays the same");
    let stored = serde_json::to_string(&h.engine.store.transcript(&h.session.id).unwrap()).unwrap();
    assert!(!stored.contains("switched from the plan agent"), "never stored");
}

#[tokio::test]
async fn an_agent_or_level_named_mid_turn_applies_from_the_next_request() {
    let h = harness().await;
    let requests = switched_mid_turn(&h, Prompt { agent: Some("plan".into()), ..prompt("plan instead") }).await;
    let planning = |request: &crate::llm::Request| format!("{:?}", request.messages).contains("# Plan mode");
    assert!(!planning(&requests[0]) && planning(&requests[1]), "plan's reminder from the next request");
    assert_eq!(h.engine.store.transcript(&h.session.id).unwrap().last().unwrap().info.agent.as_deref(), Some("plan"));

    let h = harness().await;
    let requests = switched_mid_turn(&h, Prompt { variant: Some(Some("max".into())), ..prompt("think harder") }).await;
    assert_eq!(requests[0].reasoning, None);
    assert!(matches!(requests[1].reasoning, Some(Reasoning::Budget { .. })), "{:?}", requests[1].reasoning);
}

#[tokio::test]
async fn a_follow_up_naming_what_the_turn_already_runs_as_joins_it() {
    let h = harness().await;
    let with = |text: &str, agent: Option<&str>, variant: Option<Option<&str>>| Prompt { agent: agent.map(String::from), variant: variant.map(|v| v.map(String::from)), ..prompt(text) };
    let rounds = [
        (with("start", None, None), "its own agent, no level", Some("build"), Some(None)),
        (with("start", None, Some(Some("max"))), "its own level", Some("build"), Some(Some("max"))),
        (with("start", None, Some(None)), "a level this model lacks", None, Some(Some("ultra"))),
    ];
    for (round, (first, follow_up, agent, variant)) in rounds.into_iter().enumerate() {
        h.provider.push_slow(Duration::from_millis(300), tool_call("read", r#"{"path": "missing.txt"}"#)).push(text("done"));
        h.engine.submit(&h.session.id, first).await.await_ok();
        tokio::time::sleep(Duration::from_millis(100)).await;
        h.engine.submit(&h.session.id, with(follow_up, agent, variant)).await.await_ok();
        until_idle(&h).await;
        let requests = h.provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 2 * (round + 1), "{follow_up}: joined the running turn instead of ending it");
        assert!(format!("{:?}", requests.last().unwrap().messages).contains(follow_up), "{follow_up}: answered in the same turn");
    }
}

#[test]
fn a_variant_left_unnamed_and_one_cleared_hash_apart() {
    let unnamed = prompt("x");
    let cleared = Prompt { variant: Some(None), ..prompt("x") };
    assert_ne!(payload_hash(&unnamed), payload_hash(&cleared));
}

#[tokio::test]
async fn a_prompt_that_picks_plan_runs_as_plan_and_every_message_says_so() {
    let h = harness().await;
    h.provider.push(text("planned")).push(text("still planning"));
    h.engine.submit(&h.session.id, Prompt { agent: Some("plan".into()), ..prompt("plan it") }).await.await_ok();
    until_idle(&h).await;
    h.engine.submit(&h.session.id, prompt("and then")).await.await_ok();
    until_idle(&h).await;
    let reminded: Vec<usize> = h.provider.requests.lock().unwrap().iter().map(|r| format!("{:?}", r.messages).matches("# Plan mode").count()).collect();
    assert_eq!(reminded, [1, 1], "the prompt that started planning carries plan's reminder, kept as it was sent; the next plan turn adds none");
    assert_eq!(h.engine.store.session(&h.session.id).unwrap().unwrap().agent, "plan", "a prompt that names none keeps it");
    let agents: Vec<Option<String>> = h.engine.store.transcript(&h.session.id).unwrap().into_iter().map(|m| m.info.agent).collect();
    assert_eq!(agents, vec![Some("plan".to_string()); 4]);
    for refused in ["explore", "nobody"] {
        let result = h.engine.submit(&h.session.id, Prompt { agent: Some(refused.into()), ..prompt("x") }).await;
        assert_eq!(result.err(), Some(TurnError::UnknownAgent), "{refused} cannot run a conversation");
    }
}

#[tokio::test]
async fn a_prompts_variant_sets_the_requests_reasoning_and_an_unknown_one_asks_nothing() {
    use crate::llm::catalog::Reasoning;
    let h = harness().await;
    h.provider.push(text("thought hard")).push(text("plain"));
    let mut hard = prompt("think");
    hard.variant = Some(Some("max".into()));
    h.engine.submit(&h.session.id, hard).await.await_ok();
    until_idle(&h).await;
    let mut odd = prompt("again");
    odd.variant = Some(Some("ultra".into()));
    h.engine.submit(&h.session.id, odd).await.await_ok();
    until_idle(&h).await;
    let requests = h.provider.requests.lock().unwrap();
    assert!(matches!(requests[0].reasoning, Some(Reasoning::Budget { tokens }) if tokens > 16_000), "{:?}", requests[0].reasoning);
    assert_eq!(requests[1].reasoning, None, "a name the model does not offer");
}

#[tokio::test]
async fn the_session_keeps_its_variant_for_prompts_that_name_none_until_one_clears_it() {
    use crate::llm::catalog::Reasoning;
    let h = harness().await;
    h.provider.push(text("one")).push(text("two")).push(text("three")).push(text("four"));
    let with = |text: &str, variant: Option<Option<&str>>| Prompt { variant: variant.map(|v| v.map(String::from)), ..prompt(text) };
    for prompt in [with("set", Some(Some("max"))), with("inherit", None), with("clear", Some(None)), with("after", None)] {
        h.engine.submit(&h.session.id, prompt).await.await_ok();
        until_idle(&h).await;
    }
    let thought: Vec<bool> = h.provider.requests.lock().unwrap().iter().map(|r| matches!(r.reasoning, Some(Reasoning::Budget { .. }))).collect();
    assert_eq!(thought, [true, true, false, false], "a prompt that names none, as the engine's own do, runs at the session's");
    assert_eq!(h.engine.store.session(&h.session.id).unwrap().unwrap().variant, None);
}

#[test]
fn output_and_thinking_budgets_are_valid_together() {
    use crate::llm::catalog::Reasoning;
    let budget = |tokens| Some(Reasoning::Budget { tokens });
    assert_eq!(budgets(&model_with(32_000, true), budget(32_000)), (32_000, budget(32_000 - MIN_ANSWER_TOKENS)), "never past the model's own limit");
    assert_eq!(budgets(&model_with(64_000, true), budget(32_000)), (33_024, budget(32_000)), "a budget may raise the output past our cap");
    assert_eq!(budgets(&model_with(64_000, true), budget(100_000)), (64_000, budget(64_000 - MIN_ANSWER_TOKENS)));
    assert_eq!(budgets(&model_with(64_000, true), budget(10)), (32_000, budget(MIN_THINKING_TOKENS)), "raised to the provider's minimum");
    assert_eq!(budgets(&model_with(1_500, true), budget(8_000)), (1_500, None), "no room for thinking and an answer");
    assert_eq!(budgets(&model_with(64_000, false), budget(8_000)), (32_000, None), "a model that does not reason gets no budget");
    assert_eq!(budgets(&model_with(0, false), None), (MAX_OUTPUT_TOKENS, None), "an unknown limit and window use our cap");
    let mut local = model_with(0, false);
    local.limit.context = 4_096;
    assert_eq!(budgets(&local, None), (1_024, None), "an unknown limit asks for a quarter of a known window");
    local.limit.context = 2_048;
    assert_eq!(budgets(&local, None).0, MIN_ANSWER_TOKENS, "never less than room for an answer");
    let mut whole = model_with(32_768, false);
    whole.limit.context = 32_768;
    assert_eq!(whole.reply_room(), 16_384, "an output limit as large as the window gets half of it");
    assert_eq!(budgets(&whole, None).0, 16_384, "and asks for no more than that");
    let effort = Some(Reasoning::Effort { level: "high".into() });
    assert_eq!(budgets(&model_with(64_000, true), effort.clone()), (32_000, effort), "an effort passes through at the usual cap");
    for (limit, wanted) in [(4_096, 4_096), (8_192, 8_000), (128_000, 127_000), (2_048, 1_024)] {
        let (max, thinking) = budgets(&model_with(limit, true), budget(wanted));
        assert!(max as u64 <= limit, "{limit}/{wanted}");
        let fits = |t: u32| t + MIN_ANSWER_TOKENS <= max && t >= MIN_THINKING_TOKENS;
        assert!(thinking.as_ref().is_none_or(|r| matches!(r, Reasoning::Budget { tokens } if fits(*tokens))), "{limit}/{wanted}: {max} {thinking:?}");
    }
}

#[tokio::test]
async fn a_drift_json_that_cannot_be_read_stops_the_turn_instead_of_dropping_its_rules() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws").join("drift.json"), r#"{ "permissions": [ "#).unwrap();
    let refused = h.engine.submit(&h.session.id, prompt("run")).await.err();
    assert!(matches!(&refused, Some(TurnError::Config(problem)) if problem.contains("drift.json could not be read")), "{refused:?}");
    assert!(h.engine.store.transcript(&h.session.id).unwrap().is_empty(), "nothing was admitted");
}

#[tokio::test]
async fn workspace_config_shapes_the_turn() {
    let h = harness().await;
    let ws = h._dir.join("ws");
    std::fs::write(ws.join("drift.json"), r#"{ "permissions": [{ "kind": "bash", "pattern": "echo *", "decision": "deny" }] }"#).unwrap();
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

    // A plan session is offered build's tools and system prompt, so the cache holds; its prompt rides on the user's.
    h.engine.store.update_session(&h.session.id, None, None, Some("plan")).unwrap();
    h.provider.push(text("planned"));
    h.engine.submit(&h.session.id, prompt("plan it")).await.await_ok();
    until_idle(&h).await;
    let requests = h.provider.requests.lock().unwrap().clone();
    let (build, plan) = (&requests[0], requests.last().unwrap());
    assert_eq!((&plan.system, &plan.tools), (&build.system, &build.tools));
    assert!(format!("{:?}", plan.messages).contains("# Plan mode") && !plan.system.contains("# Plan mode"));
}

#[tokio::test]
async fn a_read_only_agent_never_calls_an_untrusted_servers_mcp_tool_even_one_it_calls_read_only() {
    let h = harness().await;
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/echo-server.cjs");
    let config = crate::mcp::ServerConfig::Stdio { command: "node".into(), args: vec![script.into()], env: Default::default(), cwd: None, timeout_seconds: None };
    h.engine.store.save_mcp_server("echo", &config).unwrap();
    h.engine.store.set_mcp_read_only_trusted("echo", false).unwrap();
    h.engine.connect_mcp_in("echo", Some(&crate::tool::canonical(&h._dir.join("ws")))).await.unwrap();
    h.engine.store.update_session(&h.session.id, None, None, Some("plan")).unwrap();
    h.provider.push(tool_call("echo_echo", r#"{"text": "hi"}"#)).push(text("noted"));
    h.engine.submit(&h.session.id, prompt("echo")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Error);
    assert!(output.as_deref().unwrap().contains("only reads"), "the server's read-only mark is its own claim: {output:?}");
}

#[tokio::test]
async fn images_from_tools_reach_a_model_that_reads_them_and_a_line_reaches_one_that_does_not() {
    use crate::llm::Block;
    let h = harness().await;
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/echo-server.cjs");
    let config = crate::mcp::ServerConfig::Stdio { command: "node".into(), args: vec![script.into()], env: Default::default(), cwd: None, timeout_seconds: None };
    h.engine.store.save_mcp_server("echo", &config).unwrap();
    h.engine.connect_mcp_in("echo", Some(&crate::tool::canonical(&h._dir.join("ws")))).await.unwrap();
    let mut wide = Vec::new();
    image::RgbImage::new(2600, 20).write_to(&mut std::io::Cursor::new(&mut wide), image::ImageFormat::Png).unwrap();
    std::fs::write(h._dir.join("ws/shot.png"), wide).unwrap();
    std::fs::write(h._dir.join("ws/broken.png"), b"\x89PNG\r\n\x1a\nrest").unwrap();
    h.provider
        .push(tool_call("echo_echo", r#"{"text": "picture"}"#))
        .push(tool_call("read", r#"{"path": "shot.png"}"#))
        .push(tool_call("read", r#"{"path": "broken.png"}"#))
        .push(text("seen"));
    h.engine.submit(&h.session.id, prompt("look")).await.await_ok();
    until_idle(&h).await;
    let requests = h.provider.requests.lock().unwrap().clone();
    let last = requests.last().unwrap();
    let images: Vec<&str> = last.messages.iter().flat_map(|m| &m.blocks).filter_map(|b| match b { Block::Image { mime, .. } => Some(mime.as_str()), _ => None }).collect();
    assert_eq!(images, ["image/png", "image/jpeg"], "the MCP screenshot and the scaled (opaque, so JPEG) read image reach the model; the broken one does not");
    let said = format!("{:?}", last.messages);
    assert!(said.contains("was scaled from 2600x20 to 2000x15 to fit the model's limits"));
    assert!(said.contains("[an image (image/png) is not shown: it could not be read"));
    let stored = serde_json::to_string(&h.engine.store.transcript(&h.session.id).unwrap()).unwrap();
    assert!(stored.contains("\"hash\"") && !stored.contains("iVBORw0KGgo"), "the part names its images; their bytes live in the blob table");
    let mut blind = model_with(0, false);
    blind.attachment = false;
    let text_only = crate::llm::prepare_files(last.messages.clone(), &blind, |_| None);
    assert!(text_only.iter().flat_map(|m| &m.blocks).all(|b| !matches!(b, Block::Image { .. })));
    assert!(format!("{text_only:?}").contains("this model cannot read images"));
}

#[test]
fn only_the_newest_images_are_sent_and_a_lost_one_becomes_a_line() {
    use crate::llm::{Block, ChatMessage, Role as LlmRole, MAX_IMAGES_SENT};
    let mut seeing = model_with(0, false);
    (seeing.attachment, seeing.pdf) = (true, true);
    let stored = |n: usize| Block::Stored { mime: "image/png".into(), hash: format!("h{n}") };
    let blocks: Vec<Block> = (0..MAX_IMAGES_SENT + 5).map(stored).collect();
    let prepared = crate::llm::prepare_files(vec![ChatMessage { role: LlmRole::User, blocks }], &seeing, |hash| (hash != "h14").then(|| b"png".to_vec()));
    let kinds: Vec<&str> = prepared[0].blocks.iter().map(|b| match b { Block::Image { .. } => "image", Block::Text(t) if t.contains("earlier") => "older", _ => "lost" }).collect();
    assert_eq!(kinds, [vec!["older"; 4], vec!["image"; MAX_IMAGES_SENT], vec!["lost"]].concat(), "the newest are loaded; one no longer kept says so and takes no slot");

    let big = |n: usize| Block::Image { mime: "image/png".into(), base64: format!("{n}{}", "A".repeat(8 * 1024 * 1024)) };
    let prepared = crate::llm::prepare_files(vec![ChatMessage { role: LlmRole::User, blocks: (0..4).map(big).collect() }], &seeing, |_| None);
    let sent = prepared[0].blocks.iter().filter(|b| matches!(b, Block::Image { .. })).count();
    assert_eq!(sent, 2, "a few large images fill the data budget before the count");
    assert!(matches!(&prepared[0].blocks[3], Block::Image { base64, .. } if base64.starts_with('3')), "the newest are the ones kept");

    let pdf = vec![ChatMessage { role: LlmRole::User, blocks: vec![Block::Stored { mime: "application/pdf".into(), hash: "p".into() }] }];
    let loaded = crate::llm::prepare_files(pdf.clone(), &seeing, |_| Some(b"%PDF-1.7".to_vec()));
    assert!(matches!(&loaded[0].blocks[0], Block::Pdf { .. }), "a stored PDF loads as a PDF");
    seeing.pdf = false;
    let refused = crate::llm::prepare_files(pdf, &seeing, |_| Some(b"%PDF-1.7".to_vec()));
    assert!(matches!(&refused[0].blocks[0], Block::Text(t) if t.contains("cannot read PDFs")));
}

#[tokio::test]
async fn a_command_that_only_reads_is_not_captured_and_one_that_writes_is() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Allow }] });
    h.provider
        .push(tool_call("bash", &json!({ "command": "echo looking" }).to_string()))
        .push(tool_call("bash", &json!({ "command": "touch made.txt" }).to_string()))
        .push(text("done"));
    h.engine.submit(&h.session.id, prompt("look then write")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let recorded: Vec<bool> = transcript
        .iter()
        .flat_map(|m| &m.parts)
        .filter_map(|row| match &row.part {
            Part::ToolCall { metadata, .. } => Some(metadata.as_ref().is_some_and(|m| m.get("changes").is_some())),
            _ => None,
        })
        .collect();
    assert_eq!(recorded, [false, true], "only the writing command carries a change record");
    assert!(h._dir.join("ws/made.txt").exists());
}

#[tokio::test]
async fn whole_tree_calls_in_a_step_chain_their_captures_and_a_file_tool_write_breaks_the_chain() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy {
        rules: vec![Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Allow }, Rule { kind: "edit".into(), pattern: "*".into(), decision: Decision::Allow }],
    });
    let call = |id: &str, name: &str, input: serde_json::Value| vec![Chunk::ToolUseStart { id: id.into(), name: name.into() }, Chunk::ToolInputDelta(input.to_string()), Chunk::BlockStop];
    h.provider
        .push(
            [
                call("t1", "bash", json!({ "command": "touch one.txt" })),
                call("t2", "bash", json!({ "command": "touch two.txt" })),
                call("t3", "write", json!({ "path": "three.txt", "content": "3\n" })),
                call("t4", "bash", json!({ "command": "touch four.txt" })),
                vec![Chunk::Stop(StopReason::ToolUse)],
            ]
            .concat(),
        )
        .push(text("done"));
    h.engine.submit(&h.session.id, prompt("make files")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let changed: Vec<Vec<String>> = transcript[1]
        .parts
        .iter()
        .filter_map(|row| match &row.part {
            Part::ToolCall { metadata: Some(meta), .. } => Some(meta["changes"].as_array().into_iter().flatten().filter_map(|c| c["path"].as_str().map(String::from)).collect()),
            _ => None,
        })
        .collect();
    assert_eq!(changed, [vec!["one.txt"], vec!["two.txt"], vec!["three.txt"], vec!["four.txt"]], "each call records only its own change; the write in between is not taken for the next command's");
}

#[tokio::test]
async fn a_subscription_sign_in_is_never_sent_to_a_route_the_user_re_pointed() {
    let h = harness().await;
    let live = Credential::OAuth { access: "a".into(), refresh: "r".into(), expires_at: crate::id::now_ms() + 3_600_000, account: None };
    h.engine.credentials.set("anthropic", &live).unwrap();
    h.engine.catalog.write().unwrap().providers.get_mut("anthropic").unwrap().api = Some("https://gateway.example".into());
    let refused = h.engine.submit(&h.session.id, prompt("hi")).await.err();
    assert!(matches!(&refused, Some(TurnError::Config(why)) if why.contains("subscription sign-in is only sent to anthropic")), "{refused:?}");
    h.engine.credentials.set("anthropic", &Credential::ApiKey { key: "k".into() }).unwrap();
    h.provider.push(text("via the gateway"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_idle(&h).await;
}

#[tokio::test]
async fn a_local_servers_installed_models_appear_and_run_without_a_key_while_it_answers() {
    let app = axum::Router::new().route("/v1/models", axum::routing::get(|| async { axum::Json(json!({ "data": [{ "id": "qwen3-coder-local" }] })) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let serving = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let h = harness().await;
    h.engine.catalog.write().unwrap().providers.get_mut("lmstudio").unwrap().api = Some(base);
    h.engine.ask_local().await;
    let local = ModelRef { provider: "lmstudio".into(), model: "qwen3-coder-local".into() };
    assert!(h.engine.catalog.read().unwrap().model("lmstudio", "qwen3-coder-local").is_some(), "the installed model is listed");
    h.provider.push(text("local reply"));
    let mut ask = prompt("hi");
    ask.model = Some(local.clone());
    h.engine.submit(&h.session.id, ask).await.await_ok();
    until_idle(&h).await;
    assert_eq!(h.engine.store.transcript(&h.session.id).unwrap()[1].info.status, MessageStatus::Done, "no key was needed");

    serving.abort();
    let _ = serving.await;
    h.engine.ask_local().await;
    assert!(h.engine.credentials.resolve("lmstudio", &[]).is_none(), "a server that stopped answering is not connected");
}

#[tokio::test]
async fn a_running_command_shows_its_output_before_it_ends() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Allow }] });
    let command = if cfg!(windows) { "echo early && ping -n 3 127.0.0.1 > /dev/null" } else { "echo early; sleep 2" };
    let mut events = h.engine.hub.attach(None).rx;
    h.provider.push(tool_call("bash", &json!({ "command": command }).to_string())).push(text("done"));
    h.engine.submit(&h.session.id, prompt("run")).await.await_ok();
    let shown = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.unwrap();
            if let Event::PartUpdated { part } = envelope.event {
                if let Part::ToolCall { status: ToolStatus::Running, metadata: Some(metadata), .. } = part.part {
                    if metadata["output"].as_str().is_some_and(|out| out.contains("early")) {
                        return metadata;
                    }
                }
            }
        }
    })
    .await
    .expect("the output so far is published while the command runs");
    assert!(shown.get("shellTimeoutMs").is_some(), "running metadata is kept beside it");
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Done);
    assert!(output.as_deref().unwrap().contains("early"));
}

#[tokio::test]
async fn a_configured_formatter_runs_after_a_write() {
    let h = harness().await;
    allow_edits_and_project_commands(&h);
    let (program, rest) = if cfg!(windows) { ("cmd", r#""/c", "echo tidy> $FILE""#) } else { ("sh", r#""-c", "echo tidy > $FILE""#) };
    let config = format!(r#"{{ "formatters": {{ "tidy": {{ "command": ["{program}", {rest}], "extensions": [".txt"] }} }} }}"#);
    std::fs::write(h._dir.join("ws/drift.json"), config).unwrap();
    h.provider.push(tool_call("write", r#"{"path": "note.txt", "content": "raw\n"}"#)).push(text("written"));
    h.engine.submit(&h.session.id, prompt("write")).await.await_ok();
    until_idle(&h).await;
    assert!(std::fs::read_to_string(h._dir.join("ws/note.txt")).unwrap().starts_with("tidy"));
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { metadata, output, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(metadata.as_ref().unwrap()["formatted"][0], "tidy: note.txt");
    assert!(output.as_deref().unwrap().contains("A formatter then changed the result (tidy: note.txt)"), "the model is told: {output:?}");

    h.provider.push(tool_call("write", r#"{"path": "note.txt", "content": "tidy\n"}"#)).push(text("again"));
    h.engine.submit(&h.session.id, prompt("write the same")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { metadata, output, .. } = &transcript[transcript.len() - 2].parts[0].part else { panic!() };
    assert!(metadata.as_ref().unwrap().get("formatted").is_none() && !output.as_deref().unwrap().contains("formatter"), "a formatter that changed nothing is not mentioned");
}

#[tokio::test]
async fn undo_puts_back_what_a_fixing_check_rewrote_and_forgets_what_checks_said() {
    let h = harness().await;
    allow_edits_and_project_commands(&h);
    let fix = if cfg!(windows) { ["cmd", "/c", "echo fixed> $FILE"] } else { ["sh", "-c", "echo fixed > $FILE"] };
    std::fs::write(h._dir.join("ws/drift.json"), json!({ "checks": { "fixer": { "command": fix, "extensions": [".md"] } } }).to_string()).unwrap();
    std::fs::write(h._dir.join("ws/notes.md"), "original\n").unwrap();
    // Read first: an existing file is only written after the session has read it.
    h.provider.push(tool_call("read", r#"{"path": "notes.md"}"#)).push(text("read it"));
    h.engine.submit(&h.session.id, prompt("read notes")).await.await_ok();
    until_idle(&h).await;
    h.provider.push(two_writes(("notes.md", "draft\n"), ("new.md", "new\n"))).push(text("written"));
    h.engine.submit(&h.session.id, prompt("write notes")).await.await_ok();
    until_idle(&h).await;
    assert!(std::fs::read_to_string(h._dir.join("ws/notes.md")).unwrap().starts_with("fixed"));
    assert!(std::fs::read_to_string(h._dir.join("ws/new.md")).unwrap().starts_with("fixed"));
    h.engine.turns.repeated(&h.session.id, "fixer", Some("seen"));

    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let prompt_id = transcript.iter().filter(|m| m.info.role == Role::User).nth(1).unwrap().info.id.clone();
    let undone = h.engine.revert(&h.session.id, &prompt_id).await.unwrap();
    assert!(undone.kept.is_empty(), "the check's rewrite is the session's own, so nothing is kept: {:?}", undone.kept);
    assert_eq!(std::fs::read_to_string(h._dir.join("ws/notes.md")).unwrap(), "original\n");
    assert!(!h._dir.join("ws/new.md").exists(), "a file the step created and a check rewrote is gone again");
    assert!(!h.engine.turns.repeated(&h.session.id, "fixer", Some("seen")), "undone, what checks said is forgotten");
}

#[tokio::test]
async fn a_change_no_check_covers_is_left_to_whoever_made_it_and_undo_keeps_it() {
    let h = harness().await;
    allow_edits_and_project_commands(&h);
    let other = h._dir.join("ws/b.txt");
    // A .md check that also stands in for someone editing b.txt while the checks run.
    let fix = if cfg!(windows) {
        ["cmd".to_string(), "/c".to_string(), format!("echo fixed> $FILE & echo user edit> {}", other.display())]
    } else {
        ["sh".to_string(), "-c".to_string(), format!("echo fixed > $FILE; echo user edit > '{}'", other.display())]
    };
    std::fs::write(h._dir.join("ws/drift.json"), json!({ "checks": { "fixer": { "command": fix, "extensions": [".md"] } } }).to_string()).unwrap();
    h.provider.push(two_writes(("a.md", "draft\n"), ("b.txt", "bee\n"))).push(text("written"));
    h.engine.submit(&h.session.id, prompt("write both")).await.await_ok();
    until_idle(&h).await;
    let calls = call_outputs(&h, 1);
    assert_eq!(calls[1].1["checkChanged"], json!(["a.md"]), "only the file a check covers is the check's: {:?}", calls[1].1);

    let prompt_id = h.engine.store.transcript(&h.session.id).unwrap()[0].info.id.clone();
    h.engine.revert(&h.session.id, &prompt_id).await.unwrap();
    assert!(!h._dir.join("ws/a.md").exists(), "the check's rewrite is undone with the write");
    assert!(std::fs::read_to_string(&other).unwrap().starts_with("user edit"), "someone else's edit is never undone as the session's");
}

#[tokio::test]
async fn a_stop_while_a_fixer_runs_still_records_what_it_rewrote() {
    let h = harness().await;
    allow_edits_and_project_commands(&h);
    let fix = if cfg!(windows) { ["cmd", "/c", "echo fixed> $FILE & ping -n 30 127.0.0.1 > nul"] } else { ["sh", "-c", "echo fixed > $FILE; sleep 30"] };
    std::fs::write(h._dir.join("ws/drift.json"), json!({ "checks": { "fixer": { "command": fix, "extensions": [".md"] } } }).to_string()).unwrap();
    h.provider.push(tool_call("write", r#"{"path": "a.md", "content": "draft\n"}"#)).push(text("written"));
    h.engine.submit(&h.session.id, prompt("write a")).await.await_ok();
    for _ in 0..500 {
        if std::fs::read_to_string(h._dir.join("ws/a.md")).is_ok_and(|text| text.starts_with("fixed")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    h.engine.abort(&h.session.id);
    until_idle(&h).await;
    let (_, meta) = &call_outputs(&h, 1)[0];
    assert_eq!(meta["checkChanged"], json!(["a.md"]), "{meta:?}");
    let prompt_id = h.engine.store.transcript(&h.session.id).unwrap()[0].info.id.clone();
    h.engine.revert(&h.session.id, &prompt_id).await.unwrap();
    assert!(!h._dir.join("ws/a.md").exists(), "undo puts back the fixer's rewrite too");
}

#[tokio::test]
async fn a_whole_workspace_fixer_is_captured_whole_and_what_it_changed_elsewhere_is_said_and_left_to_stand() {
    let h = harness().await;
    allow_edits_and_project_commands(&h);
    // No $FILE: it runs once over the workspace, like `eslint --fix .`, and rewrites more than the step wrote.
    let fix = if cfg!(windows) { ["cmd", "/c", "echo fixed> a.md & echo fixed> other.md"] } else { ["sh", "-c", "echo fixed > a.md; echo fixed > other.md"] };
    std::fs::write(h._dir.join("ws/drift.json"), json!({ "checks": { "fix-all": { "command": fix, "extensions": [".md"] } } }).to_string()).unwrap();
    std::fs::write(h._dir.join("ws/other.md"), "untouched\n").unwrap();
    h.provider.push(tool_call("write", r#"{"path": "a.md", "content": "draft\n"}"#)).push(text("written"));
    h.engine.submit(&h.session.id, prompt("write a")).await.await_ok();
    until_idle(&h).await;
    let (output, meta) = &call_outputs(&h, 1)[0];
    assert_eq!((&meta["checkChanged"], &meta["checkObserved"]), (&json!(["a.md"]), &json!(["other.md"])), "{meta:?}");
    assert!(output.contains("files this step did not write changed too (other.md)"), "{output}");

    let prompt_id = h.engine.store.transcript(&h.session.id).unwrap()[0].info.id.clone();
    h.engine.revert(&h.session.id, &prompt_id).await.unwrap();
    assert!(!h._dir.join("ws/a.md").exists(), "the step's own file goes back, check's rewrite and all");
    assert!(std::fs::read_to_string(h._dir.join("ws/other.md")).unwrap().starts_with("fixed"), "a file the step never wrote is not the session's to undo");
}

#[tokio::test]
async fn a_check_rewrite_without_a_capture_is_still_announced_and_said_to_be_unrecorded() {
    let h = harness().await;
    let allow = |kind: &str| Rule { kind: kind.into(), pattern: "*".into(), decision: Decision::Allow };
    h.engine.permissions.set_policy(Policy { rules: vec![allow("edit"), allow("bash"), allow("project-commands")] });
    let fix = if cfg!(windows) { ["cmd", "/c", "echo fixed> $FILE"] } else { ["sh", "-c", "echo fixed > $FILE"] };
    std::fs::write(h._dir.join("ws/drift.json"), json!({ "checks": { "fixer": { "command": fix, "extensions": [".md"] } } }).to_string()).unwrap();
    // A second call in the step leaves the shadow store unusable, so the checks cannot be captured.
    let store = h._dir.join("data/snapshots").to_string_lossy().replace('\\', "/");
    let breaks = match crate::tool::bash::Bash::detect().dialect() {
        crate::tool::command::Dialect::Bash => format!("rm -rf '{store}' && printf x > '{store}'"),
        crate::tool::command::Dialect::PowerShell => format!("Remove-Item -Recurse -Force '{store}'; Set-Content -Path '{store}' -Value x"),
    };
    let calls = [
        vec![Chunk::ToolUseStart { id: "toolu_write".into(), name: "write".into() }, Chunk::ToolInputDelta(json!({ "path": "new.md", "content": "draft\n" }).to_string()), Chunk::BlockStop],
        vec![Chunk::ToolUseStart { id: "toolu_bash".into(), name: "bash".into() }, Chunk::ToolInputDelta(json!({ "command": breaks }).to_string()), Chunk::BlockStop],
        vec![Chunk::Stop(StopReason::ToolUse)],
    ];
    let mut rx = h.engine.hub.attach(None).rx;
    h.provider.push(calls.concat()).push(text("done"));
    h.engine.submit(&h.session.id, prompt("write and break")).await.await_ok();
    // A line that writes a file is only ever allowed as itself, so the shell call asks.
    let ask = next_ask(&mut rx).await;
    h.engine.permissions.reply(&h.engine.hub, &ask.id, ReplyBody { reply: Reply::Once, pattern: None, message: None }).unwrap();
    until_idle(&h).await;
    assert!(std::fs::read_to_string(h._dir.join("ws/new.md")).unwrap().starts_with("fixed"));
    let write = &call_outputs(&h, 1)[0];
    assert!(write.0.contains("A check then changed new.md") && write.0.contains("undo cannot put back what the checks rewrote"), "{}", write.0);
    assert_eq!(write.1["unrecorded"], json!(["new.md"]));
}

#[tokio::test]
async fn compaction_forgets_what_checks_said() {
    let h = harness().await;
    h.provider.push(text("hello")).push(text("the summary"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_idle(&h).await;
    h.engine.turns.repeated(&h.session.id, "types", Some("3 errors"));
    h.engine.compact(&h.session.id, super::compaction::Trigger::Manual, &Default::default()).await.unwrap();
    assert!(!h.engine.turns.repeated(&h.session.id, "types", Some("3 errors")), "the summary may not hold the full report, so it is sent again");
}

/// For tests about what checks and formatters do, not whether they may run: a rule allows the project's commands.
fn allow_edits_and_project_commands(h: &Harness) {
    let allow = |kind: &str| Rule { kind: kind.into(), pattern: "*".into(), decision: Decision::Allow };
    h.engine.permissions.set_policy(Policy { rules: vec![allow("edit"), allow("project-commands")] });
}

#[tokio::test]
async fn a_projects_own_commands_run_only_once_the_user_says_so_and_always_holds_for_the_workspace() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "edit".into(), pattern: "*".into(), decision: Decision::Allow }] });
    let marker = h._dir.join("ran.log");
    let (shell, flag, run) = if cfg!(windows) { ("cmd", "/c", format!("echo ran>> {}", marker.display())) } else { ("sh", "-c", format!("echo ran >> '{}'", marker.display())) };
    std::fs::write(h._dir.join("ws/drift.json"), json!({ "checks": { "mark": { "command": [shell, flag, run], "extensions": [".txt"] } } }).to_string()).unwrap();
    let mut rx = h.engine.hub.attach(None).rx;

    h.provider.push(tool_call("write", r#"{"path": "readme.md", "content": "r\n"}"#)).push(text("zero"));
    h.engine.submit(&h.session.id, prompt("write the readme")).await.await_ok();
    until_idle(&h).await;
    assert!(h.engine.permissions.pending().is_empty(), "nothing of the project's covers a .md file, so nothing is asked");

    h.provider.push(tool_call("write", r#"{"path": "a.txt", "content": "a\n"}"#)).push(text("one"));
    h.engine.submit(&h.session.id, prompt("write a")).await.await_ok();
    let ask = next_ask(&mut rx).await;
    assert_eq!((ask.ask.kind.as_str(), ask.ask.pattern.as_str()), ("project-commands", format!("check mark: {shell} {flag} {run}").as_str()));
    h.engine.permissions.reply(&h.engine.hub, &ask.id, ReplyBody { reply: Reply::Deny, pattern: None, message: None }).unwrap();
    until_idle(&h).await;
    assert!(!marker.exists(), "refused, the project's command does not run");
    h.provider.push(tool_call("write", r#"{"path": "b.txt", "content": "b\n"}"#)).push(text("two"));
    h.engine.submit(&h.session.id, prompt("write b")).await.await_ok();
    until_idle(&h).await;
    assert!(h.engine.permissions.pending().is_empty() && !marker.exists(), "a refusal holds for the session without asking again");

    let session = |title| h.engine.store.create_session(NewSession { workspace_id: &h.session.workspace_id, parent_id: None, visibility: Visibility::Sibling, title, agent: "build", model: None }).unwrap();
    let runs = || std::fs::read_to_string(&marker).unwrap_or_default().lines().count();
    let other = session("Other");
    h.provider.push(tool_call("write", r#"{"path": "c.txt", "content": "c\n"}"#)).push(text("three"));
    h.engine.submit(&other.id, prompt("write c")).await.await_ok();
    let ask = next_ask(&mut rx).await;
    h.engine.permissions.reply(&h.engine.hub, &ask.id, ReplyBody { reply: Reply::Always, pattern: None, message: None }).unwrap();
    until_session_idle(&h, &other.id).await;
    assert_eq!(runs(), 1, "allowed, it runs");
    let third = session("Third");
    h.provider.push(tool_call("write", r#"{"path": "d.txt", "content": "d\n"}"#)).push(text("four"));
    h.engine.submit(&third.id, prompt("write d")).await.await_ok();
    until_session_idle(&h, &third.id).await;
    assert_eq!(runs(), 2);
    assert!(h.engine.permissions.pending().is_empty(), "always holds for the workspace, in a new session too");

    std::fs::write(h._dir.join("ws/drift.json"), json!({ "checks": { "mark": { "command": [shell, flag, format!("{run} & echo changed")], "extensions": [".txt"] } } }).to_string()).unwrap();
    let fourth = session("Fourth");
    h.provider.push(tool_call("write", r#"{"path": "e.txt", "content": "e\n"}"#)).push(text("five"));
    h.engine.submit(&fourth.id, prompt("write e")).await.await_ok();
    assert_eq!(next_ask(&mut rx).await.ask.kind, "project-commands", "changed commands are asked about again");
    assert!(h.engine.abort(&fourth.id));
}

#[tokio::test]
async fn a_formatter_installed_in_the_project_runs_only_once_allowed() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "edit".into(), pattern: "*".into(), decision: Decision::Allow }] });
    let ws = h._dir.join("ws");
    let marker = h._dir.join("ran.log");
    std::fs::create_dir_all(ws.join("node_modules/.bin")).unwrap();
    std::fs::write(ws.join("package.json"), r#"{ "devDependencies": { "prettier": "^3" } }"#).unwrap();
    let (shim, body) = if cfg!(windows) { ("prettier.cmd", format!("@echo ran>> \"{}\"\r\n", marker.display())) } else { ("prettier", format!("#!/bin/sh\necho ran >> '{}'\n", marker.display())) };
    std::fs::write(ws.join("node_modules/.bin").join(shim), body).unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(ws.join("node_modules/.bin").join(shim), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mut rx = h.engine.hub.attach(None).rx;
    h.provider.push(tool_call("write", r#"{"path": "a.ts", "content": "let a = 1\n"}"#)).push(text("written"));
    h.engine.submit(&h.session.id, prompt("write a")).await.await_ok();
    let ask = next_ask(&mut rx).await;
    assert_eq!(ask.ask.kind, "project-commands");
    assert!(ask.ask.pattern.starts_with("formatter prettier: ") && ask.ask.pattern.contains("node_modules"), "{}", ask.ask.pattern);
    h.engine.permissions.reply(&h.engine.hub, &ask.id, ReplyBody { reply: Reply::Deny, pattern: None, message: None }).unwrap();
    until_idle(&h).await;
    assert!(!marker.exists(), "a binary the repository brings never runs without the user's say-so");
}

#[tokio::test]
async fn refusing_the_projects_formatter_leaves_its_checks_to_their_own_answer() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "edit".into(), pattern: "*".into(), decision: Decision::Allow }] });
    let ws = h._dir.join("ws");
    let (formatted, checked) = (h._dir.join("formatted.log"), h._dir.join("checked.log"));
    std::fs::create_dir_all(ws.join("node_modules/.bin")).unwrap();
    std::fs::write(ws.join("package.json"), r#"{ "devDependencies": { "prettier": "^3" } }"#).unwrap();
    let (shim, body) = if cfg!(windows) { ("prettier.cmd", format!("@echo ran>> \"{}\"\r\n", formatted.display())) } else { ("prettier", format!("#!/bin/sh\necho ran >> '{}'\n", formatted.display())) };
    std::fs::write(ws.join("node_modules/.bin").join(shim), body).unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(ws.join("node_modules/.bin").join(shim), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let (shell, flag, run) = if cfg!(windows) { ("cmd", "/c", format!("echo ran>> {}", checked.display())) } else { ("sh", "-c", format!("echo ran >> '{}'", checked.display())) };
    std::fs::write(ws.join("drift.json"), json!({ "checks": { "mark": { "command": [shell, flag, run], "extensions": [".ts"] } } }).to_string()).unwrap();
    let mut rx = h.engine.hub.attach(None).rx;
    h.provider.push(tool_call("write", r#"{"path": "a.ts", "content": "let a = 1\n"}"#)).push(text("one"));
    h.engine.submit(&h.session.id, prompt("write a")).await.await_ok();
    let formatter = next_ask(&mut rx).await;
    assert!(formatter.ask.pattern.starts_with("formatter prettier: "), "{}", formatter.ask.pattern);
    h.engine.permissions.reply(&h.engine.hub, &formatter.id, ReplyBody { reply: Reply::Deny, pattern: None, message: None }).unwrap();
    let check = next_ask(&mut rx).await;
    assert!(check.ask.pattern.starts_with("check mark: "), "asked about apart from the formatter: {}", check.ask.pattern);
    h.engine.permissions.reply(&h.engine.hub, &check.id, ReplyBody { reply: Reply::Always, pattern: None, message: None }).unwrap();
    until_idle(&h).await;
    h.provider.push(tool_call("write", r#"{"path": "b.ts", "content": "let b = 1\n"}"#)).push(text("two"));
    h.engine.submit(&h.session.id, prompt("write b")).await.await_ok();
    until_idle(&h).await;
    assert!(h.engine.permissions.pending().is_empty());
    assert!(!formatted.exists(), "the refused formatter never runs");
    assert_eq!(std::fs::read_to_string(&checked).unwrap().lines().count(), 2, "the allowed check runs after both writes");
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

fn two_writes(first: (&str, &str), second: (&str, &str)) -> Vec<Chunk> {
    let call = |id: &str, (path, content): (&str, &str)| {
        vec![Chunk::ToolUseStart { id: id.into(), name: "write".into() }, Chunk::ToolInputDelta(json!({ "path": path, "content": content }).to_string()), Chunk::BlockStop]
    };
    [call("toolu_first", first), call("toolu_second", second), vec![Chunk::Stop(StopReason::ToolUse)]].concat()
}

fn call_outputs(h: &Harness, message: usize) -> Vec<(String, serde_json::Value)> {
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    transcript[message]
        .parts
        .iter()
        .filter_map(|row| match &row.part {
            Part::ToolCall { output, metadata, .. } => Some((output.clone().unwrap_or_default(), metadata.clone().unwrap_or_default())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn checks_run_once_per_step_say_unchanged_problems_briefly_and_announce_files_they_change() {
    let h = harness().await;
    allow_edits_and_project_commands(&h);
    let log = h._dir.join("whole.log");
    let (shell, flag, whole, fix) = if cfg!(windows) {
        ("cmd", "/c", format!("echo ran>> {} & echo 3 type errors in the workspace & exit 1", log.display()), "echo fixed> $FILE")
    } else {
        ("sh", "-c", format!("echo ran >> '{}'; echo 3 type errors in the workspace; exit 1", log.display()), "echo fixed > $FILE")
    };
    let config = json!({ "checks": {
        "types": { "command": [shell, flag, whole], "extensions": [".ts"] },
        "fixer": { "command": [shell, flag, fix], "extensions": [".md"] },
    } });
    std::fs::write(h._dir.join("ws/drift.json"), config.to_string()).unwrap();
    h.provider.push(two_writes(("a.ts", "let a = 1\n"), ("b.ts", "let b = 2\n"))).push(text("written"));
    h.engine.submit(&h.session.id, prompt("write two")).await.await_ok();
    until_idle(&h).await;
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1, "one run for the step, not one per write");
    let calls = call_outputs(&h, 1);
    assert!(!calls[0].0.contains("Checks reported"), "the step's report goes on its last write: {:?}", calls[0].0);
    assert!(calls[1].0.contains("[types]\n3 type errors in the workspace"), "{:?}", calls[1].0);
    assert_eq!(calls[1].1["checks"][0]["status"], "problems");

    h.provider.push(tool_call("write", r#"{"path": "c.ts", "content": "let c = 3\n"}"#)).push(text("again"));
    h.engine.submit(&h.session.id, prompt("write one more")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let again = call_outputs(&h, transcript.len() - 2);
    assert!(again[0].0.contains("[types] the same problems as reported before") && !again[0].0.contains("3 type errors"), "{:?}", again[0].0);

    h.provider.push(tool_call("write", r#"{"path": "notes.md", "content": "draft\n"}"#)).push(text("noted"));
    h.engine.submit(&h.session.id, prompt("write notes")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let fixed = call_outputs(&h, transcript.len() - 2);
    assert!(std::fs::read_to_string(h._dir.join("ws/notes.md")).unwrap().starts_with("fixed"));
    assert!(fixed[0].0.contains("A check then changed notes.md"), "{:?}", fixed[0].0);
    assert_eq!(fixed[0].1["checkChanged"][0], "notes.md");
}

#[tokio::test]
async fn configured_checks_report_problems_with_the_write_and_stop_cuts_them_off() {
    let h = harness().await;
    allow_edits_and_project_commands(&h);
    let (shell, flag, fail, hang) =
        if cfg!(windows) { ("cmd", "/c", "echo unused import in $FILE&& exit 1", "ping -n 30 127.0.0.1") } else { ("sh", "-c", "echo unused import in $FILE; exit 1", "sleep 30") };
    let config = json!({ "checks": {
        "lint": { "command": [shell, flag, fail], "extensions": [".ts"] },
        "slow": { "command": [shell, flag, hang], "extensions": [".rs"] },
    } });
    std::fs::write(h._dir.join("ws/drift.json"), config.to_string()).unwrap();
    h.provider.push(tool_call("write", r#"{"path": "a.ts", "content": "import x\n"}"#)).push(text("written"));
    h.engine.submit(&h.session.id, prompt("write")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, metadata, output, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Done, "the write itself succeeded");
    let output = output.as_deref().unwrap();
    assert!(output.contains("Checks reported problems after this step's changes") && output.contains("[lint: a.ts]") && output.contains("unused import in"), "{output}");
    assert_eq!(metadata.as_ref().unwrap()["checks"][0]["status"], "problems");

    h.provider.push(tool_call("write", r#"{"path": "b.rs", "content": "fn main() {}\n"}"#));
    let started = std::time::Instant::now();
    h.engine.submit(&h.session.id, prompt("write rust")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(h.engine.abort(&h.session.id));
    until_idle(&h).await;
    assert!(started.elapsed() < Duration::from_secs(15), "Stop does not wait out a slow check");
    assert!(h._dir.join("ws/b.rs").exists());
}

#[tokio::test]
async fn a_read_only_agent_is_refused_every_call_that_would_change_something() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy {
        rules: vec![Rule { kind: "edit".into(), pattern: "*".into(), decision: Decision::Allow }, Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Allow }],
    });
    h.engine.store.update_session(&h.session.id, None, None, Some("plan")).unwrap();
    let call = |id: &str, name: &str, input: &str| vec![Chunk::ToolUseStart { id: id.into(), name: name.into() }, Chunk::ToolInputDelta(input.into()), Chunk::BlockStop];
    h.provider
        .push(
            [
                call("t1", "write", r#"{"path": "plan-mutated.txt", "content": "x\n"}"#),
                call("t2", "bash", r#"{"command": "echo x > made.txt"}"#),
                call("t3", "task", r#"{"description": "Change it", "prompt": "edit a file", "subagent_type": "general"}"#),
                call("t4", "bash", r#"{"command": "git status"}"#),
                vec![Chunk::Stop(StopReason::ToolUse)],
            ]
            .concat(),
        )
        .push(text("noted"));
    h.engine.submit(&h.session.id, prompt("write it anyway")).await.await_ok();
    until_idle(&h).await;
    assert!(!h._dir.join("ws/plan-mutated.txt").exists() && !h._dir.join("ws/made.txt").exists(), "plan mode must not write");
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    for refused in &transcript[1].parts[..3] {
        let Part::ToolCall { status, output, metadata, .. } = &refused.part else { panic!() };
        assert_eq!(*status, ToolStatus::Error);
        assert!(output.as_deref().unwrap().contains("only reads"), "{output:?}");
        assert!(metadata.is_none(), "nothing was recorded for a call that never ran");
    }
    let Part::ToolCall { status, .. } = &transcript[1].parts[3].part else { panic!() };
    assert_eq!(*status, ToolStatus::Done, "a shell line that only reads runs, so plan can look at git history");
    let sessions = h.engine.store.sessions(crate::store::SessionFilter { workspace_id: None, archived: false, before: None, limit: 10 }).unwrap();
    assert_eq!(sessions.len(), 1, "no writing subagent was started");
}

#[tokio::test]
async fn a_call_to_a_tool_the_run_did_not_offer_is_refused_before_anything_happens() {
    let h = harness().await;
    std::fs::create_dir_all(h._dir.join("ws/.drift/agents")).unwrap();
    std::fs::write(h._dir.join("ws/.drift/agents/reader.md"), "---\ndescription: Reads\ntools: read\n---\nRead only.").unwrap();
    h.engine.store.update_session(&h.session.id, None, None, Some("reader")).unwrap();
    h.provider.push(tool_call("write", r#"{"path": "mutated.txt", "content": "x\n"}"#)).push(text("noted"));
    h.engine.submit(&h.session.id, prompt("write it anyway")).await.await_ok();
    until_idle(&h).await;
    assert!(!h._dir.join("ws/mutated.txt").exists());
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
    assert!(output.as_deref().unwrap().starts_with("a.txt contains alpha\n\n(task_id:"));
    let child_id = metadata.as_ref().unwrap()["sessionId"].as_str().unwrap().to_string();
    let child = h.engine.store.session(&child_id).unwrap().unwrap();
    assert_eq!(child.parent_id.as_deref(), Some(h.session.id.as_str()));
    assert_eq!(child.visibility, Visibility::Hidden);
    assert_eq!(child.title, "Check a.txt (@general subagent)", "general takes a task when no type is given");
    assert_eq!(h.engine.store.transcript(&child_id).unwrap().len(), 3);
    let listed = h.engine.store.sessions(crate::store::SessionFilter { workspace_id: None, archived: false, before: None, limit: 10 }).unwrap();
    assert!(listed.iter().any(|s| s.id == child_id && s.parent_id.as_deref() == Some(h.session.id.as_str())), "subagents are listed so the UI can nest them");
}

#[tokio::test]
async fn a_finished_subagent_can_be_continued_with_what_it_already_saw() {
    let h = harness().await;
    h.provider
        .push(tool_call("task", r#"{"description": "Find it", "prompt": "Where is FIRST_CONTEXT handled?"}"#))
        .push(text("in parser.rs"))
        .push(text("found"));
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    until_idle(&h).await;
    let (_, _, first) = task_call(&h.engine.store.transcript(&h.session.id).unwrap());
    let (task_id, child_id) = (first["taskId"].as_str().unwrap().to_string(), first["sessionId"].as_str().unwrap().to_string());
    let follow_up = json!({ "description": "Follow up", "prompt": "And who calls it?", "task_id": task_id, "subagent_type": "explore" }).to_string();
    // Each call gets its own id from a real provider; the same id would be a replay of the first.
    let mut again = tool_call("task", &follow_up);
    again[0] = Chunk::ToolUseStart { id: "toolu_task_2".into(), name: "task".into() };
    h.provider.push(again).push(text("main.rs calls it")).push(text("done"));
    h.engine.submit(&h.session.id, prompt("ask again")).await.await_ok();
    until_idle(&h).await;
    let child_request = h.provider.requests.lock().unwrap().iter().rev().nth(1).unwrap().clone();
    let seen = format!("{:?}", child_request.messages);
    assert!(seen.contains("FIRST_CONTEXT") && seen.contains("in parser.rs") && seen.contains("who calls it"), "the subagent continues its own conversation: {seen}");
    let tasks = h.engine.store.tasks_of(&h.session.id).unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!((tasks[1].session_id.as_str(), tasks[1].agent.as_str()), (child_id.as_str(), "general"), "same transcript, same agent");
    assert_eq!(h.engine.store.task_for_session(&child_id).unwrap().unwrap().id, tasks[1].id, "the latest task speaks for the session");

    let mut bad = tool_call("task", &json!({ "description": "x", "prompt": "y", "task_id": "task_nope" }).to_string());
    bad[0] = Chunk::ToolUseStart { id: "toolu_task_3".into(), name: "task".into() };
    h.provider.push(bad).push(text("ok"));
    h.engine.submit(&h.session.id, prompt("bad id")).await.await_ok();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let refused = transcript.iter().rev().flat_map(|m| &m.parts).find_map(|row| match &row.part { Part::ToolCall { output, .. } => output.clone(), _ => None }).unwrap();
    assert!(refused.contains("no task task_nope"), "{refused}");
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
async fn a_turn_keeps_the_tools_it_started_with_and_a_change_reaches_the_next_one() {
    use crate::mcp::ServerConfig;
    let h = harness().await;
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/echo-server.cjs");
    let config = ServerConfig::Stdio { command: "node".into(), args: vec![script.into()], env: Default::default(), cwd: None, timeout_seconds: None };
    h.engine.store.save_mcp_server("echo", &config).unwrap();
    h.engine.connect_mcp_in("echo", Some(&crate::tool::canonical(&h._dir.join("ws")))).await.unwrap();
    h.provider.push_slow(Duration::from_millis(500), tool_call("echo_echo", r#"{"text": "still here"}"#)).push(text("done"));
    h.engine.submit(&h.session.id, prompt("echo")).await.await_ok();
    // The server goes away while the turn is still streaming its first reply.
    tokio::time::sleep(Duration::from_millis(150)).await;
    h.engine.mcp.disconnect("echo", &h.engine.store, &h.engine.hub).await;
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!((*status, output.as_deref()), (ToolStatus::Done, Some("still here")), "served by the client the turn began with");

    h.provider.push(text("ok"));
    h.engine.submit(&h.session.id, prompt("again")).await.await_ok();
    until_idle(&h).await;
    let requests = h.provider.requests.lock().unwrap();
    assert!(requests[0].tools.iter().any(|t| t.name == "echo_echo"));
    assert!(requests[0].system.contains("# Instructions from the echo MCP server\n\nEcho repeats what it is given."), "the server's instructions come with its tools");
    assert!(!requests.last().unwrap().tools.iter().any(|t| t.name == "echo_echo"), "the next turn sees the change");
    assert!(!requests.last().unwrap().system.contains("echo MCP server"), "and its instructions go with them");
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

fn status(state: &str) -> Vec<Chunk> {
    text(&format!("round\n<orchestrator_status>{{\"state\":\"{state}\"}}</orchestrator_status>"))
}

fn nudges(h: &Harness) -> Vec<String> {
    h.engine.store.transcript(&h.session.id).unwrap().iter().flat_map(|m| m.parts.iter()).filter_map(|row| match &row.part {
        Part::Nudge { text } => Some(text.clone()),
        _ => None,
    }).collect()
}

#[tokio::test]
async fn the_engine_drives_an_orchestrator_until_it_is_done_and_reminds_it_of_the_protocol() {
    let h = harness().await;
    h.provider.push(status("working")).push(text("forgot the block")).push(status("done"));
    h.engine.submit(&h.session.id, Prompt { agent: Some("orchestrator".into()), ..prompt("ship it") }).await.await_ok();
    until_idle(&h).await;
    let sent = nudges(&h);
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(sent[0].starts_with("Proceed toward the goal") && sent[1].contains("did not end with a valid <orchestrator_status> block"));
    assert_eq!(h.provider.requests.lock().unwrap().len(), 3, "done ends the turn");
    let last = h.provider.requests.lock().unwrap()[2].messages.last().cloned().unwrap();
    assert!(matches!(&last.blocks[..], [crate::llm::Block::Text(text)] if text.contains("<orchestrator_status>")), "the model reads the nudge as a prompt");
}

#[tokio::test]
async fn an_orchestrator_stops_after_the_round_limit_and_a_new_prompt_restarts_the_count() {
    let h = harness().await;
    for _ in 0..=crate::session::drive::MAX_ROUNDS {
        h.provider.push(status("working"));
    }
    h.engine.submit(&h.session.id, Prompt { agent: Some("orchestrator".into()), ..prompt("big goal") }).await.await_ok();
    until_idle(&h).await;
    assert_eq!(nudges(&h).len(), crate::session::drive::MAX_ROUNDS);
    assert_eq!(h.engine.store.nudges_since_prompt(&h.session.id).unwrap(), crate::session::drive::MAX_ROUNDS);
    h.provider.push(status("working")).push(status("blocked"));
    h.engine.submit(&h.session.id, prompt("keep going")).await.await_ok();
    until_idle(&h).await;
    assert_eq!(nudges(&h).len(), crate::session::drive::MAX_ROUNDS + 1, "the user's own prompt starts a fresh budget; blocked ends it");
}

#[tokio::test]
async fn other_agents_are_never_driven() {
    let h = harness().await;
    h.provider.push(status("working"));
    h.engine.submit(&h.session.id, prompt("hello")).await.await_ok();
    until_idle(&h).await;
    assert!(nudges(&h).is_empty());
}
#[test]
fn a_request_with_a_long_prompt_costs_its_tier_counting_cached_input() {
    let mut model = crate::llm::catalog::Catalog::bundled().model("anthropic", "claude-sonnet-4-5").unwrap().clone();
    model.cost = serde_json::from_str(r#"{ "input": 3, "output": 15, "cache_read": 0.3, "context_over_200k": { "input": 6, "output": 22.5, "cache_read": 0.6 } }"#).unwrap();
    let short = cost(&model, Usage { input: 1_000_000, output: 0, cache_read: 0, cache_write: 0 });
    assert!((short - 6.0).abs() < 1e-9, "a million fresh tokens in one prompt is over 200k: {short}");
    let cached = cost(&model, Usage { input: 10_000, output: 1_000_000, cache_read: 250_000, cache_write: 0 });
    assert!((cached - (0.06 + 22.5 + 0.15)).abs() < 1e-9, "cached input counts toward the prompt's length: {cached}");
    let small = cost(&model, Usage { input: 100_000, output: 0, cache_read: 0, cache_write: 0 });
    assert!((small - 0.3).abs() < 1e-9, "{small}");
}
#[tokio::test]
async fn auto_accept_answers_every_ask_and_only_a_deny_rule_still_refuses() {
    let h = harness().await;
    asks_for(&h, "bash");
    let mut rx = h.engine.hub.attach(None).rx;
    h.provider.push(tool_call("bash", r#"{"command": "echo inside"}"#)).push(text("done"));
    h.engine.submit(&h.session.id, prompt("go")).await.await_ok();
    let waiting = next_ask(&mut rx).await;
    assert_eq!(waiting.ask.pattern, "echo inside", "a rule asks");
    // Turning it on answers the ask already waiting.
    assert!(h.engine.set_session_auto_accept(&h.session.id, true).unwrap().unwrap().auto_accept, "stored on the session");
    until_idle(&h).await;
    let Part::ToolCall { status, .. } = &h.engine.store.transcript(&h.session.id).unwrap()[1].parts[0].part else { panic!() };
    assert_eq!(*status, ToolStatus::Done);
    let ask = |ask: crate::tool::Ask| h.engine.permissions.decide_now(&h.session.id, &Policy::default(), &ask);
    let ws = h._dir.join("ws");
    let bash = |line: &str| crate::tool::Ask::shell(crate::tool::command::Dialect::Bash, line, line);
    assert_eq!(ask(crate::tool::Ask { default_allow: true, ..bash("git push") }), Decision::Allow, "what only a rule asks about");
    assert_eq!(ask(crate::tool::Ask::path("edit", &ws.join("drift.json"), &ws, "Edit")), Decision::Allow, "a guarded workspace file");
    assert_eq!(ask(crate::tool::Ask::path("read", &ws.join(".env"), &ws, "Read")), Decision::Allow, "a secret");
    assert_eq!(ask(crate::tool::Ask::path("edit", &h._dir.join("elsewhere.txt"), &ws, "Edit")), Decision::Allow, "outside the workspace");
    assert_eq!(ask(bash("cat ../notes")), Decision::Allow, "a line reaching outside");
    assert_eq!(ask(crate::tool::Ask::shell(crate::tool::command::Dialect::Bash, "powershell.exe -Command \"Get-Process\"", "run")), Decision::Allow, "a line that hides what it runs");
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "rm *".into(), decision: Decision::Deny }] });
    assert_eq!(ask(bash("rm -rf dist")), Decision::Deny, "a deny rule never asks, so auto-accept never answers it");
    asks_for(&h, "bash");
    h.engine.set_session_auto_accept(&h.session.id, false).unwrap();
    assert_eq!(ask(crate::tool::Ask { default_allow: true, ..bash("git push") }), Decision::Ask, "off again");
    h.engine.set_auto_accept_all(true).unwrap();
    assert_eq!(ask(crate::tool::Ask { default_allow: true, ..bash("git push") }), Decision::Allow, "or on for every session");
}
#[tokio::test]
async fn a_subscription_turn_is_priced_at_the_api_rates_it_saves() {
    let h = harness().await;
    let far = crate::id::now_ms() + 3_600_000;
    h.engine.credentials.set("anthropic", &Credential::OAuth { access: "a".into(), refresh: "r".into(), expires_at: far, account: None }).unwrap();
    h.provider.push(text("hello"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_idle(&h).await;
    let reply = h.engine.store.transcript(&h.session.id).unwrap().pop().unwrap().info;
    assert!(reply.cost > 0.0, "a signed-in turn shows what the API would have charged: {}", reply.cost);
}

#[tokio::test]
async fn a_finished_reply_tells_the_ui_its_conversation_moved_up() {
    let h = harness().await;
    let mut rx = h.engine.hub.attach(None).rx;
    h.provider.push(text("done"));
    h.engine.submit(&h.session.id, prompt("go")).await.await_ok();
    until_idle(&h).await;
    let (mut replying, mut moved) = (false, false);
    while let Ok(envelope) = rx.try_recv() {
        match envelope.event {
            crate::event::Event::MessageCreated { message } if message.role == Role::Assistant => replying = true,
            crate::event::Event::SessionUpdated { .. } if replying => moved = true,
            _ => {}
        }
    }
    assert!(moved, "a session.updated follows the reply, not only the prompt");
}
