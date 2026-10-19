use super::*;
use crate::imagegen::{Paid, Payment};

/// A ChatGPT sign-in for OpenAI, and an API key saved for images beside it.
fn plan_and_key(engine: &Engine) {
    let plan = Credential::OAuth {
        access: "plan-token".into(),
        refresh: "r".into(),
        expires_at: i64::MAX,
        account: Some("acct".into()),
    };
    engine.credentials.set("openai", &plan).unwrap();
    engine.set_image_key("openai", "sk-images").unwrap();
}

fn openai_first(engine: &Engine, first: Payment) {
    let settings = ImageGeneration {
        provider: "openai".into(),
        openai_first: first,
        ..ImageGeneration::default()
    };
    engine.store.set_setting(SETTINGS_KEY, &settings).unwrap();
}

#[tokio::test]
async fn the_plan_pays_first_through_the_codex_images_route() {
    let (engine, server) = engine().await;
    plan_and_key(&engine);
    openai_first(&engine, Payment::Plan);
    let listed = engine
        .image_providers()
        .into_iter()
        .find(|provider| provider.id == "openai")
        .unwrap();
    assert!(listed.has_plan && listed.has_key && listed.ready);
    assert_eq!(listed.key.as_deref(), Some("...ages"));
    assert_eq!(engine.image_ask_label().as_deref(), Some("OpenAI on your ChatGPT plan"));

    let made = engine
        .generate_image(request("a red square", "auto", vec![reference()]))
        .await
        .unwrap();
    assert_eq!(
        (made.paid, made.cost),
        (Paid::Plan, None),
        "the plan's allowance, not a bill"
    );
    assert!(made.handed_over.is_none());

    let asked = asked(&server);
    assert_eq!(asked[0].path, "/chatgpt/images/edits");
    assert_eq!(asked[0].key, "plan-token");
    assert_eq!(asked[0].headers["chatgpt-account-id"], "acct");
    assert!(asked[0].headers.contains_key("originator"));
    assert!(
        asked[0].body.get("output_format").is_none(),
        "only the fields Codex sends: {}",
        asked[0].body
    );
    assert_eq!(asked[0].body["model"], "gpt-image-2");
}

#[tokio::test]
async fn a_spent_plan_hands_over_to_the_api_key() {
    let (engine, server) = engine().await;
    plan_and_key(&engine);
    openai_first(&engine, Payment::Plan);
    server.plan_spent.store(true, Ordering::SeqCst);

    let made = engine
        .generate_image(request("a red square", "auto", Vec::new()))
        .await
        .unwrap();
    assert_eq!(made.paid, Paid::Billed);
    assert!(made.cost.is_some());
    let note = made.handed_over.unwrap();
    assert!(
        note.contains("your ChatGPT plan was refused (429") && note.contains("the API key paid instead"),
        "{note}"
    );

    let paths: Vec<String> = asked(&server).into_iter().map(|asked| asked.path).collect();
    assert_eq!(paths, ["/chatgpt/images/generations", "/openai/images/generations"]);
    assert_eq!(asked(&server)[1].key, "sk-images");
}

#[tokio::test]
async fn the_key_goes_first_when_chosen_and_the_plan_alone_still_makes_images() {
    let (engine, server) = engine().await;
    plan_and_key(&engine);
    openai_first(&engine, Payment::ApiKey);
    assert_eq!(engine.image_ask_label().as_deref(), Some("OpenAI (billed)"));
    engine
        .generate_image(request("a red square", "auto", Vec::new()))
        .await
        .unwrap();
    assert_eq!(asked(&server)[0].path, "/openai/images/generations");

    engine.set_image_key("openai", "").unwrap();
    let plan_only = engine
        .image_providers()
        .into_iter()
        .find(|provider| provider.id == "openai")
        .unwrap();
    assert!(plan_only.has_plan && plan_only.key.is_none());
    assert!(engine.image_generation_available(), "a sign-in alone can pay");
    assert_eq!(
        engine.image_ask_label().as_deref(),
        Some("OpenAI on your ChatGPT plan"),
        "with no key, the plan goes first whatever the order says"
    );
}

#[tokio::test]
async fn off_and_a_sign_in_that_cannot_pay_leave_the_tool_out() {
    let (engine, _) = engine().await;
    assert_eq!(engine.image_settings().provider, "off", "off until enabled");
    assert!(!engine.image_generation_available());
    assert!(engine.generate_image(request("x", "auto", Vec::new())).await.is_err());

    let subscription = Credential::OAuth {
        access: "a".into(),
        refresh: "r".into(),
        expires_at: i64::MAX,
        account: None,
    };
    engine.credentials.set("xai", &subscription).unwrap();
    let settings = ImageGeneration {
        provider: "xai".into(),
        ..ImageGeneration::default()
    };
    engine.store.set_setting(SETTINGS_KEY, &settings).unwrap();
    if std::env::var("XAI_API_KEY").is_err() {
        assert!(
            !engine.image_generation_available(),
            "a Grok sign-in pays for chat, not images"
        );
    }
    assert!(
        engine.set_image_key("comfyui", "k").is_err(),
        "a local server takes no key"
    );
}
