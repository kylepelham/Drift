use super::*;

fn calls(h: &Harness) -> Vec<(String, ToolStatus, String)> {
    transcript(h)
        .iter()
        .flat_map(|message| &message.parts)
        .filter(|row| matches!(row.part, Part::ToolCall { .. }))
        .map(|row| {
            let call = tool(row);
            (call.name.into(), call.status, call.output.unwrap_or_default().into())
        })
        .collect()
}

#[tokio::test]
async fn a_stopped_commands_setup_calls_that_never_ran_are_closed() {
    let h = harness().await;
    h.engine
        .store
        .update_session(&h.session.id, None, Some(&model()), None)
        .unwrap();
    rule(&h, "bash", "cargo publish*", Decision::Ask);
    let commands = h._dir.join("ws/.drift/commands");
    std::fs::create_dir_all(&commands).unwrap();
    std::fs::write(
        commands.join("ship.md"),
        "Ship with !`cargo publish` then !`echo after`.",
    )
    .unwrap();
    let mut events = h.engine.hub.attach(None).rx;
    h.engine.execute_command(&h.session.id, "ship", "", None).await.unwrap();
    next_ask(&mut events).await;

    assert!(h.engine.abort(&h.session.id));
    until_idle(&h).await;

    let calls = calls(&h);
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(
        calls.iter().all(|(_, status, _)| *status == ToolStatus::Error),
        "{calls:?}"
    );
    assert_eq!(calls[1].2, "Not run: the turn was stopped before this call ran.");
    assert!(h.provider.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_reply_that_cannot_be_saved_closes_its_calls() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    h.engine
        .store
        .lock()
        .execute_batch(
            "CREATE TEMP TRIGGER reject_reply BEFORE UPDATE OF status ON message \
             WHEN NEW.status = 'done' AND OLD.role = 'assistant' BEGIN SELECT RAISE(ABORT, 'injected'); END;",
        )
        .unwrap();
    h.provider.push(read_a());
    h.engine.submit(&h.session.id, prompt("read a")).await.await_ok();
    until_idle(&h).await;

    let calls = calls(&h);
    assert_eq!(calls.len(), 1);
    assert_eq!(
        (calls[0].1, calls[0].2.as_str()),
        (
            ToolStatus::Error,
            "Not run: the reply could not be saved, so this call was not run."
        )
    );
}

/// A reply that finished `finished` calls and hit its output limit while writing one more.
fn cut_off_after(finished: &[(&str, &str, &str)], cut: (&str, &str)) -> Vec<Chunk> {
    let mut chunks: Vec<Chunk> = finished
        .iter()
        .flat_map(|(id, name, input)| call_block(id, name, input))
        .collect();
    chunks.extend([
        Chunk::ToolUseStart {
            id: format!("{}_cut", finished.first().map_or("toolu", |call| call.0)),
            name: cut.0.into(),
        },
        Chunk::ToolInputDelta(cut.1.into()),
        Chunk::BlockStop,
        Chunk::Stop(StopReason::MaxTokens),
    ]);

    chunks
}

#[tokio::test]
async fn a_reply_cut_off_at_its_output_limit_runs_the_calls_it_finished() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "alpha\n").unwrap();
    std::fs::write(h._dir.join("ws/b.txt"), "beta\n").unwrap();
    let finished = [
        ("toolu_a", "read", r#"{"path": "a.txt"}"#),
        ("toolu_b", "read", r#"{"path": "b.txt"}"#),
    ];
    h.provider
        .push(cut_off_after(
            &finished,
            ("write", r#"{"path": "big.txt", "content": "AAAA"#),
        ))
        .push(text("carried on"));
    h.engine
        .submit(&h.session.id, prompt("read and write"))
        .await
        .await_ok();
    until_idle(&h).await;

    let calls = calls(&h);
    assert_eq!(
        calls.iter().map(|(_, status, _)| *status).collect::<Vec<_>>(),
        [ToolStatus::Done, ToolStatus::Done, ToolStatus::Error],
        "{calls:?}"
    );
    assert!(
        calls[2].2.ends_with("several smaller writes or edits."),
        "{}",
        calls[2].2
    );
    assert!(!h._dir.join("ws/big.txt").exists());

    let requests = h.provider.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2, "the turn goes on after the cut");
    let results: Vec<_> = requests[1]
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| match block {
            llm::Block::ToolResult { content, is_error, .. } => Some((content.clone(), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 3);
    assert!(results[0].0.contains("alpha") && results[1].0.contains("beta") && results[2].1);
    assert!(
        format!("{:?}", requests[1].messages).contains("hit the output limit"),
        "the model is told to go on in smaller pieces"
    );
    assert_eq!(
        transcript(&h).last().map(|message| message.info.status),
        Some(MessageStatus::Done)
    );
    assert_eq!(calls.len(), 3);
}

#[tokio::test]
async fn a_subagent_launch_cut_off_at_the_output_limit_says_to_start_fewer() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "alpha\n").unwrap();
    h.provider
        .push(cut_off_after(
            &[("toolu_a", "read", r#"{"path": "a.txt"}"#)],
            ("task", r#"{"description": "look", "prompt": "Look at every"#),
        ))
        .push(text("carried on"));
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    until_idle(&h).await;

    let calls = calls(&h);
    assert_eq!(calls[0].1, ToolStatus::Done, "{calls:?}");
    assert_eq!((calls[1].0.as_str(), calls[1].1), ("task", ToolStatus::Error));
    assert!(calls[1].2.contains("Start fewer subagents per reply"), "{}", calls[1].2);
}

#[tokio::test]
async fn a_cut_off_call_with_no_input_yet_is_not_run() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "alpha\n").unwrap();
    h.provider
        .push(cut_off_after(
            &[("toolu_a", "read", r#"{"path": "a.txt"}"#)],
            ("read", ""),
        ))
        .push(text("carried on"));
    h.engine.submit(&h.session.id, prompt("read")).await.await_ok();
    until_idle(&h).await;

    let calls = calls(&h);
    assert_eq!(
        calls.iter().map(|(_, status, _)| *status).collect::<Vec<_>>(),
        [ToolStatus::Done, ToolStatus::Error],
        "{calls:?}"
    );
    assert!(calls[1].2.starts_with("Not run: the reply hit its output limit"));
}

#[tokio::test]
async fn the_cut_off_that_ends_the_turn_runs_none_of_its_calls() {
    let h = harness().await;
    for name in ["a", "b", "c"] {
        std::fs::write(h._dir.join(format!("ws/{name}.txt")), "text\n").unwrap();
    }
    for (id, path) in [("toolu_a", "a.txt"), ("toolu_b", "b.txt"), ("toolu_c", "c.txt")] {
        let input = format!(r#"{{"path": "{path}"}}"#);
        h.provider.push(cut_off_after(
            &[(id, "read", input.as_str())],
            ("write", r#"{"path": "big.txt"#),
        ));
    }
    h.engine.submit(&h.session.id, prompt("read")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(
        h.provider.requests.lock().unwrap().len(),
        3,
        "the third cut ends the turn"
    );
    let calls = calls(&h);
    assert_eq!(calls.len(), 6, "{calls:?}");
    assert_eq!(
        (calls[0].1, calls[2].1),
        (ToolStatus::Done, ToolStatus::Done),
        "{calls:?}"
    );
    assert_eq!(
        (calls[4].1, calls[4].2.as_str()),
        (
            ToolStatus::Error,
            "Not run: the reply hit its output limit again and the turn ended there, so this call was not run."
        ),
        "its result would never reach the model"
    );
}
