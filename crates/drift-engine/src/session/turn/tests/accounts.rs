use super::*;
use crate::llm::credentials::Profile;
use crate::llm::limits::{Limits, Window};

/// Two Claude sign-ins in use order; returns their account keys.
fn two_accounts(h: &Harness) -> (String, String) {
    let sign_in = |access: &str| Credential::OAuth {
        access: access.into(),
        refresh: format!("{access}-refresh"),
        expires_at: id::now_ms() + 3_600_000,
        account: None,
    };
    let person = |name: &str| Profile {
        identity: Some(name.into()),
        email: Some(format!("{name}@example.com")),
    };

    let credentials = &h.engine.credentials;
    let first = credentials
        .add_account("anthropic", &sign_in("first"), &person("ann"))
        .unwrap();
    let second = credentials
        .add_account("anthropic", &sign_in("second"), &person("bob"))
        .unwrap();

    (first, second)
}

fn weekly(used_percent: f64) -> Limits {
    Limits {
        weekly: Some(Window {
            used_percent,
            resets_at: Some(id::now_ms() + 3_600_000),
        }),
        ..Limits::default()
    }
}

#[tokio::test]
async fn the_limits_a_response_reports_are_kept_for_the_account_that_sent_it() {
    let h = harness().await;
    let (first, second) = two_accounts(&h);
    let mut reply = vec![Chunk::Limits(weekly(39.0))];
    reply.extend(text("a"));
    h.provider.push(reply);

    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;

    let reported = h.engine.limits.get(&first).unwrap();
    assert_eq!(reported.weekly.unwrap().used_percent, 39.0);
    assert!(
        h.engine.limits.get(&second).is_none(),
        "the idle account reported nothing"
    );
    assert!(transcript(&h).iter().all(|message| message.info.error.is_none()));
}

/// The access token each request was sent with, in order.
fn sent_with(h: &Harness) -> Vec<String> {
    h.provider
        .credentials
        .lock()
        .unwrap()
        .iter()
        .map(|credential| match credential {
            Credential::OAuth { access, .. } => access.clone(),
            other => format!("{other:?}"),
        })
        .collect()
}

fn spent() -> llm::Error {
    llm::Error::Api {
        status: 429,
        kind: crate::llm::limits::LIMIT_REACHED.into(),
        message: "The usage limit has been reached".into(),
        retryable: true,
        retry_after: Some(Duration::from_secs(3600)),
    }
}

#[tokio::test]
async fn a_spent_account_hands_the_request_to_the_next_at_once() {
    let h = harness().await;
    let (first, second) = two_accounts(&h);
    let mut events = h.engine.hub.attach(None).rx;
    h.provider.push_error(spent()).push(text("answered by bob"));

    let started = std::time::Instant::now();
    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(sent_with(&h), ["first", "second"]);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "no backoff before the next account"
    );
    let messages = transcript(&h);
    assert_eq!(messages.len(), 2, "the refused reply leaves nothing behind");
    assert!(messages.iter().all(|message| message.info.error.is_none()));
    assert_eq!(messages[1].info.account.as_deref(), Some(second.as_str()));
    assert!(h.engine.limits.spent(&first, id::now_ms()).is_some());

    let mut switched = None;
    let mut retried = false;
    while let Ok(envelope) = events.try_recv() {
        match envelope.event {
            Event::ProviderSwitched {
                to, limited, position, ..
            } => switched = Some((to, limited, position)),
            Event::SessionRetry { .. } => retried = true,
            _ => {}
        }
    }
    assert_eq!(switched, Some((second, true, 2)));
    assert!(!retried, "a switch is not a retry");
}

#[tokio::test]
async fn later_turns_start_on_the_account_with_usage_left() {
    let h = harness().await;
    let (first, _) = two_accounts(&h);
    h.engine.limits.refuse(&first, id::now_ms() + 3_600_000);
    h.provider.push(text("a"));

    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(sent_with(&h), ["second"]);
}

#[tokio::test]
async fn a_window_reported_full_moves_the_next_step_before_credits_are_spent() {
    let h = harness().await;
    let (first, _) = two_accounts(&h);
    let full = Limits {
        spent_until: Some(id::now_ms() + 3_600_000),
        ..weekly(100.0)
    };
    let mut call = vec![Chunk::Limits(full)];
    call.extend(tool_call("todowrite", r#"{"todos":[]}"#));
    h.provider.push(call).push(text("done"));

    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(sent_with(&h), ["first", "second"]);
    assert!(h.engine.limits.spent(&first, id::now_ms()).is_some());
}

#[tokio::test]
async fn the_first_account_is_used_again_once_its_usage_frees() {
    let h = harness().await;
    let (first, _) = two_accounts(&h);
    h.engine.limits.refuse(&first, id::now_ms() + 3_600_000);
    h.provider.push(text("a")).push(text("b"));
    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;

    h.engine.limits.refuse(&first, id::now_ms() - 1);
    h.engine.submit(&h.session.id, prompt("two")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(sent_with(&h), ["second", "first"]);
}

#[tokio::test]
async fn with_every_account_spent_the_refusal_is_shown() {
    let h = harness().await;
    two_accounts(&h);
    h.provider.push_error(spent()).push_error(spent());

    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(sent_with(&h), ["first", "second"]);
    let error = transcript(&h).last().unwrap().info.error.clone().unwrap();
    assert!(error.contains("usage limit"), "{error}");
}

#[tokio::test]
async fn an_ordinary_rate_limit_does_not_switch_accounts() {
    let h = harness().await;
    two_accounts(&h);
    h.provider
        .push_error(llm::Error::api(429, "rate_limit_error", "slow down").with_headers(&{
            let mut headers = ::http::HeaderMap::new();
            headers.insert("retry-after-ms", "1".parse().unwrap());
            headers
        }))
        .push(text("a"));

    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(sent_with(&h), ["first", "first"]);
}
