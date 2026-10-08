use super::*;

#[tokio::test]
async fn a_prompts_variant_sets_the_requests_reasoning_and_an_unknown_one_asks_nothing() {
    let h = harness().await;
    h.provider.push(text("thought hard")).push(text("plain"));
    let mut hard = prompt("think");
    hard.variant = Some(Some("max".into()));
    h.engine.submit(&h.session.id, hard).await.await_ok();
    until_idle(&h).await;
    let mut odd = prompt("again");
    odd.variant = Some(Some("ultra".into()));
    h.engine.submit(&h.session.id, odd).await.await_ok();
    until_idle(&h).await;

    let requests = h.provider.requests.lock().unwrap();
    assert!(
        matches!(requests[0].reasoning, Some(Reasoning::Budget { tokens }) if tokens > 16_000),
        "{:?}",
        requests[0].reasoning
    );
    assert_eq!(requests[1].reasoning, None, "a name the model does not offer");
}

#[tokio::test]
async fn the_session_keeps_its_variant_for_prompts_that_name_none_until_one_clears_it() {
    let h = harness().await;
    h.provider
        .push(text("one"))
        .push(text("two"))
        .push(text("three"))
        .push(text("four"));
    let with = |text: &str, variant: Option<Option<&str>>| Prompt {
        variant: variant.map(|variant| variant.map(String::from)),
        ..prompt(text)
    };
    for prompt in [
        with("set", Some(Some("max"))),
        with("inherit", None),
        with("clear", Some(None)),
        with("after", None),
    ] {
        h.engine.submit(&h.session.id, prompt).await.await_ok();
        until_idle(&h).await;
    }

    let thought: Vec<_> = h
        .provider
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| matches!(request.reasoning, Some(Reasoning::Budget { .. })))
        .collect();
    assert_eq!(
        thought,
        [true, true, false, false],
        "a prompt that names none, as the engine's own do, runs at the session's"
    );
    assert_eq!(session(&h).variant, None);
}

#[test]
fn output_and_thinking_budgets_are_valid_together() {
    let budget = |tokens| Some(Reasoning::Budget { tokens });
    assert_eq!(
        budgets(&model_with(32_000, true), budget(32_000)),
        (32_000, budget(32_000 - MIN_ANSWER_TOKENS)),
        "never past the model's own limit"
    );
    assert_eq!(
        budgets(&model_with(64_000, true), budget(32_000)),
        (33_024, budget(32_000)),
        "a budget may raise the output past our cap"
    );
    assert_eq!(
        budgets(&model_with(64_000, true), budget(100_000)),
        (64_000, budget(64_000 - MIN_ANSWER_TOKENS))
    );
    assert_eq!(
        budgets(&model_with(64_000, true), budget(10)),
        (32_000, budget(MIN_THINKING_TOKENS)),
        "raised to the provider's minimum"
    );
    assert_eq!(
        budgets(&model_with(1_500, true), budget(8_000)),
        (1_500, None),
        "no room for thinking and an answer"
    );
    assert_eq!(
        budgets(&model_with(64_000, false), budget(8_000)),
        (32_000, None),
        "a model that does not reason gets no budget"
    );
    assert_eq!(
        budgets(&model_with(0, false), None),
        (MAX_OUTPUT_TOKENS, None),
        "an unknown limit and window use our cap"
    );

    let mut local = model_with(0, false);
    local.limit.context = 4_096;
    assert_eq!(
        budgets(&local, None),
        (1_024, None),
        "an unknown limit asks for a quarter of a known window"
    );
    local.limit.context = 2_048;
    assert_eq!(
        budgets(&local, None).0,
        MIN_ANSWER_TOKENS,
        "never less than room for an answer"
    );
    let mut whole = model_with(32_768, false);
    whole.limit.context = 32_768;
    assert_eq!(
        whole.reply_room(),
        16_384,
        "an output limit as large as the window gets half of it"
    );
    assert_eq!(budgets(&whole, None).0, 16_384, "and asks for no more than that");
    let effort = Some(Reasoning::Effort { level: "high".into() });
    assert_eq!(
        budgets(&model_with(64_000, true), effort.clone()),
        (32_000, effort),
        "an effort passes through at the usual cap"
    );
    assert_thinking_budgets_fit();
}

fn assert_thinking_budgets_fit() {
    let budget = |tokens| Some(Reasoning::Budget { tokens });

    for (limit, wanted) in [(4_096, 4_096), (8_192, 8_000), (128_000, 127_000), (2_048, 1_024)] {
        let (max, thinking) = budgets(&model_with(limit, true), budget(wanted));
        assert!(max as u64 <= limit, "{limit}/{wanted}");
        let fits = |tokens: u32| tokens + MIN_ANSWER_TOKENS <= max && tokens >= MIN_THINKING_TOKENS;
        assert!(
            thinking
                .as_ref()
                .is_none_or(|reasoning| matches!(reasoning, Reasoning::Budget { tokens } if fits(*tokens))),
            "{limit}/{wanted}: {max} {thinking:?}"
        );
    }
}
