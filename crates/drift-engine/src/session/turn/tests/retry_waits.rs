use super::*;

#[tokio::test]
async fn a_retry_wait_is_announced_and_ends_with_running_again() {
    let h = harness().await;
    let mut events = h.engine.hub.attach(None).rx;
    h.provider.push_error(overloaded()).push(text("second time lucky"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();

    let (attempt, message, next_at) = loop {
        let envelope = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .unwrap()
            .unwrap();
        if let Event::SessionRetry {
            attempt,
            message,
            next_at,
            ..
        } = envelope.event
        {
            break (attempt, message, next_at);
        }
    };

    assert_eq!(attempt, 1);
    assert!(message.contains("busy"), "{message}");
    let wait = next_at - id::now_ms();
    assert!(
        (29_000..=30_000).contains(&wait),
        "the provider's own wait is used: {wait}ms"
    );
    assert!(h.engine.switch_retry_model(&h.session.id, &model(), None).await.is_ok());
    let running_again = loop {
        let envelope = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .unwrap()
            .unwrap();
        if let Event::SessionStatusChanged { status, .. } = envelope.event {
            break status;
        }
    };
    assert_eq!(running_again, SessionStatus::Running);
    until_idle(&h).await;
}

#[tokio::test]
async fn a_turn_waiting_to_retry_can_be_moved_to_another_model_and_keeps_it() {
    let h = harness().await;
    let pinned = h.engine.catalog.read().unwrap().providers["anthropic"]
        .models
        .keys()
        .find(|id| id.as_str() != "claude-sonnet-4-5")
        .unwrap()
        .clone();
    let other = ModelRef {
        provider: "anthropic".into(),
        model: pinned.clone(),
    };
    assert_eq!(
        h.engine.switch_retry_model(&h.session.id, &other, None).await,
        Err(TurnError::NotRetrying),
        "nothing is waiting yet"
    );

    h.provider
        .push_error(overloaded())
        .push(text("answered by the other model"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_waiting_to_retry(&h).await;

    let unusable = ModelRef {
        provider: "openai".into(),
        model: "gpt-5".into(),
    };
    assert_eq!(
        h.engine.switch_retry_model(&h.session.id, &unusable, None).await,
        Err(TurnError::NoCredentials),
        "a model without a credential is refused up front"
    );

    let started = std::time::Instant::now();
    h.engine
        .switch_retry_model(&h.session.id, &other, Some(Some("high".into())))
        .await
        .unwrap();
    until_idle(&h).await;

    assert!(
        started.elapsed() < Duration::from_millis(900),
        "the switch retries at once instead of waiting out the backoff"
    );
    let requests = h.provider.requests.lock().unwrap().clone();
    assert_eq!(
        (requests[0].model.as_str(), requests[1].model.as_str()),
        ("claude-sonnet-4-5", pinned.as_str())
    );
    let kept = session(&h);
    assert_eq!(
        kept.model,
        Some(other),
        "the session keeps the model it was switched to"
    );
    assert_eq!(kept.variant.as_deref(), Some("high"), "and the variant chosen with it");
}

#[tokio::test]
async fn stop_ends_a_retry_wait_at_once() {
    let h = harness().await;
    h.provider.push_error(overloaded());
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_waiting_to_retry(&h).await;

    let started = std::time::Instant::now();
    assert!(h.engine.abort(&h.session.id));
    until_idle(&h).await;

    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(
        h.provider.requests.lock().unwrap().len(),
        1,
        "no attempt after the stop"
    );
}

#[tokio::test]
async fn retryable_provider_errors_are_retried_and_others_are_not() {
    let h = harness().await;
    h.provider
        .push_error(llm::Error::api(529, "overloaded_error", "busy"))
        .push(text("second time lucky"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1].info.status, MessageStatus::Error);
    assert_eq!(messages[2].info.status, MessageStatus::Done);

    h.provider
        .push_error(llm::Error::Unauthenticated("invalid x-api-key".into()));
    h.engine.submit(&h.session.id, prompt("again")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    assert_eq!(messages.len(), 5, "a refused key is not retried");
    assert_eq!(messages[4].info.status, MessageStatus::Error);
    assert_eq!(
        messages[4].info.error.as_deref(),
        Some("the provider refused the credentials: invalid x-api-key")
    );
}

#[tokio::test]
async fn stop_ends_a_request_still_waiting_for_its_response() {
    let h = harness().await;
    let url = crate::llm::tests::silent_server().await;
    *h.engine.turns.provider_override.lock().unwrap() = Some(Provider::Anthropic(llm::anthropic::Anthropic::new(&url)));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let started = std::time::Instant::now();
    assert!(h.engine.abort(&h.session.id));
    until_idle(&h).await;

    assert!(
        started.elapsed() < Duration::from_secs(1),
        "no wait for the 120 s header limit"
    );
    assert_eq!(transcript(&h).last().unwrap().info.status, MessageStatus::Aborted);
}

#[tokio::test]
async fn an_overload_inside_the_stream_is_retried() {
    let h = harness().await;
    let partial = vec![Chunk::TextStart, Chunk::TextDelta("Let me".into())];
    h.provider
        .push_fail_midway(
            partial,
            llm::Error::api(llm::STREAMED, "overloaded_error", "Overloaded"),
        )
        .push(text("answered"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    assert_eq!(messages.len(), 3, "the stream's overload was retried");
    assert_eq!(messages[1].info.status, MessageStatus::Error);
    assert!(messages[1].info.error.as_deref().unwrap().contains("overloaded_error"));
    assert_eq!(messages[2].info.status, MessageStatus::Done);
}

#[tokio::test]
async fn a_provider_asking_for_too_long_a_wait_is_not_waited_on() {
    let h = harness().await;
    h.provider
        .push_error(asking_to_wait(MAX_REQUESTED_WAIT + Duration::from_secs(1)))
        .push(text("never asked"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(h.provider.requests.lock().unwrap().len(), 1, "no retry");
    assert_eq!(
        transcript(&h).last().unwrap().info.status,
        MessageStatus::Error,
        "the error stands for the user to see"
    );
}

#[tokio::test]
async fn a_spent_quota_is_one_request_and_an_endless_wait_releases_the_session() {
    let h = harness().await;
    h.provider.push_error(llm::Error::api(
        429,
        "insufficient_quota",
        "You exceeded your current quota",
    ));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(
        h.provider.requests.lock().unwrap().len(),
        1,
        "a spent quota is not retried"
    );

    h.provider.push_error(asking_to_wait(Duration::MAX));
    h.engine.submit(&h.session.id, prompt("again")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(
        h.provider.requests.lock().unwrap().len(),
        2,
        "no retry, and the session is free"
    );
    assert!(!h.engine.turns.is_running(&h.session.id));
}

#[test]
fn backoff_doubles_with_jitter_under_a_cap_and_a_named_wait_is_used_as_is() {
    let unnamed = Retry {
        message: String::new(),
        after: None,
    };
    for attempt in 1..=MAX_RETRIES {
        let delay = unnamed.delay(attempt);
        let nominal = RETRY_BASE.saturating_mul(1 << (attempt - 1));
        assert!(
            delay >= nominal.mul_f64(0.79).min(MAX_BACKOFF) && delay <= nominal.mul_f64(1.21).min(MAX_BACKOFF),
            "attempt {attempt}: {delay:?}"
        );
    }
    for _ in 0..50 {
        assert!(
            unnamed.delay(40) <= MAX_BACKOFF,
            "the cap holds after jitter, however many attempts"
        );
    }

    let named = Retry {
        message: String::new(),
        after: Some(Duration::from_millis(1500)),
    };
    assert_eq!(named.delay(5), Duration::from_millis(1500));
    assert!(named.allowed(MAX_RETRIES - 1) && !named.allowed(MAX_RETRIES));
}
