use std::time::Duration;

use serde_json::json;

use super::*;
use crate::llm::Chunk;
use crate::permission::{Reply, ReplyBody};
use crate::session::turn::tests::{harness, prompt, text, tool_call, until_idle, Harness};
use crate::session::types::MessageWithParts;
use crate::store::{NewSession, NewTask};
use crate::tool::Tool;

#[test]
fn the_mode_is_what_was_asked_then_the_agents_default_then_the_foreground() {
    assert_eq!(resolve_mode(Some(true), Some(false), true), Ok((Mode::Background, "requested")));
    assert_eq!(resolve_mode(Some(false), Some(true), true), Ok((Mode::Foreground, "requested")));
    assert_eq!(resolve_mode(None, Some(true), true), Ok((Mode::Background, "agent default")));
    assert_eq!(resolve_mode(None, Some(false), true), Ok((Mode::Foreground, "agent default")));
    assert_eq!(resolve_mode(None, None, true), Ok((Mode::Foreground, "default")));
    assert!(resolve_mode(Some(true), None, false).unwrap_err().contains("turned off"), "an explicit request is refused, not quietly changed");
    assert_eq!(resolve_mode(None, Some(true), false), Ok((Mode::Foreground, "background turned off")));
}

fn background(description: &str, child_prompt: &str) -> serde_json::Value {
    json!({ "description": description, "prompt": child_prompt, "run_in_background": true })
}

/// One assistant message launching every task given, in order.
fn launches(tasks: &[serde_json::Value]) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    for (index, input) in tasks.iter().enumerate() {
        chunks.push(Chunk::ToolUseStart { id: format!("launch_{index}"), name: "task".into() });
        chunks.push(Chunk::ToolInputDelta(input.to_string()));
        chunks.push(Chunk::BlockStop);
    }
    chunks.push(Chunk::Stop(crate::llm::StopReason::ToolUse));
    chunks
}

async fn until<F: Fn() -> bool>(what: &str, done: F) {
    for _ in 0..600 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("never: {what}");
}

fn tasks(h: &Harness) -> Vec<TaskRecord> {
    h.engine.store.tasks_of(&h.session.id).unwrap()
}

fn delivered_results(transcript: &[MessageWithParts]) -> Vec<(String, String)> {
    transcript
        .iter()
        .flat_map(|m| &m.parts)
        .filter_map(|row| match &row.part {
            Part::TaskResult { description, text, .. } => Some((description.clone(), text.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_background_worker_returns_a_receipt_and_its_result_arrives_later() {
    let h = harness().await;
    h.provider
        .push_for("PARENT", launches(&[background("Survey", "CHILD survey the code")]))
        .push_for("PARENT", text("working meanwhile"))
        .push_for("PARENT", text("thanks for the survey"))
        .push_slow_for("CHILD survey", Duration::from_millis(700), text("three things found"));
    h.engine.submit(&h.session.id, prompt("PARENT goal")).await.unwrap();
    until("the parent's own turn ends", || !h.engine.turns.is_running(&h.session.id)).await;
    let launched = tasks(&h).pop().unwrap();
    assert!(!launched.state.is_terminal(), "the parent finished its turn while the worker still ran");
    assert_eq!((launched.mode, launched.reason.as_str()), (Mode::Background, "requested"));
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { output, status, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, crate::session::types::ToolStatus::Done, "a launch is a successful call");
    assert!(output.as_deref().unwrap().starts_with("Started Survey in the background"), "{output:?}");

    until("the result is delivered", || tasks(&h)[0].delivered).await;
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert_eq!(delivered_results(&transcript), [("Survey".to_string(), "three things found".to_string())], "delivered once");
    let texts: Vec<String> = h.provider.requests.lock().unwrap().iter().flat_map(|r| r.messages.iter().flat_map(|m| m.blocks.clone())).filter_map(|b| if let crate::llm::Block::Text(t) = b { Some(t) } else { None }).collect();
    assert!(texts.iter().any(|t| t.starts_with("<task-result") && t.contains("three things found")), "the parent's model saw the result");
    assert_eq!(h.provider.responses_left(), 0, "the result started the parent's next turn");
}

#[tokio::test]
async fn workers_finish_out_of_order_each_with_its_own_result() {
    let h = harness().await;
    h.provider
        .push_for("PARENT", launches(&[background("Slow one", "CHILD slow"), background("Quick one", "CHILD quick")]))
        .push_for("PARENT", text("both launched"))
        .push_for("PARENT", text("noted one"))
        .push_for("PARENT", text("noted two"))
        .push_slow_for("CHILD slow", Duration::from_millis(800), text("slow result"))
        .push_for("CHILD quick", text("quick result"));
    h.engine.submit(&h.session.id, prompt("PARENT two jobs")).await.unwrap();
    until("both delivered", || tasks(&h).len() == 2 && tasks(&h).iter().all(|t| t.delivered)).await;
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let delivered = delivered_results(&transcript);
    assert_eq!(delivered, [("Quick one".to_string(), "quick result".to_string()), ("Slow one".to_string(), "slow result".to_string())], "in the order they finished");
}

#[tokio::test]
async fn background_slots_are_bounded_and_session_stop_ends_them_even_when_idle() {
    let h = harness().await;
    let launched: Vec<_> = (0..MAX_BACKGROUND + 1).map(|i| background(&format!("Job {i}"), &format!("CHILD {i} wait"))).collect();
    h.provider.push_for("PARENT", launches(&launched)).push_for("PARENT", text("carrying on"));
    for i in 0..=MAX_BACKGROUND {
        h.provider.push_stall_for(&format!("CHILD {i} wait"));
    }
    h.engine.submit(&h.session.id, prompt("PARENT many")).await.unwrap();
    until("the parent's turn ends", || !h.engine.turns.is_running(&h.session.id)).await;
    until("the slots fill", || tasks(&h).iter().filter(|t| t.state == TaskState::Running).count() == MAX_BACKGROUND).await;
    assert_eq!(tasks(&h).iter().filter(|t| t.state == TaskState::Queued).count(), 1, "one waits for a slot");

    let requests = h.provider.requests.lock().unwrap().len();
    assert!(h.engine.abort(&h.session.id), "Stop has something to stop with the parent idle");
    until("all stopped", || tasks(&h).len() == MAX_BACKGROUND + 1 && tasks(&h).iter().all(|t| t.state == TaskState::Stopped && t.delivered)).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(h.provider.requests.lock().unwrap().len(), requests, "no worker started after the stop and the parent was not woken");
    assert!(delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()).is_empty());
}

#[tokio::test]
async fn stopping_one_worker_leaves_the_others_running() {
    let h = harness().await;
    h.provider
        .push_for("PARENT", launches(&[background("First", "CHILD first"), background("Second", "CHILD second")]))
        .push_for("PARENT", text("launched"))
        .push_stall_for("CHILD first")
        .push_stall_for("CHILD second");
    h.engine.submit(&h.session.id, prompt("PARENT pair")).await.unwrap();
    until("both run", || tasks(&h).len() == 2 && tasks(&h).iter().all(|t| t.state == TaskState::Running)).await;
    let first = tasks(&h).into_iter().find(|t| t.description == "First").unwrap();
    h.engine.stop_task(&first.id).unwrap();
    until("the first stops", || h.engine.store.task(&first.id).unwrap().unwrap().state == TaskState::Stopped).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let second = tasks(&h).into_iter().find(|t| t.description == "Second").unwrap();
    assert_eq!(second.state, TaskState::Running, "only the one asked for stops");
    assert!(!h.engine.turns.is_running(&h.session.id), "a stopped worker does not wake an idle parent");
    h.engine.abort(&h.session.id);
}

#[tokio::test]
async fn a_workers_permission_wait_blocks_only_that_worker() {
    let h = harness().await;
    let mut rx = h.engine.hub.attach(None).rx;
    h.provider
        .push_for("PARENT", launches(&[background("Build", "CHILD build it")]))
        .push_for("PARENT", text("launched the build"))
        .push_for("PARENT", text("build result noted"))
        .push_for("CHILD build", tool_call("bash", r#"{"command": "echo built"}"#))
        .push_for("CHILD build", text("built"));
    h.engine.submit(&h.session.id, prompt("PARENT build")).await.unwrap();
    let ask = loop {
        let envelope = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        if let Event::PermissionAsked { request } = envelope.event {
            break request;
        }
    };
    let worker = tasks(&h).pop().unwrap();
    assert_eq!(ask.session_id, worker.session_id, "attributed to the worker");
    until("the parent's turn ends while the worker waits", || !h.engine.turns.is_running(&h.session.id)).await;
    h.engine.permissions.reply(&h.engine.hub, &ask.id, ReplyBody { reply: Reply::Once, pattern: None, message: None }).unwrap();
    until("delivered", || tasks(&h)[0].delivered).await;
    until_idle(&h).await;
    assert_eq!(tasks(&h)[0].state, TaskState::Replied);
}

fn context(h: &Harness, session_id: &str) -> crate::tool::Context {
    crate::tool::Context {
        workspace: h._dir.join("ws"),
        session_id: session_id.into(),
        message_id: "msg".into(),
        call_id: "call".into(),
        files: Default::default(),
        abort: CancellationToken::new(),
        engine: h.engine.clone(),
    }
}

#[tokio::test]
async fn task_output_answers_for_this_conversations_tasks_and_waits_only_as_asked() {
    let h = harness().await;
    let new = |call: &'static str| NewTask { parent_session_id: &h.session.id, session_id: "ses_worker", call_id: call, description: "Look", agent: "general", mode: Mode::Background, reason: "requested" };
    let (running, _) = h.engine.store.create_task(new("a")).unwrap();
    h.engine.store.start_task(&running.id).unwrap();
    let ctx = context(&h, &h.session.id);
    let started = std::time::Instant::now();
    let out = crate::tool::task::TaskOutput.run(&ctx, json!({ "task_id": running.id, "wait_seconds": 1 })).await.unwrap();
    assert!(out.output.ends_with(": running") && started.elapsed() >= Duration::from_millis(900), "{}", out.output);

    h.engine.end_task(&running.id, TaskState::Replied, "the answer");
    let out = crate::tool::task::TaskOutput.run(&ctx, json!({ "task_id": running.id })).await.unwrap();
    assert!(out.output.ends_with("replied\n\nthe answer"), "{}", out.output);
    assert!(h.engine.store.task(&running.id).unwrap().unwrap().delivered, "read here, it is not delivered again");

    let other = h.engine.store.create_session(NewSession { workspace_id: &h.session.workspace_id, parent_id: None, visibility: crate::session::types::Visibility::Sibling, title: "Other", agent: "build", model: None }).unwrap();
    let foreign = crate::tool::task::TaskOutput.run(&context(&h, &other.id), json!({ "task_id": running.id })).await.unwrap_err();
    assert!(foreign.0.contains("not launched from this conversation"));
}

#[tokio::test]
async fn after_a_restart_unfinished_workers_are_interrupted_and_finished_results_arrive_once() {
    let h = harness().await;
    let new = |call: &'static str, session: &'static str| NewTask { parent_session_id: &h.session.id, session_id: session, call_id: call, description: call, agent: "general", mode: Mode::Background, reason: "requested" };
    let (was_running, _) = h.engine.store.create_task(new("was running", "ses_a")).unwrap();
    h.engine.store.start_task(&was_running.id).unwrap();
    let (finished, _) = h.engine.store.create_task(new("finished", "ses_b")).unwrap();
    h.engine.store.finish_task(&finished.id, TaskState::Replied, "done before the restart").unwrap();
    let model = crate::session::turn::tests::model();
    h.engine.store.update_session(&h.session.id, None, Some(&model), None).unwrap();
    h.provider.push(text("noted"));

    // What opening the store after a restart does, then what the engine does once it listens.
    h.engine.store.interrupt_unfinished_tasks().unwrap();
    h.engine.recover_tasks().await;
    until_idle(&h).await;
    h.engine.recover_tasks().await;
    until_idle(&h).await;
    let interrupted = h.engine.store.task(&was_running.id).unwrap().unwrap();
    assert_eq!((interrupted.state, interrupted.delivered), (TaskState::Interrupted, true), "never rerun");
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert_eq!(delivered_results(&transcript), [("finished".to_string(), "done before the restart".to_string())], "once, however often recovery runs");
    assert_eq!(h.provider.requests.lock().unwrap().len(), 1, "only the delivered result started a turn");
}

#[tokio::test]
async fn a_worker_cut_off_at_its_output_limit_is_incomplete_not_an_answer() {
    let h = harness().await;
    let cut_off = vec![Chunk::TextStart, Chunk::TextDelta("The first half of an ans".into()), Chunk::BlockStop, Chunk::Stop(crate::llm::StopReason::MaxTokens)];
    h.provider
        .push_for("PARENT", launches(&[json!({ "description": "Write it up", "prompt": "CHILD write a long report" })]))
        .push_for("PARENT", text("it did not finish"))
        .push_for("CHILD write", cut_off);
    h.engine.submit(&h.session.id, prompt("PARENT report")).await.unwrap();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, metadata, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, crate::session::types::ToolStatus::Error, "not a successful task");
    assert_eq!(metadata.as_ref().unwrap()["outcome"], "incomplete");
    let output = output.as_deref().unwrap();
    assert!(output.contains("not a complete answer") && output.contains("The first half of an ans"), "the partial text is kept: {output}");
    assert_eq!(tasks(&h)[0].state, TaskState::Failed);
}

#[tokio::test]
async fn with_background_turned_off_an_explicit_request_is_refused_and_foreground_still_waits() {
    let h = harness().await;
    h.engine.store.set_setting(BACKGROUND_TASKS_KEY, &false).unwrap();
    h.provider
        .push_for("PARENT", launches(&[background("Refused", "CHILD refused")]))
        .push_for("PARENT", launches(&[json!({ "description": "Waited", "prompt": "CHILD waited" })]))
        .push_for("PARENT", text("done"))
        .push_for("CHILD waited", text("waited result"));
    h.engine.submit(&h.session.id, prompt("PARENT off")).await.unwrap();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { output, .. } = &transcript[1].parts[0].part else { panic!() };
    assert!(output.as_deref().unwrap().contains("turned off"), "{output:?}");
    let recorded = tasks(&h);
    assert_eq!(recorded.len(), 1);
    assert_eq!((recorded[0].mode, recorded[0].state, recorded[0].delivered), (Mode::Foreground, TaskState::Replied, true));
    let Part::ToolCall { output, .. } = &transcript[2].parts[0].part else { panic!() };
    assert_eq!(output.as_deref(), Some("waited result"), "foreground returns the result as the call's own");
}
