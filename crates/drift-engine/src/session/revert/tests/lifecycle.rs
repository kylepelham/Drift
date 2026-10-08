use super::*;

#[tokio::test]
async fn undo_redo_and_moving_the_point_keep_files_and_history_in_step() {
    let h = harness().await;
    let (first, second) = two_writing_turns(&h).await;
    assert_eq!(
        (read(&h, "a.txt").as_deref(), read(&h, "b.txt").as_deref()),
        (Some("two"), Some("bee"))
    );
    let undone = h.engine.revert(&h.session.id, &second).await.unwrap();
    assert_eq!(undone.session.revert.as_ref().unwrap().message_id, second);
    assert!(undone.kept.is_empty());
    assert_eq!(
        (read(&h, "a.txt").as_deref(), read(&h, "b.txt")),
        (Some("one"), None),
        "back to before the second prompt"
    );

    h.engine.revert(&h.session.id, &first).await.unwrap();
    assert_eq!(read(&h, "a.txt"), None, "back to before anything was written");
    h.engine.revert(&h.session.id, &second).await.unwrap();
    assert_eq!(
        read(&h, "a.txt").as_deref(),
        Some("one"),
        "moving the point forward redoes the first turn"
    );

    let redone = h.engine.unrevert(&h.session.id).await.unwrap();
    assert!(redone.session.revert.is_none() && redone.kept.is_empty());
    assert_eq!(
        (read(&h, "a.txt").as_deref(), read(&h, "b.txt").as_deref()),
        (Some("two"), Some("bee")),
        "redo returns everything"
    );
    assert_eq!(
        h.engine.store.transcript(&h.session.id).unwrap().len(),
        7,
        "nothing was deleted along the way"
    );
}

#[tokio::test]
async fn an_undo_that_keeps_files_moves_only_the_conversation_and_a_later_one_starts_from_where_they_stand() {
    let h = harness().await;
    let (first, second) = two_writing_turns(&h).await;
    let kept = h.engine.revert_keeping_files(&h.session.id, &first).await.unwrap();
    assert_eq!(
        kept.session.revert.as_ref().unwrap().message_id,
        first,
        "the conversation goes back"
    );
    assert_eq!(
        (read(&h, "a.txt").as_deref(), read(&h, "b.txt").as_deref()),
        (Some("two"), Some("bee")),
        "the files stay"
    );

    h.engine.revert(&h.session.id, &second).await.unwrap();
    assert_eq!(
        (read(&h, "a.txt").as_deref(), read(&h, "b.txt")),
        (Some("one"), None),
        "an ordinary undo puts back from where the files stood"
    );
    h.engine.revert_keeping_files(&h.session.id, &first).await.unwrap();
    assert_eq!(read(&h, "a.txt").as_deref(), Some("one"), "still kept as they are");
    h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(
        (read(&h, "a.txt").as_deref(), read(&h, "b.txt").as_deref()),
        (Some("two"), Some("bee")),
        "redo returns the files the earlier undo put back"
    );

    h.engine.revert_keeping_files(&h.session.id, &second).await.unwrap();
    h.provider.push(text("carried on"));
    turn(&h, "again").await;
    let prompts = h
        .engine
        .store
        .transcript(&h.session.id)
        .unwrap()
        .iter()
        .filter(|message| message.info.role == Role::User)
        .count();
    assert_eq!(prompts, 2, "the next prompt commits the undo");
    assert_eq!(
        (read(&h, "a.txt").as_deref(), read(&h, "b.txt").as_deref()),
        (Some("two"), Some("bee")),
        "and the dropped turns' files stay"
    );
}

#[tokio::test]
async fn a_prompt_sent_while_undone_commits_the_undo() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    h.engine.revert(&h.session.id, &second).await.unwrap();
    let mut events = h.engine.hub.attach(None).rx;
    h.provider.push(text("fresh start"));
    turn(&h, "different second").await;

    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let texts: Vec<_> = transcript
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|row| match &row.part {
            Part::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["first", "wrote a", "different second", "fresh start"]);
    assert!(h.engine.store.session(&h.session.id).unwrap().unwrap().revert.is_none());
    assert_eq!(
        read(&h, "a.txt").as_deref(),
        Some("one"),
        "the files stay where the undo left them"
    );
    let mut removed = 0;
    while let Ok(envelope) = events.try_recv() {
        if matches!(envelope.event, Event::MessageRemoved { .. }) {
            removed += 1;
        }
    }
    assert_eq!(
        removed, 4,
        "the hidden prompt and its three replies are announced as gone"
    );
}

#[tokio::test]
async fn undo_refuses_non_prompts_and_stops_a_running_turn_before_undoing() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    let reply = h.engine.store.transcript(&h.session.id).unwrap()[1].info.id.clone();
    assert!(matches!(
        h.engine.revert(&h.session.id, &reply).await,
        Err(RevertError::NotAPrompt)
    ));
    let sleep = if cfg!(windows) {
        "ping -n 10 127.0.0.1"
    } else {
        "sleep 10"
    };
    allow_shell(&h);
    h.provider
        .push(tool_call("bash", &json!({ "command": sleep }).to_string()));
    h.engine.submit(&h.session.id, prompt("wait")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = std::time::Instant::now();
    let undone = h
        .engine
        .revert(&h.session.id, &second)
        .await
        .expect("the turn is stopped, then the undo runs");

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "it did not wait out the command"
    );
    assert_eq!(undone.session.revert.unwrap().message_id, second);
    assert_eq!((read(&h, "a.txt").as_deref(), read(&h, "b.txt")), (Some("one"), None));
    assert!(!h.engine.turns.is_running(&h.session.id));
}

#[tokio::test]
async fn undo_stops_an_mcp_call_under_way_instead_of_waiting_for_it() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/echo-server.cjs");
    let config = crate::mcp::ServerConfig::Stdio {
        command: "node".into(),
        args: vec![script.into()],
        env: Default::default(),
        cwd: None,
        timeout_seconds: None,
    };
    h.engine.store.save_mcp_server("echo", &config).unwrap();
    h.engine
        .connect_mcp_in("echo", Some(&crate::tool::canonical(&h._dir.join("ws"))))
        .await
        .unwrap();
    h.engine.permissions.set_policy(Policy {
        rules: vec![Rule {
            kind: "mcp".into(),
            pattern: "*".into(),
            decision: Decision::Allow,
        }],
    });
    h.provider
        .push(tool_call("echo_shout", &json!({ "text": "hang" }).to_string()));
    h.engine.submit(&h.session.id, prompt("hang")).await.unwrap();
    until_running_call(&h).await;
    let started = std::time::Instant::now();
    let undone = h
        .engine
        .revert(&h.session.id, &second)
        .await
        .expect("the call is stopped, then the undo runs");

    assert!(
        started.elapsed() < Duration::from_secs(2),
        "it waited {:?} for the call",
        started.elapsed()
    );
    assert_eq!(undone.session.revert.unwrap().message_id, second);
}

#[tokio::test]
async fn undo_puts_back_what_a_subagent_wrote() {
    let h = harness().await;
    allow_writes(&h);
    h.provider.push(text("ok"));
    turn(&h, "warm up").await;

    h.provider
        .push(tool_call(
            "task",
            r#"{"description": "Write c", "prompt": "write c.txt"}"#,
        ))
        .push(write("c.txt", "sea"))
        .push(text("child wrote c"))
        .push(text("parent done"));
    turn(&h, "delegate").await;
    assert_eq!(read(&h, "c.txt").as_deref(), Some("sea"));

    let delegated = h
        .engine
        .store
        .transcript(&h.session.id)
        .unwrap()
        .iter()
        .filter(|message| message.info.role == Role::User)
        .nth(1)
        .unwrap()
        .info
        .id
        .clone();
    h.engine.revert(&h.session.id, &delegated).await.unwrap();
    assert_eq!(
        read(&h, "c.txt"),
        None,
        "the subagent's write is undone with its parent's prompt"
    );

    h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(read(&h, "c.txt").as_deref(), Some("sea"));
}

#[tokio::test]
async fn while_undone_a_fork_copies_only_what_is_visible_and_compaction_waits() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    h.engine.revert(&h.session.id, &second).await.unwrap();

    let fork = h.engine.fork(&h.session.id, None).unwrap();
    let copied = h.engine.store.transcript(&fork.id).unwrap();
    assert_eq!(copied.len(), 3, "the first prompt, its write and its reply");
    assert!(copied.iter().all(|message| message.info.status == MessageStatus::Done));

    assert_eq!(h.engine.start_compaction(&h.session.id), Err(TurnError::Reverted));
    assert!(
        !h.engine.turns.is_running(&h.session.id),
        "the refused compaction left nothing claimed"
    );
}
