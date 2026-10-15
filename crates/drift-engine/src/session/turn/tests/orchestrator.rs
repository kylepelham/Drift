use super::*;

fn status(state: &str) -> Vec<Chunk> {
    text(&format!(
        "round\n<orchestrator_status>{{\"state\":\"{state}\"}}</orchestrator_status>"
    ))
}

fn nudges(h: &Harness) -> Vec<String> {
    transcript(h)
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|row| match &row.part {
            Part::Nudge { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn the_engine_drives_an_orchestrator_until_it_is_done_and_reminds_it_of_the_protocol() {
    let h = harness().await;
    h.provider
        .push(status("working"))
        .push(text("forgot the block"))
        .push(status("done"));
    h.engine
        .submit(
            &h.session.id,
            Prompt {
                agent: Some("orchestrator".into()),
                ..prompt("ship it")
            },
        )
        .await
        .await_ok();
    until_idle(&h).await;

    let sent = nudges(&h);
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(
        sent[0].starts_with("Proceed toward the goal")
            && sent[1].contains("did not end with a valid <orchestrator_status> block")
    );
    assert_eq!(h.provider.requests.lock().unwrap().len(), 3, "done ends the turn");
    let last = h.provider.requests.lock().unwrap()[2].messages.last().cloned().unwrap();
    assert!(
        matches!(&last.blocks[..], [llm::Block::Text(text)] if text.contains("<orchestrator_status>")),
        "the model reads the nudge as a prompt"
    );
}

#[tokio::test]
async fn an_orchestrator_stops_after_the_round_limit_and_a_new_prompt_restarts_the_count() {
    let h = harness().await;
    for _ in 0..=crate::session::drive::MAX_ROUNDS {
        h.provider.push(status("working"));
    }
    h.engine
        .submit(
            &h.session.id,
            Prompt {
                agent: Some("orchestrator".into()),
                ..prompt("big goal")
            },
        )
        .await
        .await_ok();
    until_idle(&h).await;

    assert_eq!(nudges(&h).len(), crate::session::drive::MAX_ROUNDS);
    assert_eq!(
        h.engine.store.nudges_since_prompt(&h.session.id).unwrap(),
        crate::session::drive::MAX_ROUNDS
    );

    h.provider.push(status("working")).push(status("blocked"));
    h.engine.submit(&h.session.id, prompt("keep going")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(
        nudges(&h).len(),
        crate::session::drive::MAX_ROUNDS + 1,
        "the user's own prompt starts a fresh budget; blocked ends it"
    );
}

#[tokio::test]
async fn other_agents_are_never_driven() {
    let h = harness().await;
    h.provider.push(status("working"));
    h.engine.submit(&h.session.id, prompt("hello")).await.await_ok();
    until_idle(&h).await;

    assert!(nudges(&h).is_empty());
}

#[tokio::test]
async fn an_orchestrator_waiting_on_background_work_ends_its_turn_and_the_result_wakes_it() {
    let h = harness().await;
    let (_, generation) = h.engine.worker_scope(&h.session.id);
    let task = h
        .engine
        .store
        .launch_task(
            crate::store::NewTask {
                generation,
                ..crate::store::tasks::tests::new_task(
                    &h.session.id,
                    "call_bg",
                    crate::session::tasks::Mode::Background,
                )
            },
            crate::store::tasks::tests::child(&h.session),
        )
        .unwrap()
        .task;
    h.engine.store.start_task(&task.id).unwrap();
    h.provider.push(status("working"));

    h.engine
        .submit(
            &h.session.id,
            Prompt {
                agent: Some("orchestrator".into()),
                ..prompt("ship it")
            },
        )
        .await
        .await_ok();
    until_idle(&h).await;
    assert!(nudges(&h).is_empty(), "nothing to do but wait, so no nudge");

    // The worker's result starts the next turn by itself.
    h.provider.push(status("done"));
    h.engine
        .end_task(&task.id, crate::session::tasks::TaskState::Replied, "finished");
    h.engine.deliver(&task.id).await;
    until_idle(&h).await;

    let last = transcript(&h);
    assert!(
        last.iter()
            .flat_map(|message| &message.parts)
            .any(|row| matches!(row.part, Part::TaskResult { .. })),
        "the result arrived as a prompt"
    );
    assert!(nudges(&h).is_empty());
    assert_eq!(
        h.provider.requests.lock().unwrap().len(),
        2,
        "one request to dispatch, one when the result came"
    );
}
