use super::*;

/// The engine as it opens again on the same data, with the same scripted provider.
fn reopen(h: &Harness) -> Arc<crate::Engine> {
    let restarted = crate::Engine::open_with(
        &h._dir.join("data"),
        crate::Options {
            file_credentials: true,
            ..Default::default()
        },
    )
    .unwrap();
    *restarted.turns.provider_override.lock().unwrap() = h.engine.turns.provider_override.lock().unwrap().clone();

    restarted
}

/// A background worker that was running when Drift stopped, its own conversation able to run.
fn running_worker(h: &Harness, call_id: &str) -> TaskRecord {
    let task = recorded(h, call_id, Mode::Background);
    h.engine.store.start_task(&task.id).unwrap();
    let model = crate::session::turn::tests::model();
    h.engine
        .store
        .update_session(&task.session_id, None, Some(&model), None)
        .unwrap();

    task
}

async fn until_ended(engine: &crate::Engine, task_id: &str) -> TaskRecord {
    for _ in 0..300 {
        let task = engine.store.task(task_id).unwrap().unwrap();
        if task.state.is_terminal() && task.delivered {
            return task;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the worker never finished and reported");
}

#[tokio::test]
async fn a_background_worker_cut_off_by_a_restart_carries_on_and_reports_to_its_parent() {
    let h = harness().await;
    with_model(&h);
    let task = running_worker(&h, "cut off");
    h.provider.push(text("finished after the restart")).push(text("noted"));

    let restarted = reopen(&h);
    assert_eq!(
        restarted.store.task(&task.id).unwrap().unwrap().state,
        TaskState::Queued,
        "queued again at open, not interrupted"
    );
    restarted.restart_workers().await;
    let ended = until_ended(&restarted, &task.id).await;

    assert_eq!(
        (ended.state, ended.result.as_deref()),
        (TaskState::Replied, Some("finished after the restart"))
    );
    let requests = h.provider.requests.lock().unwrap().clone();
    assert!(
        requests.iter().any(|request| texts_of(request)
            .iter()
            .any(|text| text.contains("Drift restarted while you were working"))),
        "the worker is told it was restarted"
    );
    for _ in 0..200 {
        if !delivered_results(&restarted.store.transcript(&h.session.id).unwrap()).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        delivered_results(&restarted.store.transcript(&h.session.id).unwrap()),
        [("cut off".to_string(), "finished after the restart".to_string())],
        "its result wakes the parent as any other would"
    );
}

#[tokio::test]
async fn a_worker_is_restarted_at_most_twice_and_not_at_all_when_turned_off() {
    let h = harness().await;
    let task = running_worker(&h, "flaky");
    for restart in 1..=2 {
        assert_eq!(
            h.engine.store.interrupt_unfinished_tasks(true).unwrap(),
            std::slice::from_ref(&task.id),
            "restart {restart}"
        );
        h.engine.store.start_task(&task.id).unwrap();
    }
    assert!(h.engine.store.interrupt_unfinished_tasks(true).unwrap().is_empty());
    let given_up = h.engine.store.task(&task.id).unwrap().unwrap();
    assert_eq!((given_up.state, given_up.delivered), (TaskState::Interrupted, true));

    let other = running_worker(&h, "off");
    assert!(h.engine.store.interrupt_unfinished_tasks(false).unwrap().is_empty());
    assert_eq!(
        h.engine.store.task(&other.id).unwrap().unwrap().state,
        TaskState::Interrupted
    );
}

#[tokio::test]
async fn a_worker_whose_owner_was_stopped_is_not_restarted_and_wakes_no_one() {
    let h = harness().await;
    with_model(&h);
    let task = running_worker(&h, "stopped since");
    h.engine.abort(&h.session.id);

    let restarted = reopen(&h);
    restarted.restart_workers().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let ended = restarted.store.task(&task.id).unwrap().unwrap();
    assert_eq!((ended.state, ended.delivered), (TaskState::Interrupted, true));
    assert!(h.provider.requests.lock().unwrap().is_empty(), "nothing ran");
}

#[tokio::test]
async fn a_foreground_worker_cut_off_names_its_task_so_the_model_can_continue_it() {
    let h = harness().await;
    let task = recorded(&h, "fore", Mode::Foreground);
    h.engine.store.start_task(&task.id).unwrap();

    assert!(
        h.engine.store.interrupt_unfinished_tasks(true).unwrap().is_empty(),
        "the resumed parent decides"
    );
    let ended = h.engine.store.task(&task.id).unwrap().unwrap();
    assert_eq!(ended.state, TaskState::Interrupted);
    assert!(ended.result.unwrap().contains(&format!("task_id {}", task.id)));
}

fn texts_of(request: &crate::llm::Request) -> Vec<String> {
    request
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| match block {
            crate::llm::Block::Text(text) | crate::llm::Block::Reminder(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}
