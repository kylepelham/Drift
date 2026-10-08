use super::*;

#[tokio::test]
async fn a_stop_while_a_result_waits_to_be_admitted_keeps_it_from_starting_a_turn() {
    let h = harness().await;
    with_model(&h);
    let task = recorded(&h, "launch", Mode::Background);
    h.engine.end_task(&task.id, TaskState::Replied, "late answer");
    let job = CancellationToken::new();
    assert!(h.engine.turns.claim(&h.session.id, &job));
    let delivering = tokio::spawn({
        let (engine, id) = (h.engine.clone(), task.id.clone());
        async move { engine.deliver(&id).await }
    });
    until("the delivery is waiting", || {
        h.engine.workers.holds(&task.id, &Claimant::Automatic)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(h.engine.abort(&h.session.id));
    tokio::time::timeout(Duration::from_secs(5), delivering)
        .await
        .expect("the wait ends with the Stop")
        .unwrap();
    h.engine.turns.release(&h.session.id);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let held = h.engine.store.task(&task.id).unwrap().unwrap();
    assert!(
        held.held && !held.delivered,
        "held for the next prompt, not marked handed over"
    );
    assert!(
        delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()).is_empty(),
        "the stopped parent was not woken"
    );
    assert!(!h.engine.turns.is_running(&h.session.id) && h.provider.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_result_launched_before_a_stop_never_wakes_the_parent_even_after_a_restart() {
    let h = harness().await;
    with_model(&h);
    let before = recorded(&h, "before", Mode::Background);
    h.engine
        .end_task(&before.id, TaskState::Replied, "from before the stop");
    h.engine.abort(&h.session.id);
    let restarted = crate::Engine::open_with(
        &h._dir.join("data"),
        crate::Options {
            file_credentials: true,
            ..Default::default()
        },
    )
    .unwrap();
    *restarted.turns.provider_override.lock().unwrap() = h.engine.turns.provider_override.lock().unwrap().clone();
    restarted.recover_tasks().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let kept = restarted.store.task(&before.id).unwrap().unwrap();
    assert!(
        kept.held && !kept.delivered,
        "held across the restart, not marked handed over"
    );
    assert!(
        h.provider.requests.lock().unwrap().is_empty() && !restarted.turns.is_running(&h.session.id),
        "it woke nothing"
    );
}

#[tokio::test]
async fn a_held_result_rides_along_with_a_later_permitted_delivery_once() {
    let h = harness().await;
    with_model(&h);
    let held = recorded(&h, "before", Mode::Background);
    h.engine.end_task(&held.id, TaskState::Replied, "from before the stop");
    h.engine.abort(&h.session.id);
    h.engine.deliver(&held.id).await;
    assert!(
        h.engine.store.task(&held.id).unwrap().unwrap().held && h.provider.requests.lock().unwrap().is_empty(),
        "alone it wakes nothing"
    );

    h.provider.push(text("noted both"));
    let later = recorded(&h, "after", Mode::Background);
    h.engine.end_task(&later.id, TaskState::Replied, "from after the stop");
    h.engine.deliver(&later.id).await;
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let carried: Vec<_> = transcript
        .iter()
        .filter(|message| !delivered_results(std::slice::from_ref(message)).is_empty())
        .collect();
    assert_eq!(carried.len(), 1, "one prompt, one turn");
    assert_eq!(
        delivered_results(&transcript),
        [
            ("before".to_string(), "from before the stop".to_string()),
            ("after".to_string(), "from after the stop".to_string())
        ]
    );
    assert!(
        [&held, &later]
            .iter()
            .all(|task| h.engine.store.task(&task.id).unwrap().unwrap().delivered)
    );
    assert_eq!(h.provider.requests.lock().unwrap().len(), 1);

    h.provider.push(text("ok"));
    h.engine.submit(&h.session.id, prompt("next")).await.unwrap();
    until_idle(&h).await;
    assert_eq!(
        delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()).len(),
        2,
        "never carried again"
    );
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
    let handover = crate::store::Handover {
        delivery: Some(&landed.id),
        held: vec![(held.id.clone(), result_part(&held))],
    };
    let admitted = h
        .engine
        .store
        .admit_delivering(
            &h.session.id,
            crate::store::Admission {
                pick: crate::store::Pick::model(&crate::session::turn::tests::model()),
                parts: vec![],
                submission: None,
                handover,
            },
        )
        .unwrap();

    assert!(matches!(admitted, crate::store::Admit::Delivered), "nothing written");
    let still = h.engine.store.task(&held.id).unwrap().unwrap();
    assert!(
        still.held && !still.delivered,
        "its acknowledgment went with the rest of the write"
    );
    assert!(h.engine.store.transcript(&h.session.id).unwrap().is_empty());
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
    assert!(
        h.provider.requests.lock().unwrap().is_empty(),
        "never wakes the stopped parent"
    );
    h.engine.deliver(&task.id).await;
    h.engine.recover_tasks().await;
    assert!(!h.engine.turns.is_running(&h.session.id) && !h.engine.store.task(&task.id).unwrap().unwrap().delivered);

    h.provider.push(text("thanks")).push(text("again"));
    h.engine.submit(&h.session.id, prompt("what next")).await.unwrap();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert_eq!(
        delivered_results(&transcript),
        [("launch".to_string(), "held answer".to_string())]
    );
    let user = &transcript[0];
    assert!(
        matches!(user.parts[0].part, Part::TaskResult { .. })
            && matches!(&user.parts[1].part, Part::Text { text } if text == "what next"),
        "carried in the user's own prompt, ahead of it"
    );
    assert!(h.engine.store.task(&task.id).unwrap().unwrap().delivered);

    h.engine.submit(&h.session.id, prompt("and then")).await.unwrap();
    until_idle(&h).await;
    assert_eq!(
        delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()).len(),
        1,
        "once only"
    );
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
    assert!(
        delivered_results(&transcript).is_empty(),
        "not a background notification"
    );
    let saved = transcript
        .iter()
        .flat_map(|message| &message.parts)
        .find(|part| part.id == row.id)
        .unwrap();
    let call = tool(saved);
    assert_eq!(
        (call.status, call.output),
        (ToolStatus::Done, Some("foreground answer"))
    );
    assert!(h.engine.store.task(&task.id).unwrap().unwrap().delivered);
    assert!(!h.engine.turns.is_running(&h.session.id));
}

#[tokio::test]
async fn after_a_restart_unfinished_workers_are_interrupted_and_finished_results_arrive_once() {
    let h = harness().await;
    let was_running = recorded(&h, "was running", Mode::Background);
    h.engine.store.start_task(&was_running.id).unwrap();
    let finished = recorded(&h, "finished", Mode::Background);
    h.engine
        .store
        .finish_task(&finished.id, TaskState::Replied, "done before the restart")
        .unwrap();
    with_model(&h);
    h.provider.push(text("noted"));
    h.engine.store.interrupt_unfinished_tasks().unwrap();
    h.engine.recover_tasks().await;
    until_idle(&h).await;
    h.engine.recover_tasks().await;
    until_idle(&h).await;

    let interrupted = h.engine.store.task(&was_running.id).unwrap().unwrap();
    assert_eq!(
        (interrupted.state, interrupted.delivered),
        (TaskState::Interrupted, true),
        "never rerun"
    );
    assert_eq!(
        delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()),
        [("finished".to_string(), "done before the restart".to_string())],
        "once, however often recovery runs"
    );
    assert_eq!(
        h.provider.requests.lock().unwrap().len(),
        1,
        "only the delivered result started a turn"
    );
}
