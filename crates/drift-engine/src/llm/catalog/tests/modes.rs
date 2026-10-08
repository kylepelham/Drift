use super::super::*;

#[test]
fn each_models_dev_mode_is_its_own_entry_at_its_own_price_and_survives_the_cache() {
    let raw = r#"{"openai":{"id":"openai","name":"OpenAI","models":{"gpt-6-astra":{
        "id":"gpt-6-astra","name":"GPT-6 Astra","family":"gpt","release_date":"2026-08-01",
        "cost":{"input":5,"output":25,"cache_read":0.5},"experimental":{"modes":{
        "ultrafast":{"cost":{"input":60,"output":300},"provider":{"body":{"service_tier":"ultrafast"}}},
        "fast":{"provider":{"body":{"service_tier":"priority"}}}}}}}}}"#;
    let catalog = Catalog::parse(raw).unwrap();
    let ultrafast = catalog.model("openai", "gpt-6-astra-ultrafast").unwrap();

    assert_eq!(
        (ultrafast.id.as_str(), ultrafast.name.as_str()),
        ("gpt-6-astra-ultrafast", "GPT-6 Astra Ultrafast")
    );
    assert_eq!(
        (ultrafast.cost.input, ultrafast.cost.output, ultrafast.cost.cache_read),
        (60.0, 300.0, 0.5),
        "a price the mode leaves out is the base model's"
    );
    assert_eq!(ultrafast.wire("gpt-6-astra-ultrafast"), "gpt-6-astra");
    assert_eq!(ultrafast.mode.as_ref().unwrap().body["service_tier"], "ultrafast");
    assert_eq!(
        (ultrafast.profile, ultrafast.release_date.as_str()),
        (ToolProfile::ApplyPatch, "2026-08-01"),
        "everything else is the base model's"
    );
    assert_eq!(catalog.model("openai", "gpt-6-astra-fast").unwrap().cost.input, 5.0);

    let base = catalog.model("openai", "gpt-6-astra").unwrap();
    assert!(base.mode.is_none() && base.wire("gpt-6-astra") == "gpt-6-astra");
    let cached = Catalog::parse(&serde_json::to_string(&catalog.providers).unwrap()).unwrap();
    assert_eq!(cached, catalog, "the cache stores the entries already made");
}

#[test]
fn small_jobs_never_take_a_mode_and_the_snapshot_lists_fast_and_ultrafast() {
    let catalog = Catalog::bundled();
    assert_eq!(
        catalog.model("anthropic", "claude-opus-5-5-fast").unwrap().name,
        "Claude Opus 5.5 Fast"
    );
    assert!(catalog.model("openai", "gpt-6-astra-ultrafast").is_some());
    assert!(catalog.model("openai", "gpt-5.5-fast").is_some());

    let model = |provider: &str, model: &str| ModelRef {
        provider: provider.into(),
        model: model.into(),
    };
    let small = catalog.small_model(&model("openai", "gpt-5.5")).unwrap();
    assert!(
        catalog.model("openai", &small.model).unwrap().mode.is_none(),
        "{small:?} is a mode; flex is slow"
    );

    assert!(
        catalog.same_model(
            &model("anthropic", "claude-opus-5-5"),
            &model("anthropic", "claude-opus-5-5-fast")
        ),
        "a mode runs its base model"
    );
    assert!(
        !catalog.same_model(
            &model("anthropic", "claude-opus-5-5"),
            &model("anthropic", "claude-opus-5")
        ),
        "a sibling in the family does not"
    );
    assert!(!catalog.same_model(
        &model("anthropic", "claude-opus-5-5"),
        &model("amazon-bedrock", "claude-opus-5-5")
    ));
    assert!(
        catalog.same_model(&model("anthropic", "retired"), &model("anthropic", "retired"))
            && !catalog.same_model(&model("anthropic", "retired"), &model("anthropic", "gone"))
    );
}
