use super::*;

#[tokio::test]
async fn permission_denial_is_reported_to_the_model() {
    let h = harness().await;
    rule(&h, "bash", "rm *", Decision::Deny);
    h.provider
        .push(tool_call("bash", r#"{"command": "rm -rf /"}"#))
        .push(text("Understood"));
    h.engine.submit(&h.session.id, prompt("wipe it")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    assert_eq!(tool(&messages[1].parts[0]).status, ToolStatus::Denied);
    let requests = h.provider.requests.lock().unwrap();
    assert!(
        matches!(&requests[1].messages[2].blocks[0], llm::Block::ToolResult { is_error: true, content, .. }
        if content == "A permission rule forbids this call."),
        "a rule, not the user"
    );
}

#[tokio::test]
async fn an_edit_holds_the_previewed_file_while_approval_is_pending() {
    let h = harness().await;
    asks_for(&h, "edit");
    let file = h._dir.join("ws/a.txt");
    std::fs::write(&file, "one\none\n").unwrap();
    h.provider
        .push(tool_call("read", r#"{"path":"a.txt"}"#))
        .push(text("read"));
    h.engine.submit(&h.session.id, prompt("read a")).await.await_ok();
    until_idle(&h).await;

    let mut events = h.engine.hub.attach(None).rx;
    h.provider
        .push(tool_call(
            "edit",
            r#"{"path":"a.txt","old_string":"one","new_string":"two","replace_all":true}"#,
        ))
        .push(text("done"));
    h.engine.submit(&h.session.id, prompt("edit a")).await.await_ok();

    let ask = next_ask(&mut events).await;
    assert!(ask.ask.diff.as_deref().is_some_and(|diff| diff.contains("+two")));

    let other_file = file.clone();
    let other = tokio::spawn(async move {
        let _held = crate::tool::lock::files(std::slice::from_ref(&other_file)).await;
        tokio::fs::read_to_string(other_file).await.unwrap()
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(
        !other.is_finished(),
        "a competing writer waits until the approved edit is recorded"
    );

    reply_permission(&h, &ask.id, Reply::Once);
    until_idle(&h).await;

    assert_eq!(other.await.unwrap(), "two\ntwo\n");
}

#[tokio::test]
async fn explicit_rules_restrict_default_allowed_tools() {
    for (kind, input) in [
        ("read", r#"{"path":"a.txt"}"#),
        ("grep", r#"{"pattern":"secret"}"#),
        ("glob", r#"{"pattern":"*.txt"}"#),
        ("skill", r#"{"name":"private"}"#),
        (
            "task",
            r#"{"description":"inspect","prompt":"inspect files","subagent_type":"explore"}"#,
        ),
        ("todowrite", r#"{"todos":[]}"#),
        ("question", r#"{"questions":[]}"#),
    ] {
        let h = harness().await;
        std::fs::write(h._dir.join("ws/a.txt"), "secret contents").unwrap();
        rule(&h, kind, "*", Decision::Deny);
        h.provider.push(tool_call(kind, input)).push(text("done"));
        h.engine.submit(&h.session.id, prompt("inspect")).await.await_ok();
        until_idle(&h).await;

        let offered: Vec<_> = h.provider.requests.lock().unwrap()[0]
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect();
        assert!(
            !offered.contains(&kind.to_string()),
            "a tool every call of which is denied is not offered: {kind}"
        );
        let messages = transcript(&h);
        assert_ne!(
            tool(&messages[1].parts[0]).status,
            ToolStatus::Done,
            "and a call to it anyway does not run: {kind}"
        );
        assert_eq!(
            h.engine.store.session_tree(&h.session.id).unwrap().len(),
            1,
            "no task starts when delegation is denied"
        );
    }
}

#[test]
fn a_tool_is_denied_outright_only_when_nothing_before_the_blanket_rule_lets_a_call_through() {
    let compiled = |list: &[(&str, &str, Decision)]| {
        let rules: Vec<Rule> = list
            .iter()
            .map(|(kind, pattern, decision)| Rule {
                kind: kind.to_string(),
                pattern: pattern.to_string(),
                decision: *decision,
            })
            .collect();
        crate::permission::Compiled::new(rules)
    };

    assert!(
        compiled(&[("edit", "*", Decision::Deny)]).denies_all("edit"),
        "opencode's edit: deny"
    );
    assert!(compiled(&[("*", "*", Decision::Deny)]).denies_all("bash"));
    assert!(compiled(&[("bash", "rm *", Decision::Deny), ("bash", "*", Decision::Deny)]).denies_all("bash"));
    assert!(
        !compiled(&[("bash", "git *", Decision::Allow), ("bash", "*", Decision::Deny)]).denies_all("bash"),
        "git still runs"
    );
    assert!(!compiled(&[("bash", "*", Decision::Ask)]).denies_all("bash"));
    assert!(
        !compiled(&[("bash", "rm *", Decision::Deny)]).denies_all("bash"),
        "only part of it"
    );
}

#[tokio::test]
async fn agent_permissions_and_default_variant_are_applied_to_the_turn() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "private contents").unwrap();
    rule(&h, "read", "a.txt", Decision::Ask);
    let mut events = h.engine.hub.attach(None).rx;
    h.provider
        .push(tool_call("read", r#"{"path":"a.txt"}"#))
        .push(text("read"));
    h.engine.submit(&h.session.id, prompt("read")).await.await_ok();
    let ask = next_ask(&mut events).await;
    reply_permission(&h, &ask.id, Reply::Always);
    until_idle(&h).await;

    h.engine.set_agent_overrides(std::collections::HashMap::from([(
        "build".into(),
        crate::config::AgentOverride::from_json(&json!({
            "permissions":[{"kind":"read","pattern":"a.txt","decision":"deny"}], "variant":"high"
        })),
    )]));
    h.provider
        .push(tool_call("read", r#"{"path":"a.txt"}"#))
        .push(text("denied"));
    h.engine
        .submit(&h.session.id, prompt("read under restricted agent"))
        .await
        .await_ok();
    until_idle(&h).await;

    assert!(
        transcript(&h)
            .iter()
            .flat_map(|message| &message.parts)
            .any(|row| matches!(
                row.part,
                Part::ToolCall {
                    status: ToolStatus::Denied,
                    ..
                }
            )),
        "agent denial overrides an earlier always grant"
    );
    assert!(
        h.provider.requests.lock().unwrap().last().unwrap().reasoning.is_some(),
        "the configured default variant reaches the provider"
    );

    h.provider.push(text("low"));
    h.engine
        .submit(
            &h.session.id,
            Prompt {
                variant: Some(Some("low".into())),
                ..prompt("explicit level")
            },
        )
        .await
        .await_ok();
    until_idle(&h).await;

    assert_eq!(session(&h).variant.as_deref(), Some("low"));
}

#[tokio::test]
async fn ordinary_reads_allow_by_default_but_explicit_ask_requires_approval() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "read me").unwrap();
    h.provider
        .push(tool_call("read", r#"{"path":"a.txt"}"#))
        .push(text("done"));
    h.engine.submit(&h.session.id, prompt("read")).await.await_ok();
    until_idle(&h).await;

    assert!(h.engine.permissions.pending().is_empty());

    rule(&h, "read", "a.txt", Decision::Ask);
    let mut events = h.engine.hub.attach(None).rx;
    h.provider
        .push(tool_call("read", r#"{"path":"a.txt"}"#))
        .push(text("done"));
    h.engine.submit(&h.session.id, prompt("read again")).await.await_ok();

    let ask = next_ask(&mut events).await;
    assert_eq!(ask.ask.kind, "read");

    reply_permission(&h, &ask.id, Reply::Once);
    until_idle(&h).await;
}

#[tokio::test]
async fn a_refusal_tells_the_model_what_the_user_said_and_the_turn_goes_on() {
    let h = harness().await;
    asks_for(&h, "bash");
    let mut events = h.engine.hub.attach(None).rx;
    h.provider
        .push(tool_call("bash", r#"{"command": "rm -rf build"}"#))
        .push(text("Using cargo clean instead"));
    h.engine.submit(&h.session.id, prompt("clean up")).await.await_ok();
    let ask = next_ask(&mut events).await;
    let body = ReplyBody {
        reply: Reply::Deny,
        pattern: None,
        message: Some("use cargo clean".into()),
    };
    h.engine.permissions.reply(&h.engine.hub, &ask.id, body).unwrap();
    until_idle(&h).await;

    let requests = h.provider.requests.lock().unwrap().clone();
    let result = requests[1]
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .find_map(|block| match block {
            llm::Block::ToolResult { content, .. } => Some(content.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        result,
        "The user denied permission for this call. They said: use cargo clean"
    );
}

#[tokio::test]
async fn deny_and_stop_ends_the_turn() {
    let h = harness().await;
    asks_for(&h, "bash");
    let mut events = h.engine.hub.attach(None).rx;
    h.provider
        .push(tool_call("bash", r#"{"command": "rm -rf build"}"#))
        .push(text("never asked"));
    h.engine.submit(&h.session.id, prompt("clean up")).await.await_ok();
    let ask = next_ask(&mut events).await;
    reply_permission(&h, &ask.id, Reply::Stop);
    until_idle(&h).await;

    assert_eq!(
        h.provider.requests.lock().unwrap().len(),
        1,
        "no request after the stop"
    );
    let messages = transcript(&h);
    let denied = tool(&messages[1].parts[0]);
    assert_eq!(denied.status, ToolStatus::Denied);
    assert!(denied.output.unwrap().contains("stopped the turn"));
}

#[tokio::test]
async fn a_stop_mid_batch_leaves_no_call_pending() {
    let h = harness().await;
    asks_for(&h, "bash");
    let mut events = h.engine.hub.attach(None).rx;
    let call = |id, command| call_block(id, "bash", &format!(r#"{{"command": "{command}"}}"#));
    h.provider.push(
        [
            call("t1", "rm -rf build"),
            call("t2", "rm -rf dist"),
            vec![Chunk::Stop(StopReason::ToolUse)],
        ]
        .concat(),
    );
    h.engine.submit(&h.session.id, prompt("clean up")).await.await_ok();
    let ask = next_ask(&mut events).await;
    reply_permission(&h, &ask.id, Reply::Stop);
    until_idle(&h).await;

    let messages = transcript(&h);
    let unrun = tool(&messages[1].parts[1]);
    assert_eq!(
        unrun.status,
        ToolStatus::Error,
        "the call queued behind the stop is closed, not left pending"
    );
    assert!(
        unrun.output.unwrap().starts_with("Not run: the turn was stopped"),
        "{:?}",
        unrun.output
    );
}

#[tokio::test]
async fn asks_wait_for_a_reply_and_mutations_snapshot_first() {
    let h = harness().await;
    asks_for(&h, "edit");
    let mut events = h.engine.hub.attach(None).rx;
    h.provider
        .push(tool_call("write", r#"{"path": "new.txt", "content": "hi\n"}"#))
        .push(text("Written"));
    h.engine.submit(&h.session.id, prompt("make new.txt")).await.await_ok();

    let request = loop {
        let envelope = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        if let Event::PermissionAsked { request } = envelope.event {
            break request;
        }
    };
    assert_eq!(request.tool, "write");
    assert_eq!(request.ask.kind, "edit");

    reply_permission(&h, &request.id, Reply::Once);
    until_idle(&h).await;

    assert_eq!(std::fs::read_to_string(h._dir.join("ws/new.txt")).unwrap(), "hi\n");
    let messages = transcript(&h);
    let written = tool(&messages[1].parts[0]);
    assert_eq!(written.status, ToolStatus::Done);
    let changes = written.metadata.unwrap().changes.as_ref().unwrap();
    assert_eq!(changes[0].path, "new.txt", "{:?}", written.metadata);
    assert!(
        changes[0].before == Some(None) && changes[0].after.as_ref().is_some_and(Option::is_some),
        "a new file: nothing before, a blob after"
    );
}
