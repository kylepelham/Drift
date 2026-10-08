use super::*;

#[tokio::test]
async fn a_smaller_limit_takes_slots_back_only_as_running_workers_finish() {
    let workers = Workers::new(2);
    let first = workers.slots.acquire().await.unwrap();
    let second = workers.slots.acquire().await.unwrap();

    // Both slots are in use, so shrinking to one retires the next slot handed back.
    workers.resize(1);
    workers.release(first);
    assert_eq!(workers.slots.available_permits(), 0, "the first slot back is retired");
    workers.release(second);
    assert_eq!(workers.slots.available_permits(), 1, "the second is free again");

    // Growing past a pending shrink cancels what is still owed before adding slots.
    let held = workers.slots.acquire().await.unwrap();
    workers.resize(0);
    workers.resize(3);
    workers.release(held);
    assert_eq!(workers.slots.available_permits(), 3);
}

#[tokio::test]
async fn raising_the_background_limit_starts_a_queued_worker_at_once() {
    let h = harness().await;
    h.engine.set_background_limit(1).unwrap();
    let launched: Vec<_> = (0..2)
        .map(|i| background(&format!("Job {i}"), &format!("CHILD {i} wait")))
        .collect();
    h.provider
        .push_for("PARENT", launches(&launched))
        .push_for("PARENT", text("carrying on"));
    for i in 0..2 {
        h.provider.push_stall_for(&format!("CHILD {i} wait"));
    }

    // One slot: one runs and one waits.
    h.engine.submit(&h.session.id, prompt("PARENT two")).await.unwrap();
    until("one runs and one waits", || {
        let all = tasks(&h);
        let running = all.iter().filter(|t| t.state == TaskState::Running).count();
        let queued = all.iter().filter(|t| t.state == TaskState::Queued).count();
        running == 1 && queued == 1
    })
    .await;

    // Two slots: the waiting one starts without anything finishing.
    h.engine.set_background_limit(2).unwrap();
    until("both run", || {
        tasks(&h).iter().filter(|t| t.state == TaskState::Running).count() == 2
    })
    .await;

    assert!(matches!(h.engine.set_background_limit(0), Err(LimitError::OutOfRange)));
    assert!(matches!(
        h.engine.set_background_limit(MAX_BACKGROUND_LIMIT + 1),
        Err(LimitError::OutOfRange)
    ));
    assert_eq!(h.engine.background_limit(), 2, "a refused limit leaves the saved one");
    h.engine.abort(&h.session.id);
}

#[tokio::test]
async fn background_slots_are_bounded_and_session_stop_ends_them_even_when_idle() {
    let h = harness().await;
    let launched: Vec<_> = (0..DEFAULT_BACKGROUND_LIMIT + 1)
        .map(|index| background(&format!("Job {index}"), &format!("CHILD {index} wait")))
        .collect();
    h.provider
        .push_for("PARENT", launches(&launched))
        .push_for("PARENT", text("carrying on"));
    for index in 0..=DEFAULT_BACKGROUND_LIMIT {
        h.provider.push_stall_for(&format!("CHILD {index} wait"));
    }
    h.engine.submit(&h.session.id, prompt("PARENT many")).await.unwrap();
    until("the parent's turn ends", || !h.engine.turns.is_running(&h.session.id)).await;
    until("the slots fill", || {
        tasks(&h).iter().filter(|task| task.state == TaskState::Running).count() == DEFAULT_BACKGROUND_LIMIT
    })
    .await;
    assert_eq!(
        tasks(&h).iter().filter(|task| task.state == TaskState::Queued).count(),
        1,
        "one waits for a slot"
    );

    let requests = h.provider.requests.lock().unwrap().len();
    assert!(
        h.engine.abort(&h.session.id),
        "Stop has something to stop with the parent idle"
    );
    until("all stopped and held", || {
        tasks(&h).len() == DEFAULT_BACKGROUND_LIMIT + 1
            && tasks(&h)
                .iter()
                .all(|task| task.state == TaskState::Stopped && task.held && !task.delivered)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        h.provider.requests.lock().unwrap().len(),
        requests,
        "no worker started after the stop and the parent was not woken"
    );
    assert!(delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()).is_empty());
}

#[tokio::test]
async fn stopping_one_worker_leaves_the_others_running() {
    let h = harness().await;
    h.provider
        .push_for(
            "PARENT",
            launches(&[background("First", "CHILD first"), background("Second", "CHILD second")]),
        )
        .push_for("PARENT", text("launched"))
        .push_stall_for("CHILD first")
        .push_stall_for("CHILD second");
    h.engine.submit(&h.session.id, prompt("PARENT pair")).await.unwrap();
    until("both run", || {
        tasks(&h).len() == 2 && tasks(&h).iter().all(|task| task.state == TaskState::Running)
    })
    .await;
    let first = tasks(&h).into_iter().find(|task| task.description == "First").unwrap();
    h.engine.stop_task(&first.id).unwrap();
    until("the first stops", || {
        h.engine.store.task(&first.id).unwrap().unwrap().state == TaskState::Stopped
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let second = tasks(&h).into_iter().find(|task| task.description == "Second").unwrap();
    assert_eq!(second.state, TaskState::Running, "only the one asked for stops");
    assert!(
        !h.engine.turns.is_running(&h.session.id),
        "a stopped worker does not wake an idle parent"
    );
    h.engine.abort(&h.session.id);
}

#[tokio::test]
async fn a_worker_stopped_between_its_start_and_its_turn_never_runs() {
    let h = harness().await;
    let launched: Vec<_> = (0..=DEFAULT_BACKGROUND_LIMIT)
        .map(|index| background(&format!("Job {index}"), &format!("CHILD {index} wait")))
        .collect();
    h.provider
        .push_for("PARENT", launches(&launched))
        .push_for("PARENT", text("carrying on"));
    for index in 0..=DEFAULT_BACKGROUND_LIMIT {
        h.provider.push_stall_for(&format!("CHILD {index} wait"));
    }
    h.engine.submit(&h.session.id, prompt("PARENT many")).await.unwrap();
    until("one waits for a slot", || {
        tasks(&h).iter().filter(|task| task.state == TaskState::Running).count() == DEFAULT_BACKGROUND_LIMIT
            && tasks(&h).iter().any(|task| task.state == TaskState::Queued)
    })
    .await;
    let queued = tasks(&h)
        .into_iter()
        .find(|task| task.state == TaskState::Queued)
        .unwrap();

    // Holding the transcript stops the worker between its running marker and turn admission.
    let holder = CancellationToken::new();
    assert!(h.engine.turns.claim(&queued.session_id, &holder));
    let freed = tasks(&h)
        .into_iter()
        .find(|task| task.state == TaskState::Running)
        .unwrap();
    h.engine.stop_task(&freed.id).unwrap();
    until("it starts", || {
        h.engine.store.task(&queued.id).unwrap().unwrap().state == TaskState::Running
    })
    .await;
    h.engine.stop_task(&queued.id).unwrap();
    until("it stops", || {
        h.engine.store.task(&queued.id).unwrap().unwrap().state == TaskState::Stopped
    })
    .await;
    h.engine.turns.release(&queued.session_id);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let prompt_text = format!("{} wait", queued.description.replace("Job", "CHILD"));
    let asked = h
        .provider
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|request| opens_with(request, &prompt_text));
    assert!(!asked, "its turn never reached the model");
    assert!(
        h.engine.store.transcript(&queued.session_id).unwrap().is_empty(),
        "nothing was admitted"
    );
    h.engine.abort(&h.session.id);
}

#[tokio::test]
async fn a_queued_worker_runs_as_it_was_admitted_not_as_settings_changed_since() {
    let h = harness().await;
    let set_prompt = |text: &str| {
        h.engine.set_agent_overrides(std::collections::HashMap::from([(
            "general".to_string(),
            crate::config::AgentOverride {
                prompt: Some(text.into()),
                ..Default::default()
            },
        )]));
    };
    set_prompt("PROMPT-ALPHA");
    let mut launched: Vec<_> = (0..DEFAULT_BACKGROUND_LIMIT)
        .map(|index| background(&format!("Job {index}"), &format!("CHILD {index} wait")))
        .collect();
    launched.push(background("Queued", "CHILD queued work"));
    h.provider
        .push_for("PARENT", launches(&launched))
        .push_for("PARENT", text("carrying on"))
        .push_for("CHILD queued", text("queued result"));
    for index in 0..DEFAULT_BACKGROUND_LIMIT {
        h.provider.push_stall_for(&format!("CHILD {index} wait"));
    }
    h.engine.submit(&h.session.id, prompt("PARENT many")).await.unwrap();
    until("the last one queues", || {
        tasks(&h)
            .iter()
            .any(|task| task.description == "Queued" && task.state == TaskState::Queued)
            && tasks(&h).iter().filter(|task| task.state == TaskState::Running).count() == DEFAULT_BACKGROUND_LIMIT
    })
    .await;

    set_prompt("PROMPT-BETA");
    let freed = tasks(&h)
        .into_iter()
        .find(|task| task.state == TaskState::Running)
        .unwrap();
    h.engine.stop_task(&freed.id).unwrap();
    until("the queued one replies", || {
        tasks(&h)
            .iter()
            .any(|task| task.description == "Queued" && task.state == TaskState::Replied)
    })
    .await;
    let requests = h.provider.requests.lock().unwrap();
    let ran = requests
        .iter()
        .find(|request| opens_with(request, "CHILD queued work"))
        .unwrap();
    assert!(
        ran.system.contains("PROMPT-ALPHA") && !ran.system.contains("PROMPT-BETA"),
        "the prompt it was admitted with"
    );
    drop(requests);
    h.engine.abort(&h.session.id);
}
