use super::*;
use crate::llm::ToolSpec;

#[test]
fn a_pdf_is_a_file_part() {
    let sent = message(&ChatMessage {
        role: Role::User,
        blocks: vec![Block::Pdf {
            base64: "JVBERi0=".into(),
        }],
    });
    assert_eq!(
        sent[0]["content"][0]["file"]["file_data"],
        "data:application/pdf;base64,JVBERi0="
    );
}

#[test]
fn zai_keeps_thinking_and_tuned_sampling_is_sent() {
    let tuned = Request {
        temperature: Some(1.0),
        top_p: Some(0.95),
        top_k: Some(40),
        ..request()
    };
    let zai = Compat::zai("https://z.example").shaped(&tuned);
    assert_eq!(zai["thinking"], json!({ "type": "enabled", "clear_thinking": false }));
    assert_eq!(
        (zai["temperature"].as_f64(), zai["top_p"].as_f64()),
        (Some(1.0), Some(0.95))
    );
    assert!(zai.get("top_k").is_none(), "not a Chat Completions field");
    assert!(
        Compat::new("https://other.example")
            .shaped(&tuned)
            .get("thinking")
            .is_none()
    );
}

#[test]
fn a_text_only_request_keeps_its_tools_but_forbids_calls() {
    assert!(body(&request()).get("tool_choice").is_none());
    let built = body(&Request {
        no_tool_calls: true,
        ..request()
    });
    assert_eq!(
        (
            built["tool_choice"].clone(),
            built["tools"].as_array().is_some_and(|tools| !tools.is_empty())
        ),
        (json!("none"), true)
    );
}

pub(super) fn request() -> Request {
    Request {
        model: "grok-4.5".into(),
        system: "sys".into(),
        messages: vec![
            ChatMessage {
                role: Role::User,
                blocks: vec![Block::Text("hi".into())],
            },
            ChatMessage {
                role: Role::Assistant,
                blocks: vec![
                    Block::Reasoning {
                        text: "hm".into(),
                        signature: None,
                        redacted: None,
                    },
                    Block::Text("ok".into()),
                    Block::ToolUse {
                        id: "call_1".into(),
                        name: "read".into(),
                        input: json!({ "path": "a" }),
                    },
                ],
            },
            ChatMessage {
                role: Role::User,
                blocks: vec![Block::ToolResult {
                    call_id: "call_1".into(),
                    content: "1: x".into(),
                    is_error: false,
                }],
            },
        ],
        tools: vec![ToolSpec {
            name: "read".into(),
            description: "r".into(),
            input_schema: json!({ "type": "object" }),
        }],
        max_tokens: 500,
        reasoning: None,
        temperature: Some(0.2),
        cache_key: None,
        connection: None,
        turn: None,
        effort_marks: Vec::new(),
        claude: Default::default(),
        cache_hour: false,
        cache_conversation: true,
        cache_writes_billed: false,
        deferred_tools: Vec::new(),
        tool_references: Default::default(),
        no_tool_calls: false,
        verbosity: None,
        show_thinking: false,
        top_p: None,
        top_k: None,
        mode: None,
        context_window: None,
    }
}

#[test]
fn reasoning_goes_as_each_preset_takes_it() {
    let reasoned = |reasoning, object: bool| {
        let mut body = json!({});
        let thinking = if object { Thinking::Object } else { Thinking::Effort };
        reason(&mut body, Some(&reasoning), thinking);
        body
    };
    let effort = || Reasoning::Effort { level: "high".into() };
    assert_eq!(reasoned(effort(), false), json!({ "reasoning_effort": "high" }));
    assert_eq!(
        reasoned(effort(), true),
        json!({ "reasoning": { "effort": "high" } }),
        "OpenRouter"
    );
    assert_eq!(
        reasoned(Reasoning::Budget { tokens: 8000 }, true),
        json!({ "reasoning": { "max_tokens": 8000 } })
    );
    assert_eq!(
        reasoned(Reasoning::Budget { tokens: 8000 }, false),
        json!({}),
        "no budget field to carry it"
    );
}

#[test]
fn body_matches_chat_completions() {
    let built = body(&request());
    let messages = built["messages"].as_array().unwrap();
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[1]["content"][0]["type"], "text");
    assert_eq!(messages[2]["role"], "assistant");
    assert_eq!(messages[2]["content"], "ok");
    assert_eq!(messages[2]["reasoning_content"], "hm");
    assert_eq!(messages[2]["tool_calls"][0]["function"]["arguments"], r#"{"path":"a"}"#);
    assert_eq!(messages[3]["role"], "tool");
    assert_eq!(messages[3]["tool_call_id"], "call_1");
    assert_eq!(built["tools"][0]["function"]["name"], "read");
    assert_eq!(built["stream_options"]["include_usage"], true);
    assert_eq!(built["temperature"], 0.2);
}

#[test]
fn stream_deltas_become_blocks() {
    let mut state = StreamState::default();
    let feed = |state: &mut StreamState, json: &str| state.chunks(json).unwrap();
    assert_eq!(
        feed(
            &mut state,
            r#"{"choices":[{"delta":{"role":"assistant","content":""}}]}"#
        ),
        vec![]
    );
    assert_eq!(
        feed(&mut state, r#"{"choices":[{"delta":{"reasoning_content":"th"}}]}"#),
        vec![Chunk::ReasoningStart, Chunk::ReasoningDelta("th".into())]
    );
    assert_eq!(
        feed(&mut state, r#"{"choices":[{"delta":{"content":"Hi"}}]}"#),
        vec![Chunk::BlockStop, Chunk::TextStart, Chunk::TextDelta("Hi".into())]
    );
    assert_eq!(
        feed(&mut state, r#"{"choices":[{"delta":{"content":"!"}}]}"#),
        vec![Chunk::TextDelta("!".into())]
    );
    assert_eq!(
        feed(
            &mut state,
            &json!({ "choices": [{ "delta": { "tool_calls": [
                { "index": 0, "id": "call_a", "function": { "name": "read", "arguments": "" } }
            ] } }] })
            .to_string()
        ),
        vec![Chunk::Generating]
    );
    assert_eq!(
        feed(
            &mut state,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"p\":1}"}}]}}]}"#
        ),
        vec![Chunk::Generating]
    );
    assert_eq!(
        feed(&mut state, r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#),
        vec![]
    );
    assert_eq!(
        feed(
            &mut state,
            &json!({
                "choices": [],
                "usage": {
                    "prompt_tokens": 50,
                    "completion_tokens": 7,
                    "prompt_tokens_details": { "cached_tokens": 20 }
                }
            })
            .to_string()
        ),
        vec![]
    );
    assert_eq!(
        feed(&mut state, "[DONE]"),
        vec![
            Chunk::BlockStop,
            Chunk::ToolUseStart {
                id: "call_a".into(),
                name: "read".into()
            },
            Chunk::ToolInputDelta("{\"p\":1}".into()),
            Chunk::BlockStop,
            Chunk::Usage(Usage {
                input: 30,
                output: 7,
                cache_read: 20,
                cache_write: 0
            }),
            Chunk::Stop(StopReason::ToolUse)
        ]
    );
}

#[test]
fn interleaved_calls_are_put_together_by_index() {
    let mut state = StreamState::default();
    for delta in [
        r#"{"index":0,"id":"call_a","function":{"name":"read","arguments":"{\"path\":"}}"#,
        r#"{"index":1,"id":"call_b","function":{"name":"grep","arguments":"{\"pattern\":"}}"#,
        r#"{"index":0,"function":{"arguments":"\"a.txt\"}"}}"#,
        r#"{"index":1,"function":{"arguments":"\"x\"}"}}"#,
    ] {
        assert!(
            state
                .chunks(&format!(r#"{{"choices":[{{"delta":{{"tool_calls":[{delta}]}}}}]}}"#))
                .unwrap()
                == [Chunk::Generating],
            "a held fragment only times the reply"
        );
    }
    state
        .chunks(r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#)
        .unwrap();
    let out = state.chunks("[DONE]").unwrap();
    let starts: Vec<&Chunk> = out.iter().filter(|c| matches!(c, Chunk::ToolUseStart { .. })).collect();
    assert_eq!(starts.len(), 2, "one start per call: {out:?}");
    assert!(
        out.contains(&Chunk::ToolInputDelta("{\"path\":\"a.txt\"}".into()))
            && out.contains(&Chunk::ToolInputDelta("{\"pattern\":\"x\"}".into()))
    );
}

#[test]
fn a_stream_that_never_says_why_it_finished_is_refused_with_its_calls() {
    let mut state = StreamState::default();
    let call = json!({ "choices": [{ "delta": { "tool_calls": [
        { "index": 0, "id": "call_a", "function": { "name": "bash", "arguments": "{\"command\":\"rm -rf" } }
    ] } }] });
    state.chunks(&call.to_string()).unwrap();

    let error = state.chunks("[DONE]").unwrap_err();
    assert!(error.to_string().contains("without a finish reason"), "{error}");
}

#[test]
fn errors_and_length_stops_classify() {
    assert!(matches!(
        StreamState::default().chunks(r#"{"error":{"message":"nope","code":"bad"}}"#),
        Err(Error::Api { retryable: false, .. })
    ));
    let gateway = StreamState::default()
        .chunks(r#"{"error":{"message":"Provider returned error","code":502},"choices":[{"finish_reason":"error"}]}"#);
    assert!(
        matches!(
            gateway,
            Err(Error::Api {
                status: 502,
                retryable: true,
                ..
            })
        ),
        "a gateway's streamed 502 retries: {gateway:?}"
    );
    let overloaded = StreamState::default().chunks(r#"{"error":{"message":"busy","type":"overloaded_error"}}"#);
    assert!(
        matches!(overloaded, Err(Error::Api { retryable: true, .. })),
        "an upstream overload passed through retries"
    );
    let unauthenticated = StreamState::default().chunks(r#"{"error":{"message":"key","code":401}}"#);
    assert!(matches!(unauthenticated, Err(Error::Unauthenticated(ref m)) if m == "key"));
    let mut state = StreamState::default();
    state
        .chunks(r#"{"choices":[{"delta":{"content":"x"},"finish_reason":"length"}]}"#)
        .unwrap();
    assert!(state.done().unwrap().contains(&Chunk::Stop(StopReason::MaxTokens)));
    assert!(matches!(api_error(429, "{}"), Error::Api { retryable: true, .. }));
}

#[test]
fn an_error_message_is_read_from_whichever_layout_the_server_used() {
    for body in [
        r#"{"detail":"Model not found"}"#,
        r#"{"detail":{"type":"not_found","message":"Model not found"}}"#,
        r#"{"error":"Model not found"}"#,
        r#"{"errors":[{"message":"Model not found"}]}"#,
    ] {
        let error = api_error(404, body);
        assert!(
            matches!(error, Error::Api { ref message, .. } if message == "Model not found"),
            "{body}: {error:?}"
        );
    }
    let streamed = StreamState::default().chunks(r#"{"error":"Model not found"}"#);
    assert!(
        matches!(streamed, Err(Error::Api { ref message, .. }) if message == "Model not found"),
        "a streamed error given as text fails the reply: {streamed:?}"
    );
}

#[test]
fn xais_refusal_of_a_key_is_refused_credentials() {
    let refused = api_error(
        400,
        r#"{"code":"Client specified an invalid argument","error":"Incorrect API key provided: xa***. You can obtain an API key from https://console.x.ai."}"#,
    );
    assert!(
        matches!(refused, Error::Unauthenticated(ref words) if words.starts_with("Incorrect API key")),
        "{refused:?}"
    );
    assert!(matches!(
        api_error(400, r#"{"error":"bad argument"}"#),
        Error::Api { .. }
    ));
}

#[test]
fn usage_is_read_where_each_server_puts_it() {
    let usage = |chunk: &str| {
        let mut state = StreamState::default();
        state.chunks(chunk).unwrap();
        state.usage.take()
    };
    let expected = Some(Usage {
        input: 70,
        output: 5,
        cache_read: 30,
        cache_write: 0,
    });
    for chunk in [
        r#"{"choices":[{"delta":{},"usage":{"prompt_tokens":100,"completion_tokens":5,"cached_tokens":30}}]}"#,
        r#"{"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":5,"prompt_cache_hit_tokens":30}}"#,
        r#"{"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":5,"prompt_tokens_details":{"cached_tokens":30}}}"#,
    ] {
        assert_eq!(usage(chunk), expected, "{chunk}");
    }
}

#[test]
fn call_ids_replayed_to_claude_through_a_gateway_take_anthropics_characters() {
    let mut request = request();
    request.model = "anthropic/claude-sonnet-4.5".into();
    request.messages[1].blocks[2] = Block::ToolUse {
        id: "functions.read:0".into(),
        name: "read".into(),
        input: json!({}),
    };
    request.messages[2].blocks[0] = Block::ToolResult {
        call_id: "functions.read:0".into(),
        content: "ok".into(),
        is_error: false,
    };
    let built = Compat::openrouter("https://openrouter.example").shaped(&request);
    assert_eq!(built["messages"][2]["tool_calls"][0]["id"], "functions_read_0");
    assert_eq!(built["messages"][3]["tool_call_id"], "functions_read_0");
    assert!(built["messages"][1].get("tool_call_id").is_none(), "no field is added");

    request.model = "moonshotai/kimi-k2".into();
    let kept = Compat::openrouter("https://openrouter.example").shaped(&request);
    assert_eq!(kept["messages"][2]["tool_calls"][0]["id"], "functions.read:0");
}
