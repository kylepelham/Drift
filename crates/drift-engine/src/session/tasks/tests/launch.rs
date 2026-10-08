use super::*;

#[test]
fn the_mode_is_what_was_asked_then_the_agents_default_then_the_foreground() {
    assert_eq!(
        resolve_mode(Some(true), Some(false), true),
        Ok((Mode::Background, "requested"))
    );
    assert_eq!(
        resolve_mode(Some(false), Some(true), true),
        Ok((Mode::Foreground, "requested"))
    );
    assert_eq!(
        resolve_mode(None, Some(true), true),
        Ok((Mode::Background, "agent default"))
    );
    assert_eq!(
        resolve_mode(None, Some(false), true),
        Ok((Mode::Foreground, "agent default"))
    );
    assert_eq!(resolve_mode(None, None, true), Ok((Mode::Foreground, "default")));
    assert!(
        resolve_mode(Some(true), None, false)
            .unwrap_err()
            .to_string()
            .contains("turned off"),
        "an explicit request is refused, not quietly changed"
    );
    assert_eq!(
        resolve_mode(None, Some(true), false),
        Ok((Mode::Foreground, "background turned off"))
    );
}

#[tokio::test]
async fn a_worker_thinks_at_its_parents_reasoning_level() {
    let h = harness().await;
    h.engine.store.set_session_variant(&h.session.id, Some("max")).unwrap();
    h.provider
        .push_for(
            "PARENT",
            launches(&[json!({ "description": "Look", "prompt": "CHILD look around" })]),
        )
        .push_for("CHILD", text("found it"))
        .push_for("PARENT", text("done"));
    h.engine.submit(&h.session.id, prompt("PARENT go")).await.unwrap();
    until_idle(&h).await;

    let child = h.engine.store.session(&tasks(&h)[0].session_id).unwrap().unwrap();
    assert_eq!(child.variant.as_deref(), Some("max"));
    let requests = h.provider.requests.lock().unwrap();
    let asked_child = requests
        .iter()
        .find(|request| format!("{:?}", request.messages).contains("CHILD look around"))
        .unwrap();
    assert!(
        matches!(
            asked_child.reasoning,
            Some(crate::llm::catalog::Reasoning::Budget { .. })
        ),
        "{:?}",
        asked_child.reasoning
    );
}

#[tokio::test]
async fn a_background_worker_returns_a_receipt_and_its_result_arrives_later() {
    let h = harness().await;
    h.provider
        .push_for("PARENT", launches(&[background("Survey", "CHILD survey the code")]))
        .push_for("PARENT", text("working meanwhile"))
        .push_for("PARENT", text("thanks for the survey"))
        .push_slow_for("CHILD survey", Duration::from_millis(700), text("three things found"));
    h.engine.submit(&h.session.id, prompt("PARENT goal")).await.unwrap();
    until("the parent's own turn ends", || {
        !h.engine.turns.is_running(&h.session.id)
    })
    .await;

    let launched = tasks(&h).pop().unwrap();
    assert!(
        !launched.state.is_terminal(),
        "the parent finished its turn while the worker still ran"
    );
    assert_eq!(
        (launched.mode, launched.reason.as_str()),
        (Mode::Background, "requested")
    );
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let call = tool(&transcript[1].parts[0]);
    assert_eq!(call.status, ToolStatus::Done, "a launch is a successful call");
    assert!(
        call.output.unwrap().starts_with("Started Survey in the background"),
        "{:?}",
        call.output
    );

    until("the result is delivered", || tasks(&h)[0].delivered).await;
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert_eq!(
        delivered_results(&transcript),
        [("Survey".to_string(), "three things found".to_string())],
        "delivered once"
    );
    let texts: Vec<_> = h
        .provider
        .requests
        .lock()
        .unwrap()
        .iter()
        .flat_map(|request| request.messages.iter().flat_map(|message| message.blocks.clone()))
        .filter_map(|block| match block {
            crate::llm::Block::Text(text) => Some(text),
            _ => None,
        })
        .collect();
    assert!(
        texts
            .iter()
            .any(|text| text.starts_with("<task-result") && text.contains("three things found")),
        "the parent's model saw the result"
    );
    assert_eq!(
        h.provider.responses_left(),
        0,
        "the result started the parent's next turn"
    );
}

#[tokio::test]
async fn workers_finish_out_of_order_each_with_its_own_result() {
    let h = harness().await;
    h.provider
        .push_for(
            "PARENT",
            launches(&[
                background("Slow one", "CHILD slow"),
                background("Quick one", "CHILD quick"),
            ]),
        )
        .push_for("PARENT", text("both launched"))
        .push_for("PARENT", text("noted one"))
        .push_for("PARENT", text("noted two"))
        .push_slow_for("CHILD slow", Duration::from_millis(800), text("slow result"))
        .push_for("CHILD quick", text("quick result"));
    h.engine.submit(&h.session.id, prompt("PARENT two jobs")).await.unwrap();
    until("both delivered", || {
        tasks(&h).len() == 2 && tasks(&h).iter().all(|task| task.delivered)
    })
    .await;
    until_idle(&h).await;

    assert_eq!(
        delivered_results(&h.engine.store.transcript(&h.session.id).unwrap()),
        [
            ("Quick one".to_string(), "quick result".to_string()),
            ("Slow one".to_string(), "slow result".to_string())
        ],
        "in the order they finished"
    );
}

#[tokio::test]
async fn a_workers_permission_wait_blocks_only_that_worker() {
    let h = harness().await;
    crate::session::turn::tests::asks_for(&h, "bash");
    let mut events = h.engine.hub.attach(None).rx;
    h.provider
        .push_for("PARENT", launches(&[background("Build", "CHILD build it")]))
        .push_for("PARENT", text("launched the build"))
        .push_for("PARENT", text("build result noted"))
        .push_for("CHILD build", tool_call("bash", r#"{"command": "echo built"}"#))
        .push_for("CHILD build", text("built"));
    h.engine.submit(&h.session.id, prompt("PARENT build")).await.unwrap();
    let ask = loop {
        let envelope = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap();
        if let Event::PermissionAsked { request } = envelope.event {
            break request;
        }
    };
    let worker = tasks(&h).pop().unwrap();
    assert_eq!(ask.session_id, worker.session_id, "attributed to the worker");
    until("the parent's turn ends while the worker waits", || {
        !h.engine.turns.is_running(&h.session.id)
    })
    .await;
    h.engine
        .permissions
        .reply(
            &h.engine.hub,
            &ask.id,
            ReplyBody {
                reply: Reply::Once,
                pattern: None,
                message: None,
            },
        )
        .unwrap();
    until("delivered", || tasks(&h)[0].delivered).await;
    until_idle(&h).await;
    assert_eq!(tasks(&h)[0].state, TaskState::Replied);
}

fn children(h: &Harness) -> usize {
    h.engine
        .store
        .sessions(crate::store::SessionFilter {
            workspace_id: Some(&h.session.workspace_id),
            archived: false,
            before: None,
            limit: 50,
        })
        .unwrap()
        .into_iter()
        .filter(|session| session.parent_id.as_deref() == Some(h.session.id.as_str()))
        .count()
}

#[tokio::test]
async fn the_same_launch_again_gets_what_it_launched_and_makes_nothing_new() {
    let h = harness().await;
    with_model(&h);
    h.provider
        .push_stall_for("CHILD again")
        .push_for("CHILD front", text("front answer"));
    let ctx = context(&h, &h.session.id, "bg_call");
    let first = crate::tool::task::Task
        .run(&ctx, background("Again", "CHILD again"))
        .await
        .unwrap();
    let second = crate::tool::task::Task
        .run(&ctx, background("Again", "CHILD again"))
        .await
        .unwrap();
    assert_eq!(first.metadata.task_id, second.metadata.task_id);
    assert!(
        second.output.starts_with("Started Again in the background"),
        "{}",
        second.output
    );
    assert_eq!(
        (children(&h), tasks(&h).len()),
        (1, 1),
        "no second transcript, no second worker"
    );

    let ctx = context(&h, &h.session.id, "fg_call");
    let front = json!({ "description": "Front", "prompt": "CHILD front" });
    let first = crate::tool::task::Task.run(&ctx, front.clone()).await.unwrap();
    assert!(
        first.output.starts_with("front answer\n\n(task_id: task_"),
        "{}",
        first.output
    );
    let again = crate::tool::task::Task.run(&ctx, front).await.unwrap();
    assert_eq!(
        (again.output.as_str(), again.metadata.mode.as_deref()),
        (first.output.as_str(), Some("foreground")),
        "a foreground replay is its result, not a background receipt"
    );
    assert_eq!((children(&h), tasks(&h).len()), (2, 2));
    h.engine.abort(&h.session.id);
}

#[tokio::test]
async fn with_background_turned_off_an_explicit_request_is_refused_and_foreground_still_waits() {
    let h = harness().await;
    h.engine.store.set_setting(BACKGROUND_TASKS_KEY, &false).unwrap();
    h.provider
        .push_for("PARENT", launches(&[background("Refused", "CHILD refused")]))
        .push_for(
            "PARENT",
            launches(&[json!({ "description": "Waited", "prompt": "CHILD waited" })]),
        )
        .push_for("PARENT", text("done"))
        .push_for("CHILD waited", text("waited result"));
    h.engine.submit(&h.session.id, prompt("PARENT off")).await.unwrap();
    until_idle(&h).await;

    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let refused = tool(&transcript[1].parts[0]);
    assert!(refused.output.unwrap().contains("turned off"), "{:?}", refused.output);
    let recorded = tasks(&h);
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        (recorded[0].mode, recorded[0].state, recorded[0].delivered),
        (Mode::Foreground, TaskState::Replied, true)
    );
    assert!(
        tool(&transcript[2].parts[0])
            .output
            .unwrap()
            .starts_with("waited result\n\n(task_id:"),
        "foreground returns the result as the call's own"
    );
}
