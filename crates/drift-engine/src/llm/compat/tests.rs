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
        no_tool_calls: false,
        verbosity: None,
        show_thinking: false,
        top_p: None,
        top_k: None,
        mode: None,
    }
}

#[test]
fn reasoning_goes_as_each_preset_takes_it() {
    let reasoned = |reasoning, object| {
        let mut body = json!({});
        reason(&mut body, Some(&reasoning), object);
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
        vec![]
    );
    assert_eq!(
        feed(
            &mut state,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"p\":1}"}}]}}]}"#
        ),
        vec![]
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
                .is_empty()
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
