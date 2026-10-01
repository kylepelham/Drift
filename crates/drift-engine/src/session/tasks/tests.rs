use std::time::Duration;

use serde_json::json;

use super::*;
use crate::llm::Chunk;
use crate::permission::{Reply, ReplyBody};
use crate::session::turn::tests::{harness, prompt, text, tool_call, until_idle, Harness};
use crate::session::types::{MessageWithParts, ToolStatus};
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

fn context(h: &Harness, session_id: &str, call_id: &str) -> crate::tool::Context {
    crate::tool::Context {
        workspace: h._dir.join("ws"),
        session_id: session_id.into(),
        message_id: "msg".into(),
        call_id: call_id.into(),
        files: Default::default(),
        abort: CancellationToken::new(),
        engine: h.engine.clone(),
    }
}

/// A worker of the harness session recorded as launched by `call_id`, at the owner's current Stop count.
fn recorded(h: &Harness, call_id: &str, mode: Mode) -> TaskRecord {
    let (_, generation) = h.engine.worker_scope(&h.session.id);
    let new = NewTask { generation, ..crate::store::tasks::tests::new_task(&h.session.id, call_id, mode) };
    h.engine.store.launch_task(new, crate::store::tasks::tests::child(&h.session)).unwrap().task
}

/// A request whose conversation starts with `text`: a worker's own, not its parent's quoting the launch.
fn opens_with(request: &crate::llm::Request, text: &str) -> bool {
    matches!(request.messages.first().and_then(|m| m.blocks.first()), Some(crate::llm::Block::Text(first)) if first == text)
}

fn with_model(h: &Harness) {
    h.engine.store.update_session(&h.session.id, None, Some(&crate::session::turn::tests::model()), None).unwrap();
}

/// A finished call of the parent's, as the turn would save it, carrying `metadata`.
fn call_row(h: &Harness, call_id: &str) -> PartRow {
    let message = h.engine.store.create_message(&h.session.id, Role::Assistant, Some(&crate::session::turn::tests::model())).unwrap();
    let call = Part::ToolCall { call_id: call_id.into(), name: "task_output".into(), input: json!({}), status: ToolStatus::Running, title: None, output: None, metadata: None, started_at: Some(1), finished_at: None };
    h.engine.store.add_part(&message.id, &h.session.id, call).unwrap()
}

#[tokio::test]
async fn task_output_answers_for_this_conversations_tasks_and_waits_only_as_asked() {
    let h = harness().await;
    let running = recorded(&h, "a", Mode::Background);
    h.engine.store.start_task(&running.id).unwrap();
    let ctx = context(&h, &h.session.id, "call");
    let started = std::time::Instant::now();
    let out = crate::tool::task::TaskOutput.run(&ctx, json!({ "task_id": running.id, "wait_seconds": 1 })).await.unwrap();
    assert!(out.output.ends_with(": running") && started.elapsed() >= Duration::from_millis(900), "{}", out.output);
    assert!(out.metadata.get("delivers").is_none());

    h.engine.end_task(&running.id, TaskState::Replied, "the answer");
    let out = crate::tool::task::TaskOutput.run(&ctx, json!({ "task_id": running.id })).await.unwrap();
    assert!(out.output.ends_with("replied\n\nthe answer"), "{}", out.output);
    assert_eq!(out.metadata["delivers"], running.id, "this call hands it over");
    assert!(!h.engine.store.task(&running.id).unwrap().unwrap().delivered, "not until the call's result is saved");

    let other = h.engine.store.create_session(NewSession { workspace_id: &h.session.workspace_id, parent_id: None, visibility: crate::session::types::Visibility::Sibling, title: "Other", agent: "build", model: None }).unwrap();
    let foreign = crate::tool::task::TaskOutput.run(&context(&h, &other.id, "call"), json!({ "task_id": running.id })).await.unwrap_err();
    assert!(foreign.0.contains("not launched from this conversation"));
}

#[tokio::test]
async fn a_result_being_read_is_not_delivered_again_and_one_being_delivered_is_not_read_again() {
    let h = harness().await;
    with_model(&h);
    let task = recorded(&h, "launch", Mode::Background);
    h.engine.store.start_task(&task.id).unwrap();
    let ctx = context(&h, &h.session.id, "reading");
    // task_output claims while it waits; the worker finishes meanwhile and automatic delivery stands aside.
    let reading = tokio::spawn({
        let input = json!({ "task_id": task.id, "wait_seconds": 5 });
        async move { crate::tool::task::TaskOutput.run(&ctx, input).await.unwrap() }
    });
    until("the read claims it", || h.engine.workers.holds(&task.id, &Claimant::call(&h.session.id, "reading"))).await;
    h.engine.end_task(&task.id, TaskState::Replied, "the answer");
    h.engine.deliver(&task.id).await;
    let out = reading.await.unwrap();
    assert_eq!(out.metadata["delivers"], task.id);
    assert!(delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()).is_empty(), "no second copy as a message");

    // The turn saves the result and marks it handed over in one write, then lets go of the claim.
    let mut row = call_row(&h, "reading");
    h.engine.settle_delivering(&mut row, ToolStatus::Done, None, out.output.clone(), Some(out.metadata.clone()), Some(&task.id));
    h.engine.release_claims(&Claimant::call(&h.session.id, "reading"));
    assert!(h.engine.store.task(&task.id).unwrap().unwrap().delivered);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()).is_empty() && !h.engine.turns.is_running(&h.session.id));

    // The other way round: a delivery under way keeps the result to itself.
    let second = recorded(&h, "second", Mode::Background);
    h.engine.end_task(&second.id, TaskState::Replied, "second answer");
    assert!(h.engine.workers.claim(&second.id, Claimant::Automatic));
    let out = crate::tool::task::TaskOutput.run(&context(&h, &h.session.id, "late"), json!({ "task_id": second.id })).await.unwrap();
    assert!(out.output.ends_with("arriving in this conversation as a message.") && !out.output.contains("second answer"), "{}", out.output);
    assert!(out.metadata.get("delivers").is_none());
}

#[tokio::test]
async fn a_result_whose_call_was_not_saved_is_still_owed_and_arrives_as_a_message() {
    let h = harness().await;
    with_model(&h);
    h.provider.push(text("noted the answer"));
    let task = recorded(&h, "launch", Mode::Background);
    h.engine.end_task(&task.id, TaskState::Replied, "the answer");
    let out = crate::tool::task::TaskOutput.run(&context(&h, &h.session.id, "reading"), json!({ "task_id": task.id })).await.unwrap();
    assert_eq!(out.metadata["delivers"], task.id);
    let mut row = call_row(&h, "reading");
    h.engine.store.lock().execute_batch("CREATE TEMP TRIGGER no_room BEFORE UPDATE ON part WHEN NEW.json LIKE '%the answer%' BEGIN SELECT RAISE(ABORT, 'disk is full'); END;").unwrap();
    h.engine.settle_delivering(&mut row, ToolStatus::Done, None, out.output.clone(), Some(out.metadata.clone()), Some(&task.id));
    h.engine.store.lock().execute_batch("DROP TRIGGER no_room;").unwrap();
    let Part::ToolCall { status, output, .. } = &row.part else { panic!() };
    assert_eq!(*status, ToolStatus::Error, "{output:?}");
    assert!(!h.engine.store.task(&task.id).unwrap().unwrap().delivered, "nothing saved, nothing handed over");

    h.engine.release_claims(&Claimant::call(&h.session.id, "reading"));
    until("delivered as a message instead", || h.engine.store.task(&task.id).unwrap().unwrap().delivered).await;
    until_idle(&h).await;
    assert_eq!(delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()), [("launch".to_string(), "the answer".to_string())]);
}

#[tokio::test]
async fn a_stop_while_a_result_waits_to_be_admitted_keeps_it_from_starting_a_turn() {
    let h = harness().await;
    with_model(&h);
    let task = recorded(&h, "launch", Mode::Background);
    h.engine.end_task(&task.id, TaskState::Replied, "late answer");
    // A job that takes no prompts (a compaction, say) holds the parent; the result waits it out.
    let job = CancellationToken::new();
    assert!(h.engine.turns.claim(&h.session.id, &job));
    let delivering = tokio::spawn({
        let (engine, id) = (h.engine.clone(), task.id.clone());
        async move { engine.deliver(&id).await }
    });
    until("the delivery is waiting", || h.engine.workers.holds(&task.id, &Claimant::Automatic)).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(h.engine.abort(&h.session.id));
    tokio::time::timeout(Duration::from_secs(5), delivering).await.expect("the wait ends with the Stop").unwrap();
    h.engine.turns.release(&h.session.id);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(h.engine.store.task(&task.id).unwrap().unwrap().delivered, "settled, not left owed");
    assert!(delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()).is_empty(), "the stopped parent was not woken");
    assert!(!h.engine.turns.is_running(&h.session.id) && h.provider.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_result_launched_before_a_stop_never_wakes_the_parent_even_after_a_restart() {
    let h = harness().await;
    with_model(&h);
    let before = recorded(&h, "before", Mode::Background);
    h.engine.end_task(&before.id, TaskState::Replied, "from before the stop");
    h.engine.abort(&h.session.id);
    let after = recorded(&h, "after", Mode::Background);
    h.engine.end_task(&after.id, TaskState::Replied, "from after the stop");
    h.provider.push(text("noted"));

    // A new engine on the same data: nothing in memory remembers the Stop.
    let restarted = crate::Engine::open_with(&h._dir.join("data"), crate::Options { file_credentials: true, ..Default::default() }).unwrap();
    *restarted.turns.provider_override.lock().unwrap() = h.engine.turns.provider_override.lock().unwrap().clone();
    restarted.recover_tasks().await;
    until("both settled", || [&before, &after].iter().all(|t| restarted.store.task(&t.id).unwrap().unwrap().delivered)).await;
    until("the parent's turn ends", || !restarted.turns.is_running(&h.session.id)).await;
    let transcript = restarted.store.transcript(&h.session.id).unwrap();
    assert_eq!(delivered_results(&transcript), [("after".to_string(), "from after the stop".to_string())], "only the result launched after the stop arrives");
}

#[tokio::test]
async fn a_foreground_result_left_owed_by_a_restart_goes_to_its_own_call_never_a_new_prompt() {
    let h = harness().await;
    with_model(&h);
    let task = recorded(&h, "fg_call", Mode::Foreground);
    h.engine.end_task(&task.id, TaskState::Replied, "foreground answer");
    let row = call_row(&h, "fg_call");
    h.engine.recover_tasks().await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert!(delivered_results(&transcript).is_empty(), "not a background notification");
    let saved = transcript.iter().flat_map(|m| &m.parts).find(|p| p.id == row.id).unwrap();
    let Part::ToolCall { status, output, .. } = &saved.part else { panic!() };
    assert_eq!((*status, output.as_deref()), (ToolStatus::Done, Some("foreground answer")));
    assert!(h.engine.store.task(&task.id).unwrap().unwrap().delivered);
    assert!(!h.engine.turns.is_running(&h.session.id));
}

#[tokio::test]
async fn a_worker_stopped_between_its_start_and_its_turn_never_runs() {
    let h = harness().await;
    let launched: Vec<_> = (0..=MAX_BACKGROUND).map(|i| background(&format!("Job {i}"), &format!("CHILD {i} wait"))).collect();
    h.provider.push_for("PARENT", launches(&launched)).push_for("PARENT", text("carrying on"));
    for i in 0..=MAX_BACKGROUND {
        h.provider.push_stall_for(&format!("CHILD {i} wait"));
    }
    h.engine.submit(&h.session.id, prompt("PARENT many")).await.unwrap();
    until("one waits for a slot", || tasks(&h).iter().filter(|t| t.state == TaskState::Running).count() == MAX_BACKGROUND && tasks(&h).iter().any(|t| t.state == TaskState::Queued)).await;
    let queued = tasks(&h).into_iter().find(|t| t.state == TaskState::Queued).unwrap();
    // Its transcript is held, so once started it sits between marked running and claiming its turn.
    let holder = CancellationToken::new();
    assert!(h.engine.turns.claim(&queued.session_id, &holder));
    let freed = tasks(&h).into_iter().find(|t| t.state == TaskState::Running).unwrap();
    h.engine.stop_task(&freed.id).unwrap();
    until("it starts", || h.engine.store.task(&queued.id).unwrap().unwrap().state == TaskState::Running).await;
    h.engine.stop_task(&queued.id).unwrap();
    until("it stops", || h.engine.store.task(&queued.id).unwrap().unwrap().state == TaskState::Stopped).await;
    h.engine.turns.release(&queued.session_id);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let prompt_text = format!("{} wait", queued.description.replace("Job", "CHILD"));
    let asked = h.provider.requests.lock().unwrap().iter().any(|r| opens_with(r, &prompt_text));
    assert!(!asked, "its turn never reached the model");
    assert!(h.engine.store.transcript(&queued.session_id).unwrap().is_empty(), "nothing was admitted");
    h.engine.abort(&h.session.id);
}

#[tokio::test]
async fn a_queued_worker_runs_as_it_was_admitted_not_as_settings_changed_since() {
    let h = harness().await;
    let set_prompt = |text: &str| {
        let mut overrides = std::collections::HashMap::new();
        overrides.insert("general".to_string(), crate::config::AgentOverride { model: None, prompt: Some(text.into()) });
        h.engine.set_agent_overrides(overrides);
    };
    set_prompt("PROMPT-ALPHA");
    let mut launched: Vec<_> = (0..MAX_BACKGROUND).map(|i| background(&format!("Job {i}"), &format!("CHILD {i} wait"))).collect();
    launched.push(background("Queued", "CHILD queued work"));
    h.provider.push_for("PARENT", launches(&launched)).push_for("PARENT", text("carrying on")).push_for("CHILD queued", text("queued result"));
    for i in 0..MAX_BACKGROUND {
        h.provider.push_stall_for(&format!("CHILD {i} wait"));
    }
    h.engine.submit(&h.session.id, prompt("PARENT many")).await.unwrap();
    until("the last one queues", || tasks(&h).iter().any(|t| t.description == "Queued" && t.state == TaskState::Queued) && tasks(&h).iter().filter(|t| t.state == TaskState::Running).count() == MAX_BACKGROUND).await;
    set_prompt("PROMPT-BETA");
    let freed = tasks(&h).into_iter().find(|t| t.state == TaskState::Running).unwrap();
    h.engine.stop_task(&freed.id).unwrap();
    until("the queued one replies", || tasks(&h).iter().any(|t| t.description == "Queued" && t.state == TaskState::Replied)).await;
    let requests = h.provider.requests.lock().unwrap();
    let ran = requests.iter().find(|r| opens_with(r, "CHILD queued work")).unwrap();
    assert!(ran.system.contains("PROMPT-ALPHA") && !ran.system.contains("PROMPT-BETA"), "the prompt it was admitted with");
    drop(requests);
    h.engine.abort(&h.session.id);
}

#[tokio::test]
async fn the_same_launch_again_gets_what_it_launched_and_makes_nothing_new() {
    let h = harness().await;
    with_model(&h);
    h.provider.push_stall_for("CHILD again").push_for("CHILD front", text("front answer"));
    let children = |h: &Harness| h.engine.store.sessions(crate::store::SessionFilter { workspace_id: Some(&h.session.workspace_id), archived: false, before: None, limit: 50 }).unwrap().into_iter().filter(|s| s.parent_id.as_deref() == Some(h.session.id.as_str())).count();

    let ctx = context(&h, &h.session.id, "bg_call");
    let first = crate::tool::task::Task.run(&ctx, background("Again", "CHILD again")).await.unwrap();
    let second = crate::tool::task::Task.run(&ctx, background("Again", "CHILD again")).await.unwrap();
    assert_eq!(first.metadata["taskId"], second.metadata["taskId"]);
    assert!(second.output.starts_with("Started Again in the background"), "{}", second.output);
    assert_eq!((children(&h), tasks(&h).len()), (1, 1), "no second transcript, no second worker");

    let ctx = context(&h, &h.session.id, "fg_call");
    let front = json!({ "description": "Front", "prompt": "CHILD front" });
    let first = crate::tool::task::Task.run(&ctx, front.clone()).await.unwrap();
    assert_eq!(first.output, "front answer");
    let again = crate::tool::task::Task.run(&ctx, front).await.unwrap();
    assert_eq!((again.output.as_str(), again.metadata["mode"].as_str()), ("front answer", Some("foreground")), "a foreground replay is its result, not a background receipt");
    assert_eq!((children(&h), tasks(&h).len()), (2, 2));
    h.engine.abort(&h.session.id);
}

#[tokio::test]
async fn after_a_restart_unfinished_workers_are_interrupted_and_finished_results_arrive_once() {
    let h = harness().await;
    let was_running = recorded(&h, "was running", Mode::Background);
    h.engine.store.start_task(&was_running.id).unwrap();
    let finished = recorded(&h, "finished", Mode::Background);
    h.engine.store.finish_task(&finished.id, TaskState::Replied, "done before the restart").unwrap();
    with_model(&h);
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
