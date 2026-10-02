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
async fn a_worker_thinks_at_its_parents_reasoning_level() {
    let h = harness().await;
    h.engine.store.set_session_variant(&h.session.id, Some("max")).unwrap();
    h.provider
        .push_for("PARENT", launches(&[json!({ "description": "Look", "prompt": "CHILD look around" })]))
        .push_for("CHILD", text("found it"))
        .push_for("PARENT", text("done"));
    h.engine.submit(&h.session.id, prompt("PARENT go")).await.unwrap();
    until_idle(&h).await;
    let child = h.engine.store.session(&tasks(&h)[0].session_id).unwrap().unwrap();
    assert_eq!(child.variant.as_deref(), Some("max"));
    let requests = h.provider.requests.lock().unwrap();
    let asked_child = requests.iter().find(|r| format!("{:?}", r.messages).contains("CHILD look around")).unwrap();
    assert!(matches!(asked_child.reasoning, Some(crate::llm::catalog::Reasoning::Budget { .. })), "{:?}", asked_child.reasoning);
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
    until("all stopped and held", || tasks(&h).len() == MAX_BACKGROUND + 1 && tasks(&h).iter().all(|t| t.state == TaskState::Stopped && t.held && !t.delivered)).await;
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
        config: Arc::new(h.engine.workspace_config(&h._dir.join("ws"))),
        progress: Default::default(),
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
async fn a_foreground_result_stays_its_launching_calls_even_when_saving_it_fails() {
    let h = harness().await;
    with_model(&h);
    h.provider.push_for("CHILD front", text("front answer"));
    let out = crate::tool::task::Task.run(&context(&h, &h.session.id, "fg_call"), json!({ "description": "Front", "prompt": "CHILD front" })).await.unwrap();
    let task_id = out.metadata["taskId"].as_str().unwrap().to_string();
    assert_eq!(out.metadata["delivers"], task_id.as_str());

    // Another call cannot take it, before or after the launching call's save fails.
    let reader = context(&h, &h.session.id, "reader");
    let refused = crate::tool::task::TaskOutput.run(&reader, json!({ "task_id": task_id })).await.unwrap_err();
    assert!(refused.0.contains("foreground"), "{}", refused.0);
    assert!(h.engine.workers.holds(&task_id, &Claimant::call(&h.session.id, "fg_call")) && !h.engine.workers.holds(&task_id, &Claimant::call(&h.session.id, "reader")));

    let mut row = call_row(&h, "fg_call");
    h.engine.store.lock().execute_batch("CREATE TEMP TRIGGER no_room BEFORE UPDATE ON part WHEN NEW.json LIKE '%front answer%' BEGIN SELECT RAISE(ABORT, 'disk is full'); END;").unwrap();
    h.engine.settle_delivering(&mut row, ToolStatus::Done, None, out.output.clone(), Some(out.metadata.clone()), Some(&task_id));
    h.engine.store.lock().execute_batch("DROP TRIGGER no_room;").unwrap();
    h.engine.release_claims(&Claimant::call(&h.session.id, "fg_call"));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!h.engine.store.task(&task_id).unwrap().unwrap().delivered, "nothing saved, nothing handed over");
    assert!(delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()).is_empty(), "a foreground result never becomes a message");
    assert!(crate::tool::task::TaskOutput.run(&reader, json!({ "task_id": task_id })).await.is_err());

    // Recovery puts it into the launching call, where it belongs.
    h.engine.recover_tasks().await;
    let saved = h.engine.store.transcript(&h.session.id).unwrap().into_iter().flat_map(|m| m.parts).find(|p| p.id == row.id).unwrap();
    let Part::ToolCall { status, output, .. } = &saved.part else { panic!() };
    assert_eq!((*status, output.as_deref()), (ToolStatus::Done, Some("front answer")));
    assert!(h.engine.store.task(&task_id).unwrap().unwrap().delivered);
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
    let held = h.engine.store.task(&task.id).unwrap().unwrap();
    assert!(held.held && !held.delivered, "held for the next prompt, not marked handed over");
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

    // A new engine on the same data: nothing in memory remembers the Stop.
    let restarted = crate::Engine::open_with(&h._dir.join("data"), crate::Options { file_credentials: true, ..Default::default() }).unwrap();
    *restarted.turns.provider_override.lock().unwrap() = h.engine.turns.provider_override.lock().unwrap().clone();
    restarted.recover_tasks().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let kept = restarted.store.task(&before.id).unwrap().unwrap();
    assert!(kept.held && !kept.delivered, "held across the restart, not marked handed over");
    assert!(h.provider.requests.lock().unwrap().is_empty() && !restarted.turns.is_running(&h.session.id), "it woke nothing");
}

#[tokio::test]
async fn a_held_result_rides_along_with_a_later_permitted_delivery_once() {
    let h = harness().await;
    with_model(&h);
    let held = recorded(&h, "before", Mode::Background);
    h.engine.end_task(&held.id, TaskState::Replied, "from before the stop");
    h.engine.abort(&h.session.id);
    h.engine.deliver(&held.id).await;
    assert!(h.engine.store.task(&held.id).unwrap().unwrap().held && h.provider.requests.lock().unwrap().is_empty(), "alone it wakes nothing");

    h.provider.push(text("noted both"));
    let later = recorded(&h, "after", Mode::Background);
    h.engine.end_task(&later.id, TaskState::Replied, "from after the stop");
    h.engine.deliver(&later.id).await;
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let carried: Vec<_> = transcript.iter().filter(|m| !delivered_results(std::slice::from_ref(m)).is_empty()).collect();
    assert_eq!(carried.len(), 1, "one prompt, one turn");
    assert_eq!(delivered_results(&transcript), [("before".to_string(), "from before the stop".to_string()), ("after".to_string(), "from after the stop".to_string())]);
    assert!([&held, &later].iter().all(|t| h.engine.store.task(&t.id).unwrap().unwrap().delivered));
    assert_eq!(h.provider.requests.lock().unwrap().len(), 1);

    h.provider.push(text("ok"));
    h.engine.submit(&h.session.id, prompt("next")).await.unwrap();
    until_idle(&h).await;
    assert_eq!(delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()).len(), 2, "never carried again");
}

#[tokio::test]
async fn a_delivery_that_already_landed_carries_no_held_result_with_it() {
    let h = harness().await;
    let held = recorded(&h, "held", Mode::Background);
    h.engine.end_task(&held.id, TaskState::Replied, "held");
    h.engine.store.hold_task(&held.id).unwrap();
    let landed = recorded(&h, "landed", Mode::Background);
    h.engine.end_task(&landed.id, TaskState::Replied, "landed");
    h.engine.store.mark_task_delivered(&landed.id).unwrap();
    let handover = crate::store::Handover { delivery: Some(&landed.id), held: vec![(held.id.clone(), result_part(&held))] };
    let admitted = h.engine.store.admit_delivering(&h.session.id, crate::store::Pick::model(&crate::session::turn::tests::model()), vec![], None, handover).unwrap();
    assert!(matches!(admitted, crate::store::Admit::Delivered), "nothing written");
    let still = h.engine.store.task(&held.id).unwrap().unwrap();
    assert!(still.held && !still.delivered, "its acknowledgment went with the rest of the write");
    assert!(h.engine.store.transcript(&h.session.id).unwrap().is_empty());
}

#[tokio::test]
async fn a_result_that_found_the_parent_busy_goes_in_when_that_job_ends() {
    let h = harness().await;
    with_model(&h);
    h.provider.push(text("noted"));
    let task = recorded(&h, "launch", Mode::Background);
    h.engine.end_task(&task.id, TaskState::Replied, "busy answer");
    let gate = Arc::new(tokio::sync::Notify::new());
    assert!(h.engine.turns.claim(&h.session.id, &CancellationToken::new()));
    let waiting = gate.clone();
    h.engine.spawn_job(&h.session.id, async move { waiting.notified().await });
    h.engine.deliver(&task.id).await;
    let owed = h.engine.store.task(&task.id).unwrap().unwrap();
    assert!(!owed.delivered && owed.delivery_error.as_deref().is_some_and(|e| e.contains("busy")), "{owed:?}");
    assert!(h.provider.requests.lock().unwrap().is_empty());

    gate.notify_one();
    until("delivered once the job ends", || h.engine.store.task(&task.id).unwrap().unwrap().delivered).await;
    until_idle(&h).await;
    assert_eq!(h.engine.store.task(&task.id).unwrap().unwrap().delivery_error, None);
    assert_eq!(delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()), [("launch".to_string(), "busy answer".to_string())]);
}

#[tokio::test]
async fn a_retry_asked_for_while_an_attempt_holds_the_result_is_made_by_that_attempt() {
    let h = harness().await;
    with_model(&h);
    h.provider.push(text("noted"));
    let task = recorded(&h, "launch", Mode::Background);
    h.engine.end_task(&task.id, TaskState::Replied, "late answer");
    // Busy with something whose end sends no notification of its own.
    let busy = CancellationToken::new();
    assert!(h.engine.turns.claim(&h.session.id, &busy));
    let attempt = tokio::spawn({
        let (engine, id) = (h.engine.clone(), task.id.clone());
        async move { engine.deliver(&id).await }
    });
    until("the attempt holds it", || h.engine.workers.holds(&task.id, &Claimant::Automatic)).await;
    // A readiness notification arrives while the attempt holds the result, and finds it taken.
    h.engine.retry_deliveries(Some(&h.session.id));
    until("the first attempt fails", || h.engine.store.task(&task.id).unwrap().unwrap().delivery_error.is_some()).await;
    h.engine.turns.release(&h.session.id);
    tokio::time::timeout(Duration::from_secs(5), attempt).await.expect("the attempt ends").unwrap();
    assert!(h.engine.store.task(&task.id).unwrap().unwrap().delivered, "the notification was honoured, not dropped");
    until_idle(&h).await;
    assert_eq!(delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()).len(), 1);
}

#[tokio::test]
async fn a_result_blocked_by_a_missing_model_stays_owed_with_its_reason_until_one_is_chosen() {
    let h = harness().await;
    let task = recorded(&h, "launch", Mode::Background);
    h.engine.end_task(&task.id, TaskState::Replied, "answer");
    h.engine.deliver(&task.id).await;
    let owed = h.engine.store.task(&task.id).unwrap().unwrap();
    assert_eq!((owed.delivered, owed.delivery_error.as_deref()), (false, Some("no model selected")));
    assert!(serde_json::to_value(&owed).unwrap()["deliveryError"] == "no model selected", "exposed to the UI");
    // Nothing tries again by itself: no loop.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!h.engine.workers.holds(&task.id, &Claimant::Automatic) && h.provider.requests.lock().unwrap().is_empty());

    // Choosing a model (what PATCH /sessions/{id} does) is a repair, and the result goes in.
    h.provider.push(text("noted"));
    with_model(&h);
    h.engine.retry_deliveries(Some(&h.session.id));
    until("delivered after the repair", || h.engine.store.task(&task.id).unwrap().unwrap().delivered).await;
    until_idle(&h).await;
    assert_eq!(h.engine.store.task(&task.id).unwrap().unwrap().delivery_error, None);
}

#[tokio::test]
async fn a_result_held_by_stop_rides_along_with_the_next_prompt_once() {
    let h = harness().await;
    with_model(&h);
    let task = recorded(&h, "launch", Mode::Background);
    h.engine.end_task(&task.id, TaskState::Replied, "held answer");
    h.engine.abort(&h.session.id);
    h.engine.deliver(&task.id).await;
    assert!(h.engine.store.task(&task.id).unwrap().unwrap().held);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(h.provider.requests.lock().unwrap().is_empty(), "never wakes the stopped parent");
    // Neither another automatic attempt nor a restart delivers it.
    h.engine.deliver(&task.id).await;
    h.engine.recover_tasks().await;
    assert!(!h.engine.turns.is_running(&h.session.id) && !h.engine.store.task(&task.id).unwrap().unwrap().delivered);

    h.provider.push(text("thanks")).push(text("again"));
    h.engine.submit(&h.session.id, prompt("what next")).await.unwrap();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert_eq!(delivered_results(&transcript), [("launch".to_string(), "held answer".to_string())]);
    let user = &transcript[0];
    assert!(matches!(user.parts[0].part, Part::TaskResult { .. }) && matches!(&user.parts[1].part, Part::Text { text } if text == "what next"), "carried in the user's own prompt, ahead of it");
    assert!(h.engine.store.task(&task.id).unwrap().unwrap().delivered);

    h.engine.submit(&h.session.id, prompt("and then")).await.unwrap();
    until_idle(&h).await;
    assert_eq!(delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()).len(), 1, "once only");
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
        overrides.insert("general".to_string(), crate::config::AgentOverride { prompt: Some(text.into()), ..Default::default() });
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
    assert!(first.output.starts_with("front answer\n\n(task_id: task_"), "{}", first.output);
    let again = crate::tool::task::Task.run(&ctx, front).await.unwrap();
    assert_eq!((again.output.as_str(), again.metadata["mode"].as_str()), (first.output.as_str(), Some("foreground")), "a foreground replay is its result, not a background receipt");
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
async fn a_worker_whose_reply_the_provider_refused_says_so_not_that_it_was_cut_off() {
    let h = harness().await;
    let refused = vec![Chunk::TextStart, Chunk::TextDelta("I can".into()), Chunk::BlockStop, Chunk::Stop(crate::llm::StopReason::Refused)];
    h.provider
        .push_for("PARENT", launches(&[json!({ "description": "Write it up", "prompt": "CHILD write it" })]))
        .push_for("PARENT", text("it was refused"))
        .push_for("CHILD write", refused);
    h.engine.submit(&h.session.id, prompt("PARENT report")).await.unwrap();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { status, output, metadata, .. } = &transcript[1].parts[0].part else { panic!() };
    assert_eq!(*status, crate::session::types::ToolStatus::Error);
    assert_eq!(metadata.as_ref().unwrap()["outcome"], "refused");
    let output = output.as_deref().unwrap();
    assert!(output.contains("safety filter") && !output.contains("output limit"), "{output}");
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
    assert!(output.as_deref().unwrap().starts_with("waited result\n\n(task_id:"), "foreground returns the result as the call's own");
}
