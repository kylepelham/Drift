use super::*;

#[tokio::test]
async fn a_prompt_sent_during_a_call_reaches_the_next_request_after_its_result() {
    let h = harness().await;
    rule(&h, "bash", "*", Decision::Allow);
    h.provider
        .push(tool_call("bash", r#"{"command": "sleep 1"}"#))
        .push(text("done, and noted"));
    h.engine.submit(&h.session.id, prompt("start")).await.await_ok();
    until_call_running(&h).await;
    let mut steer = prompt("also check the logs");
    steer.submission_id = Some("steer-1".into());
    let first = h
        .engine
        .submit(&h.session.id, steer.clone())
        .await
        .expect("a busy turn takes the prompt");
    let again = h.engine.submit(&h.session.id, steer).await.unwrap();
    assert_eq!(first.message.id, again.message.id, "the same submission is one prompt");
    until_idle(&h).await;

    let requests = h.provider.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2, "taken at the next request, not as a turn of its own");
    let last = requests[1].messages.last().unwrap();
    assert!(
        matches!(last.blocks[0], llm::Block::ToolResult { .. }),
        "the call's result comes first"
    );
    assert!(matches!(last.blocks.last().unwrap(), llm::Block::Text(text) if text == "also check the logs"));
    assert_eq!(h.provider.responses_left(), 0);
}

#[tokio::test]
async fn a_prompt_sent_while_the_last_reply_streams_is_answered_before_the_turn_ends() {
    let h = harness().await;
    h.provider
        .push_slow(Duration::from_millis(600), text("first answer"))
        .push(text("second answer"));
    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(150)).await;
    h.engine.submit(&h.session.id, prompt("two")).await.expect("steered");
    until_idle(&h).await;

    let requests = h.provider.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        texts_sent(&requests[1]),
        ["one", "first answer", "two"],
        "ordered as sent"
    );
    assert_eq!(transcript(&h).last().unwrap().info.status, MessageStatus::Done);
}

#[tokio::test]
async fn a_prompt_sent_during_another_job_waits_and_then_runs() {
    let h = harness().await;
    assert!(h.engine.turns.claim(&h.session.id, &CancellationToken::new()));
    let engine = h.engine.clone();
    let id = h.session.id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        engine.turns.release(&id);
    });
    h.provider.push(text("after the job"));
    let started = std::time::Instant::now();
    h.engine
        .submit(&h.session.id, prompt("queued"))
        .await
        .expect("queued behind the job");
    assert!(started.elapsed() >= Duration::from_millis(250));
    until_idle(&h).await;
    assert_eq!(h.provider.responses_left(), 0);
}

#[tokio::test]
async fn a_model_named_mid_turn_powers_its_next_request_and_the_conversation_carries_on() {
    let h = harness().await;
    let other = h.engine.catalog.read().unwrap().providers["anthropic"]
        .models
        .keys()
        .find(|id| *id != "claude-sonnet-4-5")
        .unwrap()
        .clone();
    let requests = switched_mid_turn(
        &h,
        Prompt {
            model: Some(ModelRef {
                provider: "anthropic".into(),
                model: other.clone(),
            }),
            ..prompt("try the other one")
        },
    )
    .await;
    assert_eq!(
        requests
            .iter()
            .map(|request| request.model.as_str())
            .collect::<Vec<_>>(),
        ["claude-sonnet-4-5", other.as_str()],
        "one turn, the second request on the new model"
    );
    assert!(format!("{:?}", requests[1].messages).contains("try the other one"));
    let messages = transcript(&h);
    assert_eq!(
        messages
            .last()
            .unwrap()
            .info
            .model
            .as_ref()
            .map(|model| model.model.as_str()),
        Some(other.as_str()),
        "the reply records the model that wrote it"
    );

    let unknown = Prompt {
        model: Some(ModelRef {
            provider: "anthropic".into(),
            model: "no-such-model".into(),
        }),
        ..prompt("x")
    };
    h.provider.push_slow(Duration::from_millis(300), text("busy"));
    h.engine.submit(&h.session.id, prompt("again")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        h.engine.submit(&h.session.id, unknown).await.err(),
        Some(TurnError::UnknownModel),
        "a bad choice fails for the sender"
    );
    until_idle(&h).await;
}

#[tokio::test]
async fn an_agent_or_level_named_mid_turn_applies_from_the_next_request() {
    let h = harness().await;
    let requests = switched_mid_turn(
        &h,
        Prompt {
            agent: Some("plan".into()),
            ..prompt("plan instead")
        },
    )
    .await;
    let planning = |request: &Request| format!("{:?}", request.messages).contains("# Plan mode");
    assert!(
        !planning(&requests[0]) && planning(&requests[1]),
        "plan's reminder from the next request"
    );
    assert_eq!(transcript(&h).last().unwrap().info.agent.as_deref(), Some("plan"));

    let h = harness().await;
    let requests = switched_mid_turn(
        &h,
        Prompt {
            variant: Some(Some("max".into())),
            ..prompt("think harder")
        },
    )
    .await;
    assert_eq!(requests[0].reasoning, None);
    assert!(
        matches!(requests[1].reasoning, Some(Reasoning::Budget { .. })),
        "{:?}",
        requests[1].reasoning
    );
}

#[tokio::test]
async fn a_follow_up_naming_what_the_turn_already_runs_as_joins_it() {
    let h = harness().await;
    let with = |text: &str, agent: Option<&str>, variant: Option<Option<&str>>| Prompt {
        agent: agent.map(String::from),
        variant: variant.map(|variant| variant.map(String::from)),
        ..prompt(text)
    };
    let rounds = [
        (
            with("start", None, None),
            "its own agent, no level",
            Some("build"),
            Some(None),
        ),
        (
            with("start", None, Some(Some("max"))),
            "its own level",
            Some("build"),
            Some(Some("max")),
        ),
        (
            with("start", None, Some(None)),
            "a level this model lacks",
            None,
            Some(Some("ultra")),
        ),
    ];
    for (round, (first, follow_up, agent, variant)) in rounds.into_iter().enumerate() {
        h.provider
            .push_slow(
                Duration::from_millis(300),
                tool_call("read", r#"{"path": "missing.txt"}"#),
            )
            .push(text("done"));
        h.engine.submit(&h.session.id, first).await.await_ok();
        tokio::time::sleep(Duration::from_millis(100)).await;
        h.engine
            .submit(&h.session.id, with(follow_up, agent, variant))
            .await
            .await_ok();
        until_idle(&h).await;

        let requests = h.provider.requests.lock().unwrap();
        assert_eq!(
            requests.len(),
            2 * (round + 1),
            "{follow_up}: joined the running turn instead of ending it"
        );
        assert!(
            format!("{:?}", requests.last().unwrap().messages).contains(follow_up),
            "{follow_up}: answered in the same turn"
        );
    }
}
