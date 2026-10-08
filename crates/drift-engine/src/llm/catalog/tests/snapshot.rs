use super::super::*;

#[test]
fn bundled_snapshot_has_anthropic_models_with_costs() {
    let catalog = Catalog::bundled();
    let anthropic = &catalog.providers["anthropic"];
    assert!(anthropic.models.len() > 5);

    let model = anthropic.models.values().next().unwrap();
    assert!(model.limit.context > 0);
    assert!(model.cost.input > 0.0);
    assert_eq!(model.profile, ToolProfile::Edit);
}

#[test]
fn snapshot_only_keeps_our_providers() {
    let catalog = Catalog::bundled();
    for id in catalog.providers.keys() {
        assert!(PROVIDERS.contains(&id.as_str()), "{id}");
    }
}

#[test]
fn cloud_routes_offer_only_what_their_adapters_speak_and_openrouter_is_a_provider() {
    let catalog = Catalog::bundled();
    assert!(!catalog.providers["amazon-bedrock"].models.is_empty());
    assert!(
        catalog.providers["amazon-bedrock"]
            .models
            .values()
            .all(|model| model.id.contains("anthropic."))
    );
    assert!(
        catalog.providers["google-vertex"]
            .models
            .values()
            .all(|model| model.id.starts_with("claude") || model.id.starts_with("gemini"))
    );

    let raw = r#"{"openrouter":{"id":"openrouter","name":"OpenRouter",
        "env":["OPENROUTER_API_KEY"],"api":"https://openrouter.ai/api/v1",
        "models":{"anthropic/claude-sonnet-4.5":{"id":"anthropic/claude-sonnet-4.5",
        "name":"Claude Sonnet 4.5","tool_call":true}}}}"#;
    let parsed = Catalog::parse(raw).unwrap();
    assert!(
        parsed.model("openrouter", "anthropic/claude-sonnet-4.5").is_some(),
        "selectable once the catalog lists it"
    );
}

#[test]
fn cache_is_reused_when_fresh_and_round_trips() {
    let directory = std::env::temp_dir().join(format!("drift-catalog-{}", crate::random_hex(4)));
    std::fs::create_dir_all(&directory).unwrap();
    let mut catalog = Catalog::bundled();
    catalog.providers.retain(|id, _| id == "anthropic");
    let encoded = serde_json::to_string(&catalog.providers).unwrap();
    std::fs::write(cache_path(&directory), encoded).unwrap();

    let loaded = Catalog::load(&directory);
    assert_eq!(loaded.providers.len(), 1);

    std::fs::remove_dir_all(directory).ok();
}
