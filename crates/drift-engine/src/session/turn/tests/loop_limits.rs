use super::*;

#[tokio::test]
async fn a_turn_pauses_at_its_step_limit_and_a_message_carries_on() {
    let h = harness().await;
    limits(&h, r#"{ "steps": 2 }"#);
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    std::fs::write(h._dir.join("ws/b.txt"), "b\n").unwrap();
    h.provider
        .push(read_a())
        .push(text("WRAPPED: read a, b still to do"))
        .push(tool_call("read", r#"{"path": "b.txt"}"#))
        .push(text("finished"));
    h.engine.submit(&h.session.id, prompt("work")).await.await_ok();
    until_idle(&h).await;

    let mut messages = transcript(&h);
    let last = messages.pop().unwrap();
    assert_eq!(last.info.status, MessageStatus::Paused);
    assert!(
        last.info.error.as_deref().unwrap().starts_with("Paused after 2 steps"),
        "{:?}",
        last.info.error
    );
    assert!(
        format!("{:?}", messages.last().unwrap().parts).contains("WRAPPED"),
        "the last allowed step writes up what was done"
    );
    {
        let requests = h.provider.requests.lock().unwrap();
        assert!(
            !requests[0].no_tool_calls && requests[1].no_tool_calls,
            "the last allowed step has tools off"
        );
        assert!(format!("{:?}", requests[1].messages.last()).contains("last step this turn allows"));
    }
    assert_eq!(h.provider.responses_left(), 2, "no request after the limit");

    h.engine.submit(&h.session.id, prompt("carry on")).await.await_ok();
    until_idle(&h).await;
    assert_eq!(h.provider.responses_left(), 0);
    let requests = h.provider.requests.lock().unwrap().clone();
    assert!(
        !requests
            .last()
            .unwrap()
            .messages
            .iter()
            .flat_map(|message| &message.blocks)
            .any(|block| matches!(block, llm::Block::Text(text) if text.starts_with("Paused"))),
        "the pause is not replayed to the model"
    );
}

#[tokio::test]
async fn the_same_calls_with_the_same_results_pause_the_turn() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    h.provider
        .push(read_a())
        .push(read_a())
        .push(read_a())
        .push(text("WRAPPED: stuck on a.txt"))
        .push(text("never"));
    h.engine.submit(&h.session.id, prompt("loop")).await.await_ok();
    until_idle(&h).await;

    let mut messages = transcript(&h);
    let last = messages.pop().unwrap();
    assert_eq!(last.info.status, MessageStatus::Paused);
    assert!(
        last.info
            .error
            .as_deref()
            .unwrap()
            .contains("same calls and got the same results"),
        "{:?}",
        last.info.error
    );
    assert!(format!("{:?}", messages.last().unwrap().parts).contains("WRAPPED"));
    assert!(h.provider.requests.lock().unwrap()[3].no_tool_calls);
    assert_eq!(h.provider.responses_left(), 1);
}

#[tokio::test]
async fn a_subagent_at_its_step_limit_hands_back_what_it_found() {
    let h = harness().await;
    h.engine
        .store
        .update_session(&h.session.id, None, Some(&model()), None)
        .unwrap();
    std::fs::create_dir_all(h._dir.join("ws/.drift/agents")).unwrap();
    std::fs::write(
        h._dir.join("ws/.drift/agents/scout.md"),
        "---\nmode: subagent\nsteps: 2\n---\nScout.",
    )
    .unwrap();
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    h.provider
        .push(tool_call(
            "task",
            r#"{"description": "scout", "prompt": "look around", "subagent_type": "scout"}"#,
        ))
        .push(read_a())
        .push(text("FINDINGS: a.txt holds a"))
        .push(text("thanks"));
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    until_idle(&h).await;

    let tasks = h.engine.store.tasks_of(&h.session.id).unwrap();
    assert_eq!(
        tasks[0].state,
        crate::session::tasks::TaskState::Failed,
        "a write-up is not a finished answer: {:?}",
        tasks[0]
    );
    let messages = transcript(&h);
    assert_eq!(
        tool(&messages[1].parts[0]).metadata.unwrap().outcome.as_deref(),
        Some("incomplete")
    );
    let last = format!("{:?}", h.provider.requests.lock().unwrap().last().unwrap().messages);
    assert!(
        last.contains("FINDINGS") && last.contains("reached its step or repeat limit"),
        "the parent gets the write-up, marked partial"
    );
}

#[tokio::test]
async fn a_wrap_up_that_calls_a_tool_anyway_runs_nothing() {
    let h = harness().await;
    limits(&h, r#"{ "steps": 1 }"#);
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    h.provider.push(read_a());
    h.engine.submit(&h.session.id, prompt("work")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    let call = tool(&messages[1].parts[0]);
    assert_eq!(call.status, ToolStatus::Error, "{messages:#?}");
    assert!(call.output.unwrap().contains("tools were off"), "{:?}", call.output);
    assert!(
        !h.engine
            .store
            .read_files(&h.session.id)
            .unwrap_or_default()
            .iter()
            .any(|path| path.ends_with("a.txt")),
        "not even an early read ran"
    );
}

#[tokio::test]
async fn polling_that_waits_on_purpose_is_allowed_to_repeat() {
    let h = harness().await;
    rule(&h, "bash", "*", Decision::Allow);
    std::fs::write(h._dir.join("ws/status.txt"), "pending\n").unwrap();
    let poll = tool_call("bash", r#"{"command": "sleep 0 && cat status.txt"}"#);

    h.provider
        .push(poll.clone())
        .push(poll.clone())
        .push(poll.clone())
        .push(poll)
        .push(text("still pending, stopping"));

    h.engine.submit(&h.session.id, prompt("wait for it")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(
        transcript(&h).last().unwrap().info.status,
        MessageStatus::Done,
        "four identical polls are within the polling allowance"
    );
}

#[test]
fn only_identical_results_count_as_repeats() {
    let limits = crate::config::Limits::default();
    let call = |output: &str| {
        vec![CallTrace {
            name: "read".into(),
            input: r#"{"path":"a"}"#.into(),
            output: output.into(),
        }]
    };

    let mut repeats = Repeats::default();
    assert_eq!(repeats.record(call("1"), &limits), None);
    assert_eq!(
        repeats.record(call("2"), &limits),
        None,
        "a different result is progress"
    );
    assert_eq!(repeats.record(call("2"), &limits), None);
    assert_eq!(repeats.record(call("2"), &limits), Some(3));
    assert_eq!(repeats.record(Vec::new(), &limits), None, "a step without calls resets");

    let waiting = CallTrace {
        name: "bash".into(),
        input: r#"{"command":"Start-Sleep 5; gh run view"}"#.into(),
        output: "queued".into(),
    };
    assert!(waits(&waiting));
    assert!(
        !waits(&CallTrace {
            name: "bash".into(),
            input: r#"{"command":"cat sleepy.txt"}"#.into(),
            output: String::new()
        }),
        "a word inside a name is not a wait"
    );
}

#[test]
fn an_agent_can_have_its_own_step_limit() {
    let dir = std::env::temp_dir().join(format!("drift-limits-{}", crate::random_hex(4)));
    std::fs::create_dir_all(dir.join(".drift/agents")).unwrap();
    std::fs::write(dir.join("drift.json"), r#"{ "limits": { "steps": 50, "repeats": 4 } }"#).unwrap();
    std::fs::write(
        dir.join(".drift/agents/quick.md"),
        "---\ndescription: Quick\nsteps: 5\n---\nBe quick.",
    )
    .unwrap();

    let config = Config::load_with_home(&dir, None);
    assert_eq!(
        config.limits_for("build"),
        crate::config::Limits {
            steps: 50,
            repeats: 4,
            polls: 30
        }
    );
    assert_eq!(config.limits_for("quick").steps, 5);
    std::fs::remove_dir_all(dir).ok();
}
