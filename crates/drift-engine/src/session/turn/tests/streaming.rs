use super::*;

#[tokio::test]
async fn a_response_records_its_generation_duration_with_its_usage() {
    let h = harness().await;
    h.provider.push_paused(
        vec![Chunk::TextStart, Chunk::TextDelta("first".into())],
        Duration::from_millis(30),
        vec![
            Chunk::TextDelta(" last".into()),
            Chunk::BlockStop,
            Chunk::Usage(Usage {
                output: 20,
                ..Usage::default()
            }),
            Chunk::Stop(StopReason::EndTurn),
        ],
    );
    h.engine.submit(&h.session.id, prompt("answer")).await.await_ok();
    until_idle(&h).await;
    let reply = transcript(&h).pop().unwrap().info;
    let measured = reply
        .generation_ms
        .expect("the completed response carries its generation duration");
    assert!(measured >= 30);
    assert_eq!(reply.usage.output, 20);
    assert_eq!(
        h.engine.store.message(&reply.id).unwrap().unwrap().generation_ms,
        Some(measured)
    );
}

#[tokio::test]
async fn a_read_starts_while_the_reply_still_streams() {
    let h = harness().await;
    let file = h._dir.join("ws/a.txt");
    std::fs::write(&file, "before\n").unwrap();
    let rest = [
        call_block("t2", "glob", r#"{"pattern": "*.txt"}"#),
        vec![Chunk::Stop(StopReason::ToolUse)],
    ]
    .concat();
    h.provider
        .push_paused(
            call_block("t1", "read", r#"{"path": "a.txt"}"#),
            Duration::from_millis(600),
            rest,
        )
        .push(text("done"));
    h.engine.submit(&h.session.id, prompt("read a")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(300)).await;
    std::fs::write(&file, "after\n").unwrap();
    until_idle(&h).await;

    let messages = transcript(&h);
    let read = tool(&messages[1].parts[0]);
    assert_eq!(
        (read.status, read.output),
        (ToolStatus::Done, Some("1: before")),
        "the read ran as soon as its call closed"
    );
    assert!(
        h.engine
            .turns
            .files_for(&h.engine.store, &h.session.id)
            .was_read(&crate::tool::canonical(&file)),
        "its result reached the model, so it counts as read"
    );
}

#[tokio::test]
async fn an_early_read_of_a_reply_that_fails_counts_for_nothing() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "secret plan\n").unwrap();
    h.provider.push_fail_midway(
        call_block("t1", "read", r#"{"path": "a.txt"}"#),
        llm::Error::Transport("connection reset".into()),
    );
    h.engine.submit(&h.session.id, prompt("read a")).await.await_ok();
    until_idle(&h).await;

    let file = crate::tool::canonical(&h._dir.join("ws/a.txt"));
    assert!(
        !h.engine.turns.files_for(&h.engine.store, &h.session.id).was_read(&file),
        "the model never saw it, so an edit must still read first"
    );
    let messages = transcript(&h);
    let read = tool(&messages[1].parts[0]);
    assert!(
        read.status != ToolStatus::Done && !read.output.unwrap_or_default().contains("secret"),
        "{:?}",
        read.output
    );
}

#[tokio::test]
async fn a_stream_that_ends_without_a_stop_reason_runs_no_tools() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "alpha\n").unwrap();
    let truncated = call_block("t1", "read", r#"{"path": "a.txt"}"#);
    for _ in 0..=MAX_RETRIES {
        h.provider.push(truncated.clone());
    }
    h.engine.submit(&h.session.id, prompt("read a")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    for message in messages.iter().skip(1) {
        assert_eq!(message.info.status, MessageStatus::Error);
        let call = tool(&message.parts[0]);
        assert_eq!(
            call.status,
            ToolStatus::Error,
            "the call must never run, and is closed rather than left pending"
        );
        assert!(
            call.output.is_some_and(|output| output.starts_with("Not run:")),
            "{:?}",
            call.output
        );
    }

    assert_eq!(
        messages.len(),
        2 + MAX_RETRIES as usize,
        "the first attempt and every retry, then it gives up"
    );
}

#[tokio::test]
async fn a_refused_reply_says_so_and_runs_nothing() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    let mut refused = call_block("t1", "read", r#"{"path": "a.txt"}"#);
    refused.push(Chunk::Stop(StopReason::Refused));
    h.provider.push(refused);
    h.engine.submit(&h.session.id, prompt("read")).await.await_ok();
    until_idle(&h).await;

    let last = transcript(&h).pop().unwrap();
    assert_eq!(
        (last.info.status, last.info.error.as_deref()),
        (MessageStatus::Done, Some(REFUSED_ENDING)),
        "never a silent end"
    );
    assert_eq!(
        last.info.ending,
        Some(crate::session::types::Ending::Refused),
        "typed, so the UI never reads the wording"
    );
    assert_eq!(tool(&last.parts[0]).status, ToolStatus::Error);
}

#[tokio::test]
async fn a_reply_that_fills_the_context_window_compacts_and_asks_again() {
    let h = harness().await;
    h.provider
        .push(vec![
            Chunk::TextStart,
            Chunk::TextDelta("half an ans".into()),
            Chunk::BlockStop,
            Chunk::Stop(StopReason::ContextFull),
        ])
        .push(text("SUMMARY"))
        .push(text("the whole answer"));
    h.engine.submit(&h.session.id, prompt("long")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    assert!(messages.iter().any(|message| message.info.summary), "compacted");
    let answer = messages.last().unwrap().parts.iter().find_map(|row| match &row.part {
        Part::Text { text } => Some(text.as_str()),
        _ => None,
    });
    assert_eq!(answer, Some("the whole answer"));
    let cut = messages
        .iter()
        .find(|message| {
            message
                .parts
                .iter()
                .any(|row| matches!(&row.part, Part::Text { text } if text == "half an ans"))
        })
        .unwrap();
    assert_eq!(cut.info.status, MessageStatus::Error, "the cut reply is never replayed");
}

#[tokio::test]
async fn a_reply_cut_off_without_calls_still_says_so() {
    let h = harness().await;
    h.provider.push(vec![
        Chunk::TextStart,
        Chunk::TextDelta("The answer is".into()),
        Chunk::BlockStop,
        Chunk::Stop(StopReason::MaxTokens),
    ]);
    h.engine.submit(&h.session.id, prompt("long")).await.await_ok();
    until_idle(&h).await;

    let last = transcript(&h).pop().unwrap();
    assert_eq!(last.info.status, MessageStatus::Done);
    assert!(
        last.info
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with(OUTPUT_LIMIT_ENDING))
    );
    assert_eq!(last.info.ending, Some(crate::session::types::Ending::Length));
}

#[tokio::test]
async fn a_shell_call_shows_its_limit_while_running_and_fails_when_it_expires() {
    let h = harness().await;
    rule(&h, "bash", "*", Decision::Allow);
    h.engine.set_shell_timeout(Some(Duration::from_millis(400)));
    let sleep = if cfg!(windows) {
        "ping -n 10 127.0.0.1"
    } else {
        "sleep 10"
    };
    let mut events = h.engine.hub.attach(None).rx;
    h.provider
        .push(tool_call("bash", &json!({ "command": sleep }).to_string()))
        .push(text("it was too slow"));
    h.engine.submit(&h.session.id, prompt("wait")).await.await_ok();

    let running = loop {
        let envelope = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .unwrap()
            .unwrap();
        if let Event::PartUpdated { part } = envelope.event
            && let Part::ToolCall {
                status: ToolStatus::Running,
                metadata,
                ..
            } = part.part
        {
            break metadata;
        }
    };
    assert_eq!(
        running.unwrap().shell_timeout_ms,
        Some(Some(400)),
        "the badge has the limit while the command runs"
    );
    until_idle(&h).await;

    let messages = transcript(&h);
    let call = tool(&messages[1].parts[0]);
    assert_eq!(call.status, ToolStatus::Error);
    let metadata = call.metadata.unwrap();
    assert_eq!(
        (metadata.timed_out, metadata.shell_timeout_ms.flatten()),
        (Some(true), Some(400))
    );
    assert!(
        metadata.changes.is_some(),
        "what the command changed is recorded next to the timeout details"
    );
}

#[tokio::test]
async fn abort_marks_the_message_and_frees_the_session() {
    let h = harness().await;
    rule(&h, "bash", "*", Decision::Allow);
    let sleep = if cfg!(windows) {
        "ping -n 10 127.0.0.1"
    } else {
        "sleep 10"
    };
    h.provider
        .push(tool_call("bash", &json!({ "command": sleep }).to_string()));
    h.engine.submit(&h.session.id, prompt("wait")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(300)).await;
    h.engine
        .submit(&h.session.id, prompt("again"))
        .await
        .expect("steered into the running turn");
    assert!(h.engine.abort(&h.session.id));
    until_idle(&h).await;

    let messages = transcript(&h);
    assert_eq!(tool(&messages[1].parts[0]).status, ToolStatus::Error);
    assert!(!h.engine.abort(&h.session.id));
    assert_eq!(
        h.provider.requests.lock().unwrap().len(),
        1,
        "a stop is not overridden by a steered prompt"
    );
    assert_eq!(
        messages.last().unwrap().info.role,
        Role::User,
        "the steered prompt stays for the next turn"
    );
}

#[tokio::test]
async fn a_running_command_shows_its_output_before_it_ends() {
    let h = harness().await;
    rule(&h, "bash", "*", Decision::Allow);
    let command = if cfg!(windows) {
        "echo early && ping -n 3 127.0.0.1 > /dev/null"
    } else {
        "echo early; sleep 2"
    };
    let mut events = h.engine.hub.attach(None).rx;
    h.provider
        .push(tool_call("bash", &json!({ "command": command }).to_string()))
        .push(text("done"));
    h.engine.submit(&h.session.id, prompt("run")).await.await_ok();

    let shown = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.unwrap();
            if let Event::PartUpdated { part } = envelope.event
                && let Part::ToolCall {
                    status: ToolStatus::Running,
                    metadata: Some(metadata),
                    ..
                } = part.part
                && metadata
                    .output
                    .as_deref()
                    .is_some_and(|output| output.contains("early"))
            {
                return metadata;
            }
        }
    })
    .await
    .expect("the output so far is published while the command runs");
    assert!(shown.shell_timeout_ms.is_some(), "running metadata is kept beside it");
    until_idle(&h).await;

    let messages = transcript(&h);
    let call = tool(&messages[1].parts[0]);
    assert_eq!(call.status, ToolStatus::Done);
    assert!(call.output.unwrap().contains("early"));
}
