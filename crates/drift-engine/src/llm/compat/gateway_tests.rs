use super::tests::request;
use super::*;

fn conversation(model: &str) -> Request {
    let user = |text: &str| ChatMessage {
        role: Role::User,
        blocks: vec![Block::Text(text.into())],
    };
    let assistant = ChatMessage {
        role: Role::Assistant,
        blocks: vec![Block::Text("ok".into())],
    };

    Request {
        model: model.into(),
        messages: vec![user("one"), assistant.clone(), user("two"), assistant, user("three")],
        ..request()
    }
}

fn marked(body: &Value) -> Vec<String> {
    let mut marked = Vec::new();
    for message in body["messages"].as_array().unwrap() {
        for part in message["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|part| part.get("cache_control").is_some())
        {
            let role = message["role"].as_str().unwrap();
            let text = part["text"].as_str().unwrap();
            marked.push(format!("{role}:{text}"));
        }
    }

    marked
}

#[test]
fn claude_through_a_caching_gateway_gets_the_anthropic_breakpoints_and_nothing_else_does() {
    let mut body = body(&conversation("anthropic/claude-sonnet-4.5"));
    mark_breakpoints(&mut body);
    assert_eq!(marked(&body), ["system:sys", "user:two", "user:three"]);
    assert!(is_claude("~anthropic/claude-sonnet-latest") && !is_claude("openai/gpt-6"));

    let usage = usage_from(&json!({
        "prompt_tokens": 10_339, "completion_tokens": 60,
        "prompt_tokens_details": { "cached_tokens": 10_000, "cache_write_tokens": 300 },
    }));
    assert_eq!(
        usage,
        Usage {
            input: 39,
            output: 60,
            cache_read: 10_000,
            cache_write: 300
        }
    );
}

#[test]
fn tool_messages_follow_the_calls_and_text_in_that_turn_comes_after_them() {
    let results = ChatMessage {
        role: Role::User,
        blocks: vec![
            Block::ToolResult {
                call_id: "call_1".into(),
                content: "1: x".into(),
                is_error: false,
            },
            Block::Text("The read call (call_1) returned this:".into()),
            Block::Text("and also look at b".into()),
        ],
    };
    let mut request = request();
    request.messages[2] = results;

    let built = body(&request);
    let roles: Vec<&str> = built["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["system", "user", "assistant", "tool", "user"]);
}

#[test]
fn reasoning_given_goes_back_on_its_own_assistant_message() {
    let thinking = |text: &str| ChatMessage {
        role: Role::Assistant,
        blocks: vec![
            Block::Reasoning {
                text: text.into(),
                signature: None,
                redacted: None,
            },
            Block::ToolUse {
                id: format!("c_{text}"),
                name: "read".into(),
                input: json!({}),
            },
        ],
    };
    let result = |text: &str| ChatMessage {
        role: Role::User,
        blocks: vec![Block::ToolResult {
            call_id: format!("c_{text}"),
            content: "r".into(),
            is_error: false,
        }],
    };
    let prompt = |text: &str| ChatMessage {
        role: Role::User,
        blocks: vec![Block::Text(text.into())],
    };
    let steered = ChatMessage {
        role: Role::User,
        blocks: vec![
            Block::ToolResult {
                call_id: "c_next".into(),
                content: "r".into(),
                is_error: false,
            },
            Block::Text("also check b".into()),
        ],
    };
    let messages = vec![
        prompt("one"),
        thinking("old"),
        result("old"),
        ChatMessage {
            role: Role::Assistant,
            blocks: vec![Block::Text("done".into())],
        },
        prompt("two"),
        thinking("new"),
        steered,
        thinking("next"),
        result("next"),
    ];

    let built = body(&Request { messages, ..request() });
    let kept: Vec<&str> = built["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["reasoning_content"].as_str())
        .collect();
    assert_eq!(
        kept,
        ["old", "new", "next"],
        "which turns' reasoning to send is the session's choice; the wire keeps what it is given"
    );
}

#[test]
fn a_tool_loop_caches_past_the_prompt_that_started_it() {
    let call = |id: &str| ChatMessage {
        role: Role::Assistant,
        blocks: vec![Block::ToolUse {
            id: id.into(),
            name: "read".into(),
            input: json!({}),
        }],
    };
    let result = |id: &str| ChatMessage {
        role: Role::User,
        blocks: vec![
            Block::ToolResult {
                call_id: format!("{id}a"),
                content: format!("{id} first"),
                is_error: false,
            },
            Block::ToolResult {
                call_id: format!("{id}b"),
                content: format!("{id} second"),
                is_error: false,
            },
        ],
    };
    let prompt = ChatMessage {
        role: Role::User,
        blocks: vec![Block::Text("go".into())],
    };
    let mut body = body(&Request {
        model: "anthropic/claude-sonnet-4.5".into(),
        messages: vec![prompt, call("x"), result("x"), call("y"), result("y")],
        ..request()
    });

    mark_breakpoints(&mut body);
    assert_eq!(
        marked(&body),
        ["system:sys", "tool:x second", "tool:y second"],
        "the last result of each of the last two turns"
    );
}

/// A local OpenRouter: records the body it got and replies with cache-hit usage.
#[tokio::test]
async fn an_openrouter_exchange_sends_breakpoints_for_claude_only_and_reads_cache_usage() {
    use axum::extract::State;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let seen: std::sync::Arc<std::sync::Mutex<Vec<Value>>> = Default::default();
    let handler = |State(seen): State<std::sync::Arc<std::sync::Mutex<Vec<Value>>>>,
                   axum::Json(body): axum::Json<Value>| async move {
        seen.lock().unwrap().push(body);
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":900,",
            "\"completion_tokens\":2,\"prompt_tokens_details\":{\"cached_tokens\":800,\"cache_write_tokens\":0}}}\n\n",
            "data: [DONE]\n\n",
        );

        ([("content-type", "text/event-stream")], sse)
    };
    let app = axum::Router::new()
        .route("/chat/completions", axum::routing::post(handler))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let gateway = Compat::openrouter(&url);
    let key = Credential::ApiKey { key: "or-key".into() };
    let chunks: Vec<Chunk> = gateway
        .stream(&conversation("anthropic/claude-sonnet-4.5"), &key)
        .await
        .unwrap()
        .map(Result::unwrap)
        .collect()
        .await;
    assert!(chunks.contains(&Chunk::Usage(Usage {
        input: 100,
        output: 2,
        cache_read: 800,
        cache_write: 0
    })));

    gateway
        .stream(&conversation("openai/gpt-6"), &key)
        .await
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>()
        .await;
    let seen = seen.lock().unwrap();
    assert_eq!(marked(&seen[0]), ["system:sys", "user:two", "user:three"]);
    assert!(
        marked(&seen[1]).is_empty(),
        "other vendors' models are sent as they were"
    );
}
