use super::*;

#[tokio::test]
async fn a_turn_has_no_step_limit_even_when_an_old_config_sets_one() {
    let h = harness().await;
    limits(&h, r#"{ "steps": 1 }"#);
    for name in ["a", "b", "c"] {
        std::fs::write(h._dir.join(format!("ws/{name}.txt")), format!("{name}\n")).unwrap();
    }
    h.provider
        .push(tool_call("read", r#"{"path": "a.txt"}"#))
        .push(tool_call("read", r#"{"path": "b.txt"}"#))
        .push(tool_call("read", r#"{"path": "c.txt"}"#))
        .push(text("read all three"));
    h.engine.submit(&h.session.id, prompt("work")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(transcript(&h).last().unwrap().info.status, MessageStatus::Done);
    assert_eq!(h.provider.responses_left(), 0);
    assert!(
        h.provider
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| !request.no_tool_calls),
        "tools stay on for every step"
    );
}

#[tokio::test]
async fn a_paused_turn_carries_on_with_a_message_and_the_pause_is_not_replayed() {
    let h = harness().await;
    limits(&h, r#"{ "repeats": 2 }"#);
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    h.provider
        .push(read_a())
        .push(read_a())
        .push(text("WRAPPED: stuck on a.txt"))
        .push(text("finished"));
    h.engine.submit(&h.session.id, prompt("work")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(transcript(&h).last().unwrap().info.status, MessageStatus::Paused);
    assert_eq!(h.provider.responses_left(), 1, "no request after the pause");

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
async fn a_subagent_stopped_for_repeating_hands_back_what_it_found() {
    let h = harness().await;
    limits(&h, r#"{ "repeats": 2 }"#);
    h.engine
        .store
        .update_session(&h.session.id, None, Some(&model()), None)
        .unwrap();
    std::fs::create_dir_all(h._dir.join("ws/.drift/agents")).unwrap();
    std::fs::write(
        h._dir.join("ws/.drift/agents/scout.md"),
        "---\nmode: subagent\n---\nScout.",
    )
    .unwrap();
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    h.provider
        .push(tool_call(
            "task",
            r#"{"description": "scout", "prompt": "look around", "subagent_type": "scout"}"#,
        ))
        .push(read_a())
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
        last.contains("FINDINGS") && last.contains("kept repeating the same calls"),
        "the parent gets the write-up, marked partial"
    );
}

#[tokio::test]
async fn a_wrap_up_that_calls_a_tool_anyway_runs_nothing() {
    let h = harness().await;
    limits(&h, r#"{ "repeats": 2 }"#);
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    std::fs::write(h._dir.join("ws/b.txt"), "b\n").unwrap();
    h.provider
        .push(read_a())
        .push(read_a())
        .push(tool_call("read", r#"{"path": "b.txt"}"#));
    h.engine.submit(&h.session.id, prompt("work")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    let call = tool(&messages[3].parts[0]);
    assert_eq!(call.status, ToolStatus::Error, "{messages:#?}");
    assert!(call.output.unwrap().contains("tools were off"), "{:?}", call.output);
    assert!(
        !h.engine
            .store
            .read_files(&h.session.id)
            .unwrap_or_default()
            .iter()
            .any(|path| path.ends_with("b.txt")),
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
            images: Vec::new(),
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
        images: Vec::new(),
    };
    assert!(waits(&waiting));
    assert!(
        !waits(&CallTrace {
            name: "bash".into(),
            input: r#"{"command":"cat sleepy.txt"}"#.into(),
            output: String::new(),
            images: Vec::new(),
        }),
        "a word inside a name is not a wait"
    );
}

fn screenshot(hash: &str) -> Vec<CallTrace> {
    vec![CallTrace {
        name: "HvDebugger_screenshot".into(),
        input: r#"{"monitor":1}"#.into(),
        output: "monitor 1 1920x1200 at (0, 0), duplication".into(),
        images: vec![hash.into()],
    }]
}

#[test]
fn a_screen_being_watched_is_progress_while_it_changes_and_polling_while_it_does_not() {
    let limits = crate::config::Limits::default();
    let mut repeats = Repeats::default();
    for hash in ["frame-1", "frame-2", "frame-3", "frame-4"] {
        assert_eq!(
            repeats.record(screenshot(hash), &limits),
            None,
            "the same caption over a new image is progress"
        );
    }

    let mut still = Repeats::default();
    for _ in 1..limits.polls {
        assert_eq!(still.record(screenshot("loading"), &limits), None);
    }
    assert_eq!(
        still.record(screenshot("loading"), &limits),
        Some(limits.polls),
        "an unchanged screen gets the polling allowance, not the repeat one"
    );

    let image_file = vec![CallTrace {
        name: "read".into(),
        input: r#"{"path":"logo.png"}"#.into(),
        output: "logo.png".into(),
        images: vec!["logo".into()],
    }];
    let mut reread = Repeats::default();
    for _ in 1..limits.repeats {
        assert_eq!(reread.record(image_file.clone(), &limits), None);
    }
    assert_eq!(
        reread.record(image_file, &limits),
        Some(limits.repeats),
        "an image file read again is a repeat, not a poll"
    );

    let call = |name: &str, input: &str| CallTrace {
        name: name.into(),
        input: input.into(),
        output: "still running".into(),
        images: Vec::new(),
    };
    assert!(waits(&call("process", r#"{"action":"wait","id":"p1"}"#)));
    assert!(waits(&call("task_output", r#"{"task_id":"t1","wait_seconds":600}"#)));
    assert!(!waits(&call("task_output", r#"{"task_id":"t1"}"#)));
    assert!(!waits(&call("process", r#"{"action":"output","id":"p1"}"#)));
}

#[tokio::test]
async fn the_loop_check_sees_the_images_a_call_returned() {
    let h = harness().await;
    let reply = h.engine.store.create_reply(&h.session.id, &model(), "build").unwrap();
    let metadata = crate::session::types::ToolMetadata::from(serde_json::json!({
        "images": [{ "mime": "image/png", "hash": "abc123" }]
    }));
    h.engine
        .store
        .add_part(
            &reply.id,
            &h.session.id,
            Part::ToolCall {
                call_id: "c1".into(),
                name: "screenshot".into(),
                input: serde_json::json!({}),
                status: ToolStatus::Done,
                title: None,
                output: Some("monitor 1".into()),
                metadata: Some(Box::new(metadata)),
                started_at: None,
                finished_at: None,
            },
        )
        .unwrap();

    let calls = h.engine.last_calls(&h.session.id);
    assert_eq!(calls[0].images, ["abc123"]);
}

#[test]
fn old_step_settings_load_without_complaint() {
    let dir = std::env::temp_dir().join(format!("drift-limits-{}", crate::random_hex(4)));
    std::fs::create_dir_all(dir.join(".drift/agents")).unwrap();
    std::fs::write(dir.join("drift.json"), r#"{ "limits": { "steps": 50, "repeats": 4 } }"#).unwrap();
    std::fs::write(
        dir.join(".drift/agents/quick.md"),
        "---\ndescription: Quick\nsteps: 5\n---\nBe quick.",
    )
    .unwrap();

    let config = Config::load_with_home(&dir, None);
    assert_eq!(config.limits, crate::config::Limits { repeats: 4, polls: 30 });
    assert!(config.agent("quick").unwrap().problem.is_none());
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    std::fs::remove_dir_all(dir).ok();
}
