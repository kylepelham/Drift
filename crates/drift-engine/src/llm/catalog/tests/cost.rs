use super::super::*;

#[test]
fn long_prompts_are_priced_at_the_largest_tier_they_pass() {
    let raw = r#"{ "input": 2.5, "output": 15, "cache_read": 0.25,
        "tiers": [{ "input": 5, "output": 22.5, "cache_read": 0.5,
        "tier": { "type": "context", "size": 272000 } }],
        "context_over_200k": { "input": 4, "output": 20 } }"#;
    let cost: Cost = serde_json::from_str(raw).unwrap();

    assert_eq!(
        cost.tiers.iter().map(|tier| tier.above).collect::<Vec<_>>(),
        [200_000, 272_000],
        "models.dev's over-200k becomes a tier"
    );
    assert_eq!(cost.at(150_000), (2.5, 15.0, 0.25, 0.0));
    assert_eq!(
        cost.at(250_000),
        (4.0, 20.0, 0.25, 0.0),
        "a price the tier leaves out is the base one"
    );
    assert_eq!(cost.at(300_000), (5.0, 22.5, 0.5, 0.0));

    let stored: Cost = serde_json::from_str(&serde_json::to_string(&cost).unwrap()).unwrap();
    assert_eq!(stored, cost, "a cached catalog reads back the same tiers");

    let bundled = Catalog::bundled();
    let has_tiers = bundled
        .providers
        .values()
        .flat_map(|provider| provider.models.values())
        .any(|model| !model.cost.tiers.is_empty());
    assert!(has_tiers, "the snapshot carries tiers");
}
