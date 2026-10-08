use super::*;

#[tokio::test]
async fn steering_uses_the_admitted_agent_and_model_generation() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "a").unwrap();
    h.provider
        .push_slow(Duration::from_millis(500), tool_call("read", r#"{"path":"a.txt"}"#))
        .push(text("done"));
    h.engine.submit(&h.session.id, prompt("start")).await.await_ok();
    std::fs::create_dir_all(h._dir.join("ws/.drift/agents")).unwrap();
    std::fs::write(
        h._dir.join("ws/.drift/agents/late.md"),
        "---\ndescription: Added mid-turn\n---\nA later agent.",
    )
    .unwrap();
    let late = Prompt {
        agent: Some("late".into()),
        ..prompt("late selection")
    };
    assert!(matches!(
        h.engine.submit(&h.session.id, late.clone()).await,
        Err(TurnError::UnknownAgent)
    ));

    let mut new_model = h
        .engine
        .catalog
        .read()
        .unwrap()
        .model("anthropic", "claude-sonnet-4-5")
        .unwrap()
        .clone();
    new_model.id = "new-mid-turn".into();
    h.engine
        .catalog
        .write()
        .unwrap()
        .providers
        .get_mut("anthropic")
        .unwrap()
        .models
        .insert(new_model.id.clone(), new_model);
    let model = ModelRef {
        provider: "anthropic".into(),
        model: "new-mid-turn".into(),
    };
    assert!(matches!(
        h.engine
            .submit(
                &h.session.id,
                Prompt {
                    model: Some(model),
                    ..prompt("new model")
                }
            )
            .await,
        Err(TurnError::UnknownModel)
    ));
    until_idle(&h).await;

    h.provider.push(text("later turn"));
    h.engine.submit(&h.session.id, late).await.await_ok();
    until_idle(&h).await;
    assert_eq!(session(&h).agent, "late");
}

#[tokio::test]
async fn submit_rejects_bad_plans() {
    let h = harness().await;
    let no_model = Prompt {
        parts: vec![],
        model: None,
        variant: None,
        agent: None,
        submission_id: None,
    };
    assert_eq!(
        h.engine.submit(&h.session.id, no_model).await.err(),
        Some(TurnError::NoModel)
    );
    let unknown = Prompt {
        model: Some(ModelRef {
            provider: "anthropic".into(),
            model: "nope".into(),
        }),
        ..prompt("x")
    };
    assert_eq!(
        h.engine.submit(&h.session.id, unknown).await.err(),
        Some(TurnError::UnknownModel)
    );
    assert_eq!(
        h.engine.submit("ses_missing", prompt("x")).await.err(),
        Some(TurnError::NoSession)
    );
    h.engine.credentials.remove("anthropic").unwrap();
    assert_eq!(
        h.engine.submit(&h.session.id, prompt("x")).await.err(),
        Some(TurnError::NoCredentials)
    );
}

#[tokio::test]
async fn failed_admission_releases_the_session_and_submission_ids_replay() {
    let h = harness().await;
    h.provider.push(text("ok")).push(text("again"));
    let mut first = prompt("hello");
    first.submission_id = Some("sub_1".into());
    let receipt = h.engine.submit(&h.session.id, first.clone()).await.await_ok();
    let replay = h.engine.submit(&h.session.id, first).await.await_ok();
    assert_eq!(
        replay.message.id, receipt.message.id,
        "same submission id returns the same receipt"
    );
    until_idle(&h).await;
    assert_eq!(transcript(&h).len(), 2);

    let other = sibling_session(&h, "");
    let mut reused = prompt("x");
    reused.submission_id = Some("sub_1".into());
    assert_eq!(
        h.engine.submit(&other.id, reused).await.err(),
        Some(TurnError::SubmissionReused)
    );

    let doomed = sibling_session(&h, "");
    h.engine
        .store
        .lock()
        .execute(
            "CREATE TRIGGER block BEFORE INSERT ON part BEGIN SELECT RAISE(ABORT, 'no parts'); END",
            [],
        )
        .unwrap();
    let failed = h.engine.submit(&doomed.id, prompt("boom")).await;
    assert!(matches!(failed, Err(TurnError::Store(_))), "{failed:?}");
    h.engine.store.lock().execute("DROP TRIGGER block", []).unwrap();
    assert!(
        !h.engine.turns.is_running(&doomed.id),
        "a failed admission must not leave the session busy"
    );
    assert!(
        h.engine.store.transcript(&doomed.id).unwrap().is_empty(),
        "no half-written prompt"
    );
    h.provider.push(text("fine"));
    h.engine.submit(&doomed.id, prompt("retry")).await.await_ok();
    until_idle(&h).await;
}

#[tokio::test]
async fn submission_ids_survive_a_restart_and_reject_a_different_payload() {
    let h = harness().await;
    h.provider.push(text("ok"));
    let mut first = prompt("hello");
    first.submission_id = Some("sub_durable".into());
    let receipt = h.engine.submit(&h.session.id, first.clone()).await.await_ok();
    until_idle(&h).await;

    let reopened = reopen(&h);
    let replay = reopened.submit(&h.session.id, first).await.await_ok();
    assert_eq!(replay.message.id, receipt.message.id);
    assert_eq!(
        reopened.store.transcript(&h.session.id).unwrap().len(),
        2,
        "no second prompt after restart"
    );
    let mut changed = prompt("different text");
    changed.submission_id = Some("sub_durable".into());
    assert_eq!(
        reopened.submit(&h.session.id, changed).await.err(),
        Some(TurnError::SubmissionReused)
    );
}

#[test]
fn a_variant_left_unnamed_and_one_cleared_hash_apart() {
    let unnamed = prompt("x");
    let cleared = Prompt {
        variant: Some(None),
        ..prompt("x")
    };
    assert_ne!(payload_hash(&unnamed), payload_hash(&cleared));
}
