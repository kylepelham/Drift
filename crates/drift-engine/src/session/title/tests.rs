use std::collections::HashMap;

use serde_json::json;

use super::*;
use crate::config::AgentOverride;
use crate::llm::Request;
use crate::session::turn::tests::{harness, prompt, text, Harness};
use crate::session::types::Visibility;
use crate::store::NewSession;

fn untitled(h: &Harness) -> Session {
    h.engine
        .store
        .create_session(NewSession { workspace_id: &h.session.workspace_id, parent_id: None, visibility: Visibility::Sibling, title: "", agent: "build", model: None })
        .unwrap()
}

async fn title_after(h: &Harness, id: &str, wanted: impl Fn(&str) -> bool) -> String {
    for _ in 0..300 {
        let title = h.engine.store.session(id).unwrap().unwrap().title;
        if wanted(&title) && !h.engine.turns.is_running(id) {
            return title;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("title never settled: {}", h.engine.store.session(id).unwrap().unwrap().title);
}

fn title_request(h: &Harness) -> Option<Request> {
    h.provider.requests.lock().unwrap().iter().find(|r| r.system.starts_with("You name conversations")).cloned()
}

#[tokio::test]
async fn a_new_conversation_is_named_by_a_small_model_from_its_provider() {
    let h = harness().await;
    let session = untitled(&h);
    h.provider.push(text("Fix parser bug")).push(text("Fix parser bug"));
    h.engine.submit(&session.id, prompt("please fix the parser bug in src/parse.rs")).await.unwrap();
    assert_eq!(title_after(&h, &session.id, |t| t == "Fix parser bug").await, "Fix parser bug");
    let request = title_request(&h).expect("a title request");
    let catalog = h.engine.catalog.read().unwrap();
    let models = &catalog.providers["anthropic"].models;
    let price = |id: &str| models[id].cost.input + models[id].cost.output;
    assert!(price(&request.model) < price("claude-sonnet-4-5"), "{} is not smaller", request.model);
    assert!(request.tools.is_empty());
    let small = &models[&request.model];
    if let Some(weakest) = small.variants.first().filter(|_| small.reasoning) {
        assert_eq!(request.reasoning.as_ref(), Some(&weakest.reasoning), "a reasoning model titles at its weakest level");
        assert!(request.max_tokens > 1_024, "with room to think as well as answer: {}", request.max_tokens);
    }
}

#[test]
fn a_chatgpt_sign_in_titles_on_the_backends_mini_model_never_an_api_only_one() {
    use crate::llm::openai::codex;
    use crate::session::types::ModelRef;
    let mut catalog = crate::llm::catalog::Catalog::bundled();
    codex::shape(catalog.providers.get_mut("openai").unwrap());
    let like = ModelRef { provider: "openai".into(), model: "gpt-5.5".into() };
    assert!(catalog.small_model(&like).is_some_and(|by_price| by_price.model != "gpt-5.4-mini"), "the API prices would choose another");
    let signed_in = crate::llm::Credential::OAuth { access: "a".into(), refresh: "r".into(), expires_at: 0, account: None };
    let chosen = codex::small_model(&catalog, &like, &signed_in).expect("the mini model the backend takes");
    assert_eq!(chosen.model, "gpt-5.4-mini");
    assert!(catalog.model("openai", &chosen.model).is_some(), "offered by the Codex view");
    let key = crate::llm::Credential::ApiKey { key: "k".into() };
    assert_eq!(codex::small_model(&catalog, &like, &key), None, "an API key chooses by price as before");
}

#[tokio::test]
async fn a_model_pinned_in_settings_names_the_conversation() {
    let h = harness().await;
    let pinned = h.engine.catalog.read().unwrap().providers["anthropic"].models.keys().find(|id| id.as_str() != "claude-sonnet-4-5").unwrap().clone();
    h.engine.set_agent_overrides(HashMap::from([("title".into(), AgentOverride::from_json(&json!({ "model": format!("anthropic/{pinned}") })))]));
    let session = untitled(&h);
    h.provider.push(text("Pinned title")).push(text("Pinned title"));
    h.engine.submit(&session.id, prompt("do the thing")).await.unwrap();
    title_after(&h, &session.id, |t| t == "Pinned title").await;
    assert_eq!(title_request(&h).unwrap().model, pinned);
}

#[tokio::test]
async fn without_a_usable_title_model_the_first_message_stays_the_title() {
    let h = harness().await;
    h.engine.set_agent_overrides(HashMap::from([("title".into(), AgentOverride::from_json(&json!({ "model": "openai/gpt-5" })))]));
    let session = untitled(&h);
    h.provider.push(text("done"));
    h.engine.submit(&session.id, prompt("  rename   the module\nplease ")).await.unwrap();
    assert_eq!(title_after(&h, &session.id, |t| !t.is_empty()).await, "rename the module please");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(title_request(&h).is_none(), "no credential for the pinned provider, so no request");
}

#[test]
fn a_rename_is_never_overwritten_by_a_late_title() {
    let store = crate::store::tests::store();
    let session = store.create_session(NewSession { workspace_id: "w", parent_id: None, visibility: Visibility::Sibling, title: "Mine", agent: "build", model: None }).unwrap();
    assert!(store.retitle_if(&session.id, "placeholder", "Generated").unwrap().is_none());
    assert_eq!(store.session(&session.id).unwrap().unwrap().title, "Mine");
}

#[test]
fn replies_are_trimmed_to_a_single_bare_title() {
    assert_eq!(clean("\n  \"Fix the parser.\"\nextra").as_deref(), Some("Fix the parser"));
    assert_eq!(clean("Title: Add dark mode!").as_deref(), Some("Add dark mode"));
    assert_eq!(clean("  \n "), None);
}
