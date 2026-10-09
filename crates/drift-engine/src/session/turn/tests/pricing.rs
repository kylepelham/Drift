use super::*;

#[test]
fn a_request_with_a_long_prompt_costs_its_tier_counting_cached_input() {
    let mut model = crate::llm::catalog::Catalog::bundled()
        .model("anthropic", "claude-sonnet-4-5")
        .unwrap()
        .clone();
    model.cost = serde_json::from_str(concat!(
        r#"{ "input": 3, "output": 15, "cache_read": 0.3, "context_over_200k": "#,
        r#"{ "input": 6, "output": 22.5, "cache_read": 0.6 } }"#,
    ))
    .unwrap();

    let short = cost(
        &model,
        Usage {
            input: 1_000_000,
            output: 0,
            cache_read: 0,
            cache_write: 0,
        },
    );
    assert!(
        (short - 6.0).abs() < 1e-9,
        "a million fresh tokens in one prompt is over 200k: {short}"
    );
    let cached = cost(
        &model,
        Usage {
            input: 10_000,
            output: 1_000_000,
            cache_read: 250_000,
            cache_write: 0,
        },
    );
    assert!(
        (cached - (0.06 + 22.5 + 0.15)).abs() < 1e-9,
        "cached input counts toward the prompt's length: {cached}"
    );
    let small = cost(
        &model,
        Usage {
            input: 100_000,
            output: 0,
            cache_read: 0,
            cache_write: 0,
        },
    );
    assert!((small - 0.3).abs() < 1e-9, "{small}");
}

#[tokio::test]
async fn a_subscription_turn_is_priced_at_the_api_rates_it_saves() {
    let h = harness().await;
    h.engine
        .credentials
        .set(
            "anthropic",
            &Credential::OAuth {
                access: "a".into(),
                refresh: "r".into(),
                expires_at: id::now_ms() + 3_600_000,
                account: None,
            },
        )
        .unwrap();
    h.provider.push(text("hello"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_idle(&h).await;

    let reply = transcript(&h).pop().unwrap().info;
    assert!(
        reply.cost > 0.0,
        "a signed-in turn shows what the API would have charged: {}",
        reply.cost
    );
}
