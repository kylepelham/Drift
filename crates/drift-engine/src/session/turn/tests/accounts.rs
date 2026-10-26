use super::*;
use crate::event::SwitchReason;
use crate::llm::credentials::Profile;
use crate::llm::limits::{Limits, Window};
use crate::session::types::Refusal;

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

/// Marks an account as in Anthropic's Cyber Verification Program, as the user does in Settings.
fn verify(h: &Harness, account: &str) {
    assert!(
        h.engine
            .credentials
            .set_cyber_verified("anthropic", account, true)
            .unwrap()
    );
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
                to, reason, position, ..
            } => switched = Some((to, reason, position)),
            Event::SessionRetry { .. } => retried = true,
            _ => {}
        }
    }
    assert_eq!(switched, Some((second, SwitchReason::Limit, 2)));
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
async fn switching_accounts_keeps_tool_history_without_promoting_reasoning_to_text() {
    let h = harness().await;
    two_accounts(&h);
    let mut call = vec![
        Chunk::Limits(Limits {
            spent_until: Some(id::now_ms() + 3_600_000),
            ..weekly(100.0)
        }),
        Chunk::ReasoningStart,
        Chunk::ReasoningDelta("I should review the remaining work before continuing.".into()),
        Chunk::ReasoningSignature("first-account-signature".into()),
        Chunk::BlockStop,
        Chunk::TextStart,
        Chunk::TextDelta("I'll update the task list.".into()),
        Chunk::BlockStop,
    ];
    call.extend(tool_call("todowrite", r#"{"todos":[]}"#));
    h.provider.push(call).push(text("The task list is up to date."));

    h.engine
        .submit(&h.session.id, prompt("Review the remaining work."))
        .await
        .await_ok();
    until_idle(&h).await;

    assert_eq!(sent_with(&h), ["first", "second"]);
    let requests = h.provider.requests.lock().unwrap();
    let blocks: Vec<_> = requests[1]
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .collect();
    assert!(
        blocks
            .iter()
            .all(|block| !matches!(block.unsigned(), llm::Block::Reasoning { .. }))
    );
    assert!(
        !texts_sent(&requests[1])
            .iter()
            .any(|text| text.contains("before continuing"))
    );
    assert!(texts_sent(&requests[1]).contains(&"I'll update the task list.".to_string()));
    assert!(blocks.iter().any(|block| matches!(block, llm::Block::ToolUse { .. })));
    assert!(
        blocks
            .iter()
            .any(|block| matches!(block, llm::Block::ToolResult { .. }))
    );

    assert!(transcript(&h).iter().flat_map(|message| &message.parts).any(|row| {
        matches!(&row.part, Part::Reasoning { signature: Some(signature), .. } if signature == "first-account-signature")
    }), "the stored reasoning is untouched");
}

#[tokio::test]
async fn the_first_account_is_used_again_once_its_usage_frees_and_the_cache_has_gone_cold() {
    let h = harness().await;
    let (first, _) = two_accounts(&h);
    h.engine.limits.refuse(&first, id::now_ms() + 3_600_000);
    h.provider.push(text("a")).push(text("b")).push(text("c"));
    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;

    // While the conversation's cache on the second account is warm, it stays there.
    h.engine.limits.refuse(&first, id::now_ms() - 1);
    h.engine.submit(&h.session.id, prompt("two")).await.await_ok();
    until_idle(&h).await;
    assert_eq!(sent_with(&h), ["second", "second"]);

    let mut last = h.engine.store.last_cached_reply(&h.session.id).unwrap().unwrap();
    last.finished_at = Some(id::now_ms() - 2 * 3_600_000);
    h.engine.store.save_message(&last).unwrap();
    h.engine.submit(&h.session.id, prompt("three")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(sent_with(&h), ["second", "second", "first"]);
}

#[tokio::test]
async fn a_subscription_conversation_caches_for_an_hour_until_it_draws_on_extra_usage() {
    let h = harness().await;
    two_accounts(&h);
    let mut reply = vec![Chunk::Limits(Limits {
        overage: true,
        ..weekly(50.0)
    })];
    reply.extend(tool_call("todowrite", r#"{"todos":[]}"#));
    h.provider.push(reply).push(text("done"));

    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;

    let hours: Vec<bool> = h
        .provider
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| request.cache_hour)
        .collect();
    assert_eq!(hours, [true, false]);
}

#[tokio::test]
async fn a_key_caches_for_an_hour_only_when_its_providers_option_says_so() {
    use crate::session::turn::provider_options::ProviderOptions;
    let h = harness().await;
    let hour = ProviderOptions { hour_cache: true };
    h.engine
        .set_provider_options([("openrouter".to_string(), hour.clone())].into())
        .unwrap();
    h.provider.push(text("one")).push(text("two"));

    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;
    h.engine
        .set_provider_options([("anthropic".to_string(), hour)].into())
        .unwrap();
    h.engine.submit(&h.session.id, prompt("two")).await.await_ok();
    until_idle(&h).await;

    let hours: Vec<bool> = h
        .provider
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| request.cache_hour)
        .collect();
    assert_eq!(hours, [false, true]);
    assert_eq!(h.engine.provider_options().len(), 2, "each provider keeps its own");
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

fn plan_lacks_model() -> llm::Error {
    llm::Error::api(429, "rate_limit_error", "Usage credits are required for this model.")
}

#[tokio::test]
async fn an_account_whose_plan_lacks_the_model_hands_the_request_to_one_that_has_it() {
    let h = harness().await;
    let (first, second) = two_accounts(&h);
    let mut events = h.engine.hub.attach(None).rx;
    h.provider
        .push_error(plan_lacks_model())
        .push(text("answered by bob"))
        .push(text("bob again"));

    // The refusal moves the request on at once, with no backoff and no failed reply left behind.
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

    // The first account is passed over for this model only; its usage is not marked spent.
    let model = model().model;
    assert!(h.engine.unserved.refused(&first, &model, id::now_ms()));
    assert!(h.engine.limits.spent(&first, id::now_ms()).is_none());

    // The next turn on the same model starts on the account that has it.
    h.engine.submit(&h.session.id, prompt("two")).await.await_ok();
    until_idle(&h).await;
    assert_eq!(sent_with(&h), ["first", "second", "second"]);

    let mut retried = false;
    let mut reason = None;
    while let Ok(envelope) = events.try_recv() {
        match envelope.event {
            Event::SessionRetry { .. } => retried = true,
            Event::ProviderSwitched { reason: why, .. } => reason = Some(why),
            _ => {}
        }
    }
    assert!(!retried, "a switch is not a retry");
    assert_eq!(
        reason,
        Some(SwitchReason::Model),
        "the switch says the plan lacks the model"
    );
}

#[tokio::test]
async fn when_no_account_has_the_model_the_refusal_is_shown_without_waiting() {
    let h = harness().await;
    two_accounts(&h);
    h.provider.push_error(plan_lacks_model()).push_error(plan_lacks_model());

    let started = std::time::Instant::now();
    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(sent_with(&h), ["first", "second"]);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "no backoff on a refusal waiting cannot fix"
    );
    let error = transcript(&h).last().unwrap().info.error.clone().unwrap();
    assert!(error.contains("Usage credits are required"), "{error}");
}

/// A reply the safety filter ended after some text, with the policy area the provider named.
fn refused(category: Option<&str>) -> Vec<Chunk> {
    vec![
        Chunk::TextStart,
        Chunk::TextDelta("Sure, here is".into()),
        Chunk::BlockStop,
        Chunk::Refusal(Refusal {
            category: category.map(String::from),
            explanation: Some("This request triggered restrictions on violative cyber content.".into()),
            model: None,
        }),
        Chunk::Stop(StopReason::Refused),
    ]
}

#[tokio::test]
async fn a_cyber_refusal_sends_the_request_again_from_a_verified_account_and_the_conversation_stays_there() {
    let h = harness().await;
    let (first, second) = two_accounts(&h);
    verify(&h, &second);
    let mut events = h.engine.hub.attach(None).rx;
    h.provider
        .push(refused(Some("cyber")))
        .push(text("answered by bob"))
        .push(text("bob again"));

    // The refused reply and its partial text go, and the next account answers at once.
    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(sent_with(&h), ["first", "second"]);
    let messages = transcript(&h);
    assert_eq!(messages.len(), 2, "the refused reply leaves nothing behind");
    assert!(
        messages
            .iter()
            .all(|message| message.info.error.is_none() && message.info.refusal.is_none())
    );
    assert_eq!(messages[1].info.account.as_deref(), Some(second.as_str()));

    // The first account still serves other conversations, but not this one.
    assert!(h.engine.unserved.refused_conversation(&first, &h.session.id));
    assert!(h.engine.usable_account(&first));
    h.engine.submit(&h.session.id, prompt("two")).await.await_ok();
    until_idle(&h).await;
    assert_eq!(sent_with(&h), ["first", "second", "second"]);

    let mut reason = None;
    while let Ok(envelope) = events.try_recv() {
        if let Event::ProviderSwitched { reason: why, .. } = envelope.event {
            reason = Some(why);
        }
    }
    assert_eq!(reason, Some(SwitchReason::Refusal));
}

#[tokio::test]
async fn a_cyber_fallback_sets_only_that_chat_to_the_verified_account() {
    let h = harness().await;
    let (_, second) = two_accounts(&h);
    verify(&h, &second);
    let mut events = h.engine.hub.attach(None).rx;
    h.provider
        .push(refused(Some("cyber")))
        .push(text("answered by bob"))
        .push(text("ann for the other chat"));

    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;
    let other = sibling_session(&h, "other");
    h.engine.submit(&other.id, prompt("two")).await.await_ok();
    until_session_idle(&h, &other.id).await;

    // Kept as the chat's own account, so it outlives the engine's memory of the refusal and is shown by /account.
    let chat = h.engine.store.session(&h.session.id).unwrap().unwrap();
    assert_eq!(chat.account.as_deref(), Some(second.as_str()));
    assert_eq!(h.engine.store.session(&other.id).unwrap().unwrap().account, None);
    assert_eq!(
        sent_with(&h),
        ["first", "second", "first"],
        "the other chat keeps the order"
    );
    assert_eq!(
        switched(&mut events),
        [SwitchReason::Refusal],
        "one notice, for the chat that moved"
    );
}

#[tokio::test]
async fn a_refusal_that_names_no_policy_area_also_tries_a_verified_account() {
    let h = harness().await;
    let (_, second) = two_accounts(&h);
    verify(&h, &second);
    h.provider.push(refused(None)).push(text("answered by bob"));

    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(sent_with(&h), ["first", "second"]);
    assert!(transcript(&h).iter().all(|message| message.info.error.is_none()));
}

#[tokio::test]
async fn a_cyber_refusal_stands_when_no_other_account_is_verified_and_the_conversation_keeps_its_account() {
    let h = harness().await;
    let (first, _) = two_accounts(&h);
    h.provider.push(refused(Some("cyber"))).push(text("answered by ann"));

    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(
        sent_with(&h),
        ["first"],
        "an unverified account's filter would refuse it the same way"
    );
    let shown = transcript(&h).last().unwrap().info.clone();
    assert_eq!(shown.ending, Some(crate::session::types::Ending::Refused));
    assert!(!h.engine.unserved.refused_conversation(&first, &h.session.id));

    h.engine.submit(&h.session.id, prompt("two")).await.await_ok();
    until_idle(&h).await;
    assert_eq!(sent_with(&h), ["first", "first"]);
}

#[tokio::test]
async fn a_cyber_refusal_stands_when_the_verified_account_has_no_usage_left() {
    let h = harness().await;
    let (first, second) = two_accounts(&h);
    verify(&h, &second);
    h.engine.limits.refuse(&second, id::now_ms() + 3_600_000);
    h.provider.push(refused(Some("cyber"))).push(text("never sent"));

    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(
        sent_with(&h),
        ["first"],
        "a spent account would only answer that it is spent"
    );
    let shown = transcript(&h).last().unwrap().info.clone();
    assert_eq!(shown.ending, Some(crate::session::types::Ending::Refused));
    assert!(!h.engine.unserved.refused_conversation(&first, &h.session.id));
}

#[tokio::test]
async fn a_cyber_refusal_passes_over_unverified_accounts_for_good() {
    let h = harness().await;
    let (first, _) = two_accounts(&h);
    let third = h
        .engine
        .credentials
        .add_account(
            "anthropic",
            &Credential::OAuth {
                access: "third".into(),
                refresh: "third-refresh".into(),
                expires_at: id::now_ms() + 3_600_000,
                account: None,
            },
            &Profile {
                identity: Some("cat".into()),
                email: Some("cat@example.com".into()),
            },
        )
        .unwrap();
    verify(&h, &third);
    h.provider
        .push(refused(Some("cyber")))
        .push(text("answered by cat"))
        .push(text("cat again"));

    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;
    assert_eq!(sent_with(&h), ["first", "third"]);

    // Even with its cache gone cold, the conversation never moves to the unverified account.
    let mut last = h.engine.store.last_cached_reply(&h.session.id).unwrap().unwrap();
    last.finished_at = Some(id::now_ms() - 2 * 3_600_000);
    h.engine.store.save_message(&last).unwrap();
    h.engine.submit(&h.session.id, prompt("two")).await.await_ok();
    until_idle(&h).await;
    assert_eq!(sent_with(&h), ["first", "third", "third"]);
    assert!(h.engine.unserved.refused_conversation(&first, &h.session.id));
}

#[tokio::test]
async fn a_refusal_in_another_policy_area_stands_on_the_account_that_made_it() {
    let h = harness().await;
    two_accounts(&h);
    h.provider.push(refused(Some("bio"))).push(text("never sent"));

    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(sent_with(&h), ["first"]);
    let refused = transcript(&h).last().unwrap().info.clone();
    assert_eq!(refused.ending, Some(crate::session::types::Ending::Refused));
    assert_eq!(refused.refusal.unwrap().category.as_deref(), Some("bio"));
}

#[tokio::test]
async fn when_every_account_refuses_the_last_refusal_is_shown() {
    let h = harness().await;
    let (_, second) = two_accounts(&h);
    verify(&h, &second);
    h.provider.push(refused(Some("cyber"))).push(refused(Some("cyber")));

    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(sent_with(&h), ["first", "second"]);
    let messages = transcript(&h);
    assert_eq!(messages.len(), 2, "one refused reply stays: the last account's");
    let shown = messages[1].info.clone();
    assert_eq!(shown.ending, Some(crate::session::types::Ending::Refused));
    assert_eq!(shown.refusal.unwrap().category.as_deref(), Some("cyber"));
}

#[tokio::test]
async fn a_cyber_refusal_of_a_compaction_summary_asks_a_verified_account_and_the_conversation_follows() {
    let h = harness().await;
    let (first, second) = two_accounts(&h);
    verify(&h, &second);
    let mut events = h.engine.hub.attach(None).rx;
    h.provider
        .push(text("The retry loop now backs off."))
        .push(refused(Some("cyber")))
        .push(text("We reworked the retry loop to back off."))
        .push(text("Done, the backoff is capped at a minute."));
    h.engine
        .submit(&h.session.id, prompt("Make the retry loop back off"))
        .await
        .await_ok();
    until_idle(&h).await;

    // The summary comes from the next account instead of failing on the refusing one.
    h.engine
        .compact(
            &h.session.id,
            crate::session::compaction::Trigger::Manual,
            &Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(sent_with(&h), ["first", "first", "second"]);
    assert!(h.engine.unserved.refused_conversation(&first, &h.session.id));
    assert!(transcript(&h).iter().all(|message| message.info.error.is_none()));

    // The conversation's next turn moves there too, and says why.
    h.engine
        .submit(&h.session.id, prompt("Cap it at a minute"))
        .await
        .await_ok();
    until_idle(&h).await;
    assert_eq!(sent_with(&h), ["first", "first", "second", "second"]);
    assert_eq!(
        transcript(&h).last().unwrap().info.account.as_deref(),
        Some(second.as_str())
    );

    let mut reason = None;
    while let Ok(envelope) = events.try_recv() {
        if let Event::ProviderSwitched { reason: why, .. } = envelope.event {
            reason = Some(why);
        }
    }
    assert_eq!(reason, Some(SwitchReason::Refusal));
}

#[tokio::test]
async fn a_compaction_summary_refused_in_another_policy_area_fails_on_the_account_that_refused_it() {
    let h = harness().await;
    two_accounts(&h);
    h.provider
        .push(text("The retry loop now backs off."))
        .push(refused(Some("bio")))
        .push(refused(Some("bio")));
    h.engine
        .submit(&h.session.id, prompt("Make the retry loop back off"))
        .await
        .await_ok();
    until_idle(&h).await;

    let compacted = h
        .engine
        .compact(
            &h.session.id,
            crate::session::compaction::Trigger::Manual,
            &Default::default(),
        )
        .await;

    assert!(compacted.is_err());
    assert!(
        sent_with(&h).iter().all(|account| account == "first"),
        "{:?}",
        sent_with(&h)
    );
}

fn pin(h: &Harness, session: &str, account: Option<&str>) {
    h.engine.store.set_session_account(session, account).unwrap().unwrap();
}

fn switched(events: &mut tokio::sync::broadcast::Receiver<crate::event::Envelope>) -> Vec<SwitchReason> {
    std::iter::from_fn(|| events.try_recv().ok())
        .filter_map(|envelope| match envelope.event {
            Event::ProviderSwitched { reason, .. } => Some(reason),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_chat_set_to_an_account_uses_it_quietly_while_the_others_keep_the_order() {
    let h = harness().await;
    let (_, second) = two_accounts(&h);
    pin(&h, &h.session.id, Some(&second));
    let mut events = h.engine.hub.attach(None).rx;
    h.provider.push(text("a")).push(text("b"));

    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;
    let other = sibling_session(&h, "other");
    h.engine.submit(&other.id, prompt("two")).await.await_ok();
    until_session_idle(&h, &other.id).await;

    assert_eq!(sent_with(&h), ["second", "first"]);
    assert!(
        switched(&mut events).is_empty(),
        "the user's own choice needs no notice"
    );
}

#[tokio::test]
async fn a_set_account_with_no_usage_left_hands_over_to_the_order_and_is_returned_to_when_it_frees() {
    let h = harness().await;
    let (first, second) = two_accounts(&h);
    h.provider
        .push(text("a"))
        .push_error(spent())
        .push(text("b"))
        .push(text("c"));
    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;

    // Set during the conversation, it is moved to even though the cache on the first account is warm.
    pin(&h, &h.session.id, Some(&second));
    let mut events = h.engine.hub.attach(None).rx;
    h.engine.submit(&h.session.id, prompt("two")).await.await_ok();
    until_idle(&h).await;
    assert_eq!(sent_with(&h), ["first", "second", "first"]);
    assert_eq!(switched(&mut events), [SwitchReason::Limit]);

    h.engine.limits.refuse(&second, id::now_ms() - 1);
    h.engine.submit(&h.session.id, prompt("three")).await.await_ok();
    until_idle(&h).await;
    assert_eq!(sent_with(&h), ["first", "second", "first", "second"]);
    assert!(h.engine.limits.spent(&first, id::now_ms()).is_none());
}

#[tokio::test]
async fn an_account_of_another_provider_or_one_signed_out_leaves_the_order_to_decide() {
    let h = harness().await;
    let (first, second) = two_accounts(&h);
    pin(&h, &h.session.id, Some("openai~gone"));
    h.provider.push(text("a")).push(text("b"));
    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;

    pin(&h, &h.session.id, Some(&second));
    assert!(h.engine.credentials.remove_account("anthropic", &second).unwrap());
    h.engine.submit(&h.session.id, prompt("two")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(sent_with(&h), ["first", "first"]);
    assert_eq!(transcript(&h)[3].info.account.as_deref(), Some(first.as_str()));
}
