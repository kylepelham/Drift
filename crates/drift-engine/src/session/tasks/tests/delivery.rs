use super::*;

#[tokio::test]
async fn task_output_answers_for_this_conversations_tasks_and_waits_only_as_asked() {
    let h = harness().await;
    let running = recorded(&h, "a", Mode::Background);
    h.engine.store.start_task(&running.id).unwrap();

    let ctx = context(&h, &h.session.id, "call");
    let started = std::time::Instant::now();
    let out = crate::tool::task::TaskOutput
        .run(&ctx, json!({ "task_id": running.id, "wait_seconds": 1 }))
        .await
        .unwrap();
    assert!(
        out.output.ends_with(": running") && started.elapsed() >= Duration::from_millis(900),
        "{}",
        out.output
    );
    assert!(out.metadata.delivers.is_none());

    h.engine.end_task(&running.id, TaskState::Replied, "the answer");

    let out = crate::tool::task::TaskOutput
        .run(&ctx, json!({ "task_id": running.id }))
        .await
        .unwrap();
    assert!(out.output.ends_with("replied\n\nthe answer"), "{}", out.output);
    assert_eq!(
        out.metadata.delivers.as_deref(),
        Some(running.id.as_str()),
        "this call hands it over"
    );
    assert!(
        !h.engine.store.task(&running.id).unwrap().unwrap().delivered,
        "not until the call's result is saved"
    );

    let other = h
        .engine
        .store
        .create_session(NewSession {
            workspace_id: &h.session.workspace_id,
            parent_id: None,
            visibility: crate::session::types::Visibility::Sibling,
            title: "Other",
            agent: "build",
            model: None,
        })
        .unwrap();
    let foreign = crate::tool::task::TaskOutput
        .run(&context(&h, &other.id, "call"), json!({ "task_id": running.id }))
        .await
        .unwrap_err();
    assert!(foreign.0.contains("not launched from this conversation"));
}

#[tokio::test]
async fn a_result_being_read_is_not_delivered_again_and_one_being_delivered_is_not_read_again() {
    let h = harness().await;
    with_model(&h);
    let task = recorded(&h, "launch", Mode::Background);
    h.engine.store.start_task(&task.id).unwrap();
    let ctx = context(&h, &h.session.id, "reading");
    let reading = tokio::spawn({
        let input = json!({ "task_id": task.id, "wait_seconds": 5 });
        async move { crate::tool::task::TaskOutput.run(&ctx, input).await.unwrap() }
    });
    until("the read claims it", || {
        h.engine
            .workers
            .holds(&task.id, &Claimant::call(&h.session.id, "reading"))
    })
    .await;
    h.engine.end_task(&task.id, TaskState::Replied, "the answer");
    h.engine.deliver(&task.id).await;

    let out = reading.await.unwrap();
    assert_eq!(out.metadata.delivers.as_deref(), Some(task.id.as_str()));
    assert!(
        delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()).is_empty(),
        "no second copy as a message"
    );

    let mut row = call_row(&h, "reading");
    h.engine.settle_delivering(&mut row, settlement(&out), Some(&task.id));
    h.engine.release_claims(&Claimant::call(&h.session.id, "reading"));

    assert!(h.engine.store.task(&task.id).unwrap().unwrap().delivered);
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(
        delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()).is_empty()
            && !h.engine.turns.is_running(&h.session.id)
    );

    let second = recorded(&h, "second", Mode::Background);
    h.engine.end_task(&second.id, TaskState::Replied, "second answer");

    assert!(h.engine.workers.claim(&second.id, Claimant::Automatic));
    let out = crate::tool::task::TaskOutput
        .run(&context(&h, &h.session.id, "late"), json!({ "task_id": second.id }))
        .await
        .unwrap();
    assert!(
        out.output.ends_with("arriving in this conversation as a message.") && !out.output.contains("second answer"),
        "{}",
        out.output
    );
    assert!(out.metadata.delivers.is_none());
}

#[tokio::test]
async fn a_result_whose_call_was_not_saved_is_still_owed_and_arrives_as_a_message() {
    let h = harness().await;
    with_model(&h);
    h.provider.push(text("noted the answer"));
    let task = recorded(&h, "launch", Mode::Background);
    h.engine.end_task(&task.id, TaskState::Replied, "the answer");
    let out = crate::tool::task::TaskOutput
        .run(&context(&h, &h.session.id, "reading"), json!({ "task_id": task.id }))
        .await
        .unwrap();
    assert_eq!(out.metadata.delivers.as_deref(), Some(task.id.as_str()));
    let mut row = call_row(&h, "reading");
    let trigger = "CREATE TEMP TRIGGER no_room BEFORE UPDATE ON part WHEN NEW.json LIKE '%the answer%' \
                   BEGIN SELECT RAISE(ABORT, 'disk is full'); END;";
    h.engine.store.lock().execute_batch(trigger).unwrap();
    h.engine.settle_delivering(&mut row, settlement(&out), Some(&task.id));
    h.engine.store.lock().execute_batch("DROP TRIGGER no_room;").unwrap();

    let failed = tool(&row);
    assert_eq!(failed.status, ToolStatus::Error, "{:?}", failed.output);
    assert!(
        !h.engine.store.task(&task.id).unwrap().unwrap().delivered,
        "nothing saved, nothing handed over"
    );

    h.engine.release_claims(&Claimant::call(&h.session.id, "reading"));
    until("delivered as a message instead", || {
        h.engine.store.task(&task.id).unwrap().unwrap().delivered
    })
    .await;
    until_idle(&h).await;

    assert_eq!(
        delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()),
        [("launch".to_string(), "the answer".to_string())]
    );
}

#[tokio::test]
async fn a_foreground_result_stays_its_launching_calls_even_when_saving_it_fails() {
    let h = harness().await;
    with_model(&h);
    h.provider.push_for("CHILD front", text("front answer"));

    let out = crate::tool::task::Task
        .run(
            &context(&h, &h.session.id, "fg_call"),
            json!({ "description": "Front", "prompt": "CHILD front" }),
        )
        .await
        .unwrap();
    let task_id = out.metadata.task_id.clone().unwrap();
    assert_eq!(out.metadata.delivers.as_deref(), Some(task_id.as_str()));
    let reader = context(&h, &h.session.id, "reader");
    let refused = crate::tool::task::TaskOutput
        .run(&reader, json!({ "task_id": task_id }))
        .await
        .unwrap_err();
    assert!(refused.0.contains("foreground"), "{}", refused.0);
    assert!(
        h.engine
            .workers
            .holds(&task_id, &Claimant::call(&h.session.id, "fg_call"))
            && !h
                .engine
                .workers
                .holds(&task_id, &Claimant::call(&h.session.id, "reader"))
    );

    let mut row = call_row(&h, "fg_call");
    let trigger = "CREATE TEMP TRIGGER no_room BEFORE UPDATE ON part WHEN NEW.json LIKE '%front answer%' \
                   BEGIN SELECT RAISE(ABORT, 'disk is full'); END;";
    h.engine.store.lock().execute_batch(trigger).unwrap();
    h.engine.settle_delivering(&mut row, settlement(&out), Some(&task_id));
    h.engine.store.lock().execute_batch("DROP TRIGGER no_room;").unwrap();
    h.engine.release_claims(&Claimant::call(&h.session.id, "fg_call"));
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(
        !h.engine.store.task(&task_id).unwrap().unwrap().delivered,
        "nothing saved, nothing handed over"
    );
    assert!(
        delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()).is_empty(),
        "a foreground result never becomes a message"
    );
    assert!(
        crate::tool::task::TaskOutput
            .run(&reader, json!({ "task_id": task_id }))
            .await
            .is_err()
    );

    h.engine.recover_tasks().await;

    let saved = h
        .engine
        .store
        .transcript(&h.session.id)
        .unwrap()
        .into_iter()
        .flat_map(|message| message.parts)
        .find(|part| part.id == row.id)
        .unwrap();
    let call = tool(&saved);
    assert_eq!((call.status, call.output), (ToolStatus::Done, Some("front answer")));
    assert!(h.engine.store.task(&task_id).unwrap().unwrap().delivered);
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
    h.engine
        .spawn_job(&h.session.id, async move { waiting.notified().await });
    h.engine.deliver(&task.id).await;

    let owed = h.engine.store.task(&task.id).unwrap().unwrap();
    assert!(
        !owed.delivered
            && owed
                .delivery_error
                .as_deref()
                .is_some_and(|error| error.contains("busy")),
        "{owed:?}"
    );
    assert!(h.provider.requests.lock().unwrap().is_empty());

    gate.notify_one();
    until("delivered once the job ends", || {
        h.engine.store.task(&task.id).unwrap().unwrap().delivered
    })
    .await;
    until_idle(&h).await;

    assert_eq!(h.engine.store.task(&task.id).unwrap().unwrap().delivery_error, None);
    assert_eq!(
        delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()),
        [("launch".to_string(), "busy answer".to_string())]
    );
}

#[tokio::test]
async fn a_retry_asked_for_while_an_attempt_holds_the_result_is_made_by_that_attempt() {
    let h = harness().await;
    with_model(&h);
    h.provider.push(text("noted"));
    let task = recorded(&h, "launch", Mode::Background);
    h.engine.end_task(&task.id, TaskState::Replied, "late answer");

    let busy = CancellationToken::new();
    assert!(h.engine.turns.claim(&h.session.id, &busy));

    let attempt = tokio::spawn({
        let (engine, id) = (h.engine.clone(), task.id.clone());
        async move { engine.deliver(&id).await }
    });
    until("the attempt holds it", || {
        h.engine.workers.holds(&task.id, &Claimant::Automatic)
    })
    .await;
    h.engine.retry_deliveries(Some(&h.session.id));
    until("the first attempt fails", || {
        h.engine.store.task(&task.id).unwrap().unwrap().delivery_error.is_some()
    })
    .await;
    h.engine.turns.release(&h.session.id);
    tokio::time::timeout(Duration::from_secs(5), attempt)
        .await
        .expect("the attempt ends")
        .unwrap();

    assert!(
        h.engine.store.task(&task.id).unwrap().unwrap().delivered,
        "the notification was honoured, not dropped"
    );
    until_idle(&h).await;

    assert_eq!(
        delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()).len(),
        1
    );
}

#[tokio::test]
async fn a_result_blocked_by_a_missing_model_stays_owed_with_its_reason_until_one_is_chosen() {
    let h = harness().await;
    let task = recorded(&h, "launch", Mode::Background);
    h.engine.end_task(&task.id, TaskState::Replied, "answer");
    h.engine.deliver(&task.id).await;

    let owed = h.engine.store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        (owed.delivered, owed.delivery_error.as_deref()),
        (false, Some("no model selected"))
    );
    assert!(
        serde_json::to_value(&owed).unwrap()["deliveryError"] == "no model selected",
        "exposed to the UI"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(!h.engine.workers.holds(&task.id, &Claimant::Automatic) && h.provider.requests.lock().unwrap().is_empty());

    h.provider.push(text("noted"));
    with_model(&h);
    h.engine.retry_deliveries(Some(&h.session.id));
    until("delivered after the repair", || {
        h.engine.store.task(&task.id).unwrap().unwrap().delivered
    })
    .await;
    until_idle(&h).await;

    assert_eq!(h.engine.store.task(&task.id).unwrap().unwrap().delivery_error, None);
}
