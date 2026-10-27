use super::*;
use crate::llm::ToolSpec;
use crate::llm::catalog::{THINKING_OFF, THINKING_ON};

fn level(name: &str) -> Option<Reasoning> {
    Some(Reasoning::Effort { level: name.into() })
}

fn shaped(route: &Compat, model: &str, reasoning: Option<Reasoning>) -> Value {
    let mut request = super::tests::request();
    request.model = model.into();
    request.reasoning = reasoning;
    route.shaped(&request)
}

#[test]
fn kimi_switches_thinking_on_the_models_that_have_a_switch_and_takes_an_effort_elsewhere() {
    let kimi = Compat::moonshot("https://api.moonshot.example/v1");

    let off = shaped(&kimi, "kimi-k2.6", level(THINKING_OFF));
    assert_eq!(off["thinking"], json!({ "type": "disabled" }));
    assert!(off.get("reasoning_effort").is_none());
    assert_eq!(
        shaped(&kimi, "kimi-k2.6", level(THINKING_ON))["thinking"]["type"],
        "enabled"
    );

    let effort = shaped(&kimi, "kimi-k3", level("max"));
    assert_eq!(effort["reasoning_effort"], "max");
    assert!(effort.get("thinking").is_none(), "K3 is sent only its effort");
}

#[test]
fn kimi_keeps_a_conversation_on_its_cache_and_every_reply_carries_reasoning() {
    let mut request = super::tests::request();
    request.cache_key = Some("ses_1".into());
    request.messages.push(ChatMessage {
        role: Role::Assistant,
        blocks: vec![Block::Text("done".into())],
    });

    let body = Compat::moonshot("https://api.moonshot.example/v1").shaped(&request);
    assert_eq!(body["prompt_cache_key"], "ses_1");
    let reasoning: Vec<Value> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "assistant")
        .map(|reply| reply["reasoning_content"].clone())
        .collect();
    assert_eq!(
        reasoning,
        [json!("hm"), json!("")],
        "a reply that kept no reasoning goes with it empty"
    );
    assert!(
        Compat::new("https://other.example")
            .shaped(&request)
            .get("prompt_cache_key")
            .is_none()
    );

    request.tools.clear();
    let bare = Compat::moonshot("https://api.moonshot.example/v1").shaped(&request);
    assert!(
        bare.get("tools").is_none(),
        "a request without tools sends no tools field"
    );
}

#[test]
fn kimi_tool_schemas_type_their_enums_and_flatten_tuples() {
    let mut request = super::tests::request();
    request.tools = vec![ToolSpec {
        name: "pick".into(),
        description: "p".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "mode": { "enum": ["a", "b"] },
                "maybe": { "enum": [1, null] },
                "pair": { "type": "array", "prefixItems": [{ "type": "string" }, { "type": "number" }] },
                "one": { "type": "array", "items": [{ "type": "string" }], "unevaluatedItems": false },
                "link": { "$ref": "#/$defs/x", "description": "dropped" },
            },
        }),
    }];

    let body = Compat::moonshot("https://api.moonshot.example/v1").shaped(&request);
    let properties = &body["tools"][0]["function"]["parameters"]["properties"];
    assert_eq!(properties["mode"], json!({ "type": "string", "enum": ["a", "b"] }));
    assert_eq!(properties["maybe"]["type"], json!(["number", "null"]));
    assert_eq!(
        properties["pair"],
        json!({ "type": "array", "items": { "anyOf": [{ "type": "string" }, { "type": "number" }] } })
    );
    assert_eq!(
        properties["one"],
        json!({ "type": "array", "items": { "type": "string" } })
    );
    assert_eq!(properties["link"], json!({ "$ref": "#/$defs/x" }));
}

#[test]
fn deepseek_says_whether_it_thinks_beside_every_effort() {
    let deepseek = Compat::deepseek("https://api.deepseek.example");

    let effort = shaped(&deepseek, "deepseek-v4-pro", level("high"));
    assert_eq!(
        (effort["reasoning_effort"].clone(), effort["thinking"].clone()),
        (json!("high"), json!({ "type": "enabled" }))
    );
    let off = shaped(&deepseek, "deepseek-v4-pro", level(THINKING_OFF));
    assert_eq!(off["thinking"], json!({ "type": "disabled" }));
    assert!(off.get("reasoning_effort").is_none());
    assert_eq!(effort["max_tokens"], 500, "DeepSeek takes `max_tokens`");
}

#[test]
fn alibaba_spells_thinking_its_own_way_and_caches_qwen_at_the_last_turn() {
    let alibaba = Compat::alibaba("https://dashscope.example/compatible-mode/v1");

    let budget = shaped(&alibaba, "qwen-plus", Some(Reasoning::Budget { tokens: 4096 }));
    assert_eq!(
        (budget["enable_thinking"].clone(), budget["thinking_budget"].clone()),
        (json!(true), json!(4096))
    );
    assert_eq!(
        (budget["max_completion_tokens"].clone(), budget.get("max_tokens")),
        (json!(500), None)
    );
    let off = shaped(&alibaba, "qwen-plus", level(THINKING_OFF));
    assert_eq!(off["enable_thinking"], false);
    assert!(off.get("thinking").is_none());
    let effort = shaped(&alibaba, "qwen3.8-max", level("xhigh"));
    assert_eq!(
        (effort["enable_thinking"].clone(), effort["reasoning_effort"].clone()),
        (json!(true), json!("xhigh"))
    );

    let marked = |body: &Value| {
        body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message.to_string().contains("cache_control"))
            .map(|message| message["role"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(marked(&budget), ["system", "tool"]);
    assert!(
        marked(&shaped(&alibaba, "glm-5.1", None)).is_empty(),
        "other makers' models on DashScope are sent as they were"
    );
}

#[test]
fn alibaba_never_asks_for_explicit_cache_mode() {
    let mut one_off = super::tests::request();
    one_off.model = "qwen-plus".into();
    one_off.cache_conversation = false;
    one_off.cache_writes_billed = true;

    let body = Compat::alibaba("https://dashscope.example/compatible-mode/v1").shaped(&one_off);
    assert!(body.get("prompt_cache_options").is_none());
    assert!(!body.to_string().contains("cache_control"));
}
