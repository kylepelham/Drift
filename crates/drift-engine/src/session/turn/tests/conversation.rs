use super::*;

#[tokio::test]
async fn a_plain_reply_is_stored_and_costed() {
    let h = harness().await;
    h.provider.push(text("Hello there"));
    let receipt = h
        .engine
        .submit(&h.session.id, prompt("say hello please"))
        .await
        .await_ok();
    assert_eq!(receipt.message.role, Role::User);
    until_idle(&h).await;

    let messages = transcript(&h);
    assert_eq!(messages.len(), 2);
    let reply = &messages[1];
    assert_eq!(reply.info.status, MessageStatus::Done);
    assert_eq!(
        reply.info.usage,
        Usage {
            input: 10,
            output: 3,
            cache_read: 0,
            cache_write: 0
        }
    );
    assert!(reply.info.cost > 0.0);
    assert_eq!(
        reply.parts[0].part,
        Part::Text {
            text: "Hello there".into()
        }
    );
    assert_eq!(session(&h).model, Some(model()));
    let requests = h.provider.requests.lock().unwrap();
    assert!(requests[0].system.starts_with("You are Drift"));
    assert_eq!(requests[0].tools.len(), 14);
}

#[tokio::test]
async fn a_job_that_panics_still_releases_its_session() {
    let h = harness().await;
    assert!(h.engine.turns.claim(&h.session.id, &CancellationToken::new()));
    h.engine.spawn_job(&h.session.id, async { panic!("a bug in a job") });
    until_idle(&h).await;
    assert!(!h.engine.turns.is_running(&h.session.id));

    h.provider.push(text("still usable"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_idle(&h).await;
}

#[tokio::test]
async fn a_conversation_holding_parts_this_build_cannot_read_still_runs_and_never_sends_them() {
    let h = harness().await;
    h.provider.push(text("first answer")).push(text("second answer"));
    h.engine.submit(&h.session.id, prompt("first")).await.await_ok();
    until_idle(&h).await;
    for (index, message) in transcript(&h).iter().enumerate() {
        h.engine
            .store
            .lock()
            .execute(
                "INSERT INTO part(id, message_id, session_id, json) VALUES(?1, ?2, ?3, ?4)",
                rusqlite::params![
                    format!("prt_z{index}"),
                    message.info.id,
                    h.session.id,
                    r#"{"type":"snapshot","snapshot":"SECRET_HASH"}"#
                ],
            )
            .unwrap();
    }
    h.engine.submit(&h.session.id, prompt("second")).await.await_ok();
    until_idle(&h).await;

    let sent = texts_sent(&h.provider.requests.lock().unwrap()[1]);
    assert_eq!(
        sent,
        ["first", "first answer", "second"],
        "the rest of the history goes as before"
    );
    assert_eq!(transcript(&h).len(), 4);
}

#[tokio::test]
async fn every_step_of_a_conversation_carries_the_same_cache_key() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    h.provider
        .push(tool_call("read", r#"{"path": "a.txt"}"#))
        .push(text("read it"));
    h.engine.submit(&h.session.id, prompt("read a")).await.await_ok();
    until_idle(&h).await;

    let keys: Vec<_> = h
        .provider
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| request.cache_key.clone())
        .collect();
    assert_eq!(keys, [Some(h.session.id.clone()), Some(h.session.id.clone())]);
}

#[tokio::test]
async fn a_finished_reply_tells_the_ui_its_conversation_moved_up() {
    let h = harness().await;
    let mut events = h.engine.hub.attach(None).rx;
    h.provider.push(text("done"));
    h.engine.submit(&h.session.id, prompt("go")).await.await_ok();
    until_idle(&h).await;

    let (mut replying, mut moved) = (false, false);
    while let Ok(envelope) = events.try_recv() {
        match envelope.event {
            Event::MessageCreated { message } if message.role == Role::Assistant => replying = true,
            Event::SessionUpdated { .. } if replying => moved = true,
            _ => {}
        }
    }
    assert!(moved, "a session.updated follows the reply, not only the prompt");
}
