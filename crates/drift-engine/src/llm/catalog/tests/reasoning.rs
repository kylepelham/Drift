use super::super::*;

#[test]
fn unpicked_levels_verbosity_thinking_and_sampling_follow_the_catalog() {
    let catalog = Catalog::bundled();
    let model = |provider: &str, id: &str| catalog.model(provider, id).unwrap().clone();
    let medium = Some(Reasoning::Effort { level: "medium".into() });

    assert_eq!(default_reasoning("openai", &model("openai", "gpt-5.5")), medium);
    assert_eq!(
        default_reasoning("openai", &model("openai", "gpt-5-pro")),
        None,
        "pro offers only high"
    );
    assert_eq!(
        default_reasoning("anthropic", &model("anthropic", "claude-sonnet-4-5")),
        None
    );
    assert_eq!(verbosity("openai", &model("openai", "gpt-5.5")), Some("low"));
    assert_eq!(
        verbosity("openai", &model("openai", "gpt-5.3-codex")),
        None,
        "Codex models are left as they are"
    );
    assert!(shows_thinking("google", &model("google", "gemini-2.5-pro")));
    assert!(!shows_thinking("anthropic", &model("anthropic", "claude-sonnet-4-5")));

    assert_eq!(sampling(&model("zai", "glm-4.6")).temperature, Some(1.0));
    assert_eq!(
        sampling(&model("google", "gemini-3.5-flash")),
        Sampling {
            temperature: Some(1.0),
            top_p: Some(0.95),
            top_k: Some(64)
        }
    );
    assert_eq!(sampling(&model("google", "gemini-3.5-flash-lite")), Sampling::default());
    assert_eq!(sampling(&model("google", "gemini-2.5-pro")).top_k, Some(64));
    let older = Model {
        release_date: "2024-12-11".into(),
        ..model("google", "gemini-2.5-flash")
    };
    assert_eq!(sampling(&older), Sampling::default(), "1.5 and 2.0 keep their own");
    assert_eq!(sampling(&model("anthropic", "claude-sonnet-4-5")), Sampling::default());
}

#[test]
fn each_model_gets_its_family_s_prompt_from_the_catalog_never_its_id() {
    let catalog = Catalog::bundled();
    let family = |provider: &str, model: &str| catalog.model(provider, model).unwrap().prompt;

    assert_eq!(family("openai", "gpt-5.5"), PromptFamily::Codex);
    assert_eq!(family("openai", "gpt-5.3-codex"), PromptFamily::Codex);
    assert_eq!(family("anthropic", "claude-opus-5-5"), PromptFamily::Claude);
    assert_eq!(
        family("anthropic", "claude-opus-5-5-fast"),
        PromptFamily::Claude,
        "a mode keeps its base's prompt"
    );
    assert_eq!(family("google", "gemini-2.5-pro"), PromptFamily::Gemini);

    let bedrock = catalog.providers["amazon-bedrock"].models.values().next().unwrap();
    assert_eq!(
        bedrock.prompt,
        PromptFamily::Claude,
        "Claude on another route is still Claude"
    );
    assert_eq!(
        prompt_for(ToolProfile::Edit, "gpt-4o"),
        PromptFamily::Default,
        "a model that edits with search and replace is not given the apply_patch prompt"
    );
    assert_eq!(prompt_for(ToolProfile::Edit, "grok"), PromptFamily::Default);
}

#[test]
fn openai_gpt_models_use_apply_patch() {
    assert_eq!(profile_for("openai", "gpt"), ToolProfile::ApplyPatch);
    assert_eq!(profile_for("openai", "gpt-4o"), ToolProfile::Edit);
    assert_eq!(profile_for("openai", "o"), ToolProfile::Edit);
    assert_eq!(profile_for("anthropic", "claude-sonnet"), ToolProfile::Edit);
}

#[test]
fn reasoning_variants_come_from_models_dev_and_suit_each_wire() {
    let options = |json: &str| serde_json::from_str::<Vec<serde_json::Value>>(json).unwrap();
    let names = |variants: Vec<Variant>| variants.into_iter().map(|variant| variant.name).collect::<Vec<_>>();
    let budget = |name: &str, tokens| Variant {
        name: name.into(),
        reasoning: Reasoning::Budget { tokens },
    };
    let effort = options(r#"[{"type":"effort","values":["low","medium","high","xhigh","max"]}]"#);

    assert_eq!(
        names(variants_for("anthropic", "claude-opus-5-5", 128_000, &effort)),
        ["low", "medium", "high", "xhigh", "max"]
    );
    assert_eq!(
        variants_for("amazon-bedrock", "anthropic.claude-opus-5-5", 128_000, &effort)[4].reasoning,
        Reasoning::Effort { level: "max".into() }
    );

    let both = options(r#"[{"type":"effort","values":["low","medium","high"]},{"type":"budget_tokens","min":1024}]"#);
    assert_eq!(
        variants_for("anthropic", "claude-opus-4-5", 64_000, &both),
        [budget("high", 16_000), budget("max", 31_999)],
        "Claude takes the budget it lists"
    );
    assert_eq!(
        variants_for("anthropic", "claude-haiku", 8_000, &both),
        [budget("high", 4_000), budget("max", 7_999)],
        "within the output limit"
    );
    assert_eq!(
        names(variants_for("openrouter", "z-ai/glm", 64_000, &both)),
        ["low", "medium", "high"],
        "elsewhere the effort wins"
    );
    let nullable = options(r#"[{"type":"effort","values":[null,"low","high"]}]"#);
    assert_eq!(
        names(variants_for("openai", "gpt-6-sol", 128_000, &nullable)),
        ["none", "low", "high"]
    );

    let range = options(r#"[{"type":"budget_tokens","min":128,"max":32768}]"#);
    assert_eq!(
        variants_for("google", "gemini-2.5-pro", 65_536, &range),
        [budget("high", 16_000), budget("max", 31_999)]
    );
    assert!(
        variants_for("xai", "grok", 64_000, &range).is_empty(),
        "a chat completions route has no budget to send"
    );
    assert!(variants_for("lmstudio", "qwen", 64_000, &options(r#"[{"type":"toggle"}]"#)).is_empty());
}

#[test]
fn the_bundled_snapshot_carries_each_models_reasoning_levels() {
    let catalog = Catalog::bundled();
    let names = |provider: &str, model: &str| {
        catalog
            .model(provider, model)
            .unwrap()
            .variants
            .iter()
            .map(|variant| variant.name.clone())
            .collect::<Vec<_>>()
    };

    assert_eq!(
        names("anthropic", "claude-opus-5-5"),
        ["low", "medium", "high", "xhigh", "max"]
    );
    assert_eq!(names("anthropic", "claude-sonnet-4-5"), ["high", "max"]);
    assert!(names("google", "gemini-3.8-flash").contains(&"high".to_string()));
}
