use super::*;
use crate::llm::ToolSpec;

#[test]
fn a_pdf_is_inline_data() {
    let sent = content(
        &ChatMessage {
            role: Role::User,
            blocks: vec![Block::Pdf {
                base64: "JVBERi0=".into(),
            }],
        },
        &mut HashMap::new(),
    );
    assert_eq!(
        sent["parts"][0],
        json!({ "inlineData": { "mimeType": "application/pdf", "data": "JVBERi0=" } })
    );
}

#[test]
fn thoughts_are_asked_for_at_the_default_level_and_sampling_is_sent() {
    assert!(
        body(&Request {
            reasoning: None,
            ..request()
        })
        .pointer("/generationConfig/thinkingConfig")
        .is_none()
    );
    let built = body(&Request {
        reasoning: None,
        show_thinking: true,
        temperature: Some(1.0),
        top_p: Some(0.95),
        top_k: Some(64),
        ..request()
    });
    let config = &built["generationConfig"];
    assert_eq!(config["thinkingConfig"], json!({ "includeThoughts": true }));
    assert_eq!(
        (
            config["temperature"].as_f64(),
            config["topP"].as_f64(),
            config["topK"].as_u64()
        ),
        (Some(1.0), Some(0.95), Some(64))
    );
}

#[test]
fn a_text_only_request_keeps_its_tools_but_forbids_calls() {
    assert!(body(&request()).get("toolConfig").is_none());
    let built = body(&Request {
        no_tool_calls: true,
        ..request()
    });
    assert_eq!(
        built["toolConfig"],
        json!({ "functionCallingConfig": { "mode": "NONE" } })
    );
    assert!(
        built["tools"][0]["functionDeclarations"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty())
    );
}

fn request() -> Request {
    Request {
        model: "gemini-2.5-pro".into(),
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
                        signature: Some("sig".into()),
                        redacted: None,
                    },
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
            input_schema: json!({ "type": "object", "additionalProperties": false, "properties": {} }),
        }],
        max_tokens: 500,
        reasoning: Some(Reasoning::Budget { tokens: 2048 }),
        temperature: None,
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
fn a_level_is_sent_as_gemini_3_names_it() {
    let mut request = request();
    request.reasoning = Some(Reasoning::Effort {
        level: "minimal".into(),
    });
    assert_eq!(
        body(&request)["generationConfig"]["thinkingConfig"],
        json!({ "thinkingLevel": "minimal", "includeThoughts": true })
    );
}

#[test]
fn body_matches_generate_content() {
    let built = body(&request());
    assert_eq!(built["systemInstruction"]["parts"][0]["text"], "sys");
    assert_eq!(built["generationConfig"]["thinkingConfig"]["thinkingBudget"], 2048);
    let contents = built["contents"].as_array().unwrap();
    assert_eq!(contents[1]["role"], "model");
    assert_eq!(contents[1]["parts"][1]["functionCall"]["name"], "read");
    assert_eq!(contents[1]["parts"][0]["thoughtSignature"], "sig");
    assert_eq!(contents[2]["parts"][0]["functionResponse"]["name"], "read");
    assert_eq!(
        contents[2]["parts"][0]["functionResponse"]["response"]["output"],
        "1: x"
    );
    let declaration = &built["tools"][0]["functionDeclarations"][0];
    assert_eq!(declaration["parametersJsonSchema"]["additionalProperties"], false);
}

#[test]
fn raw_json_schema_keeps_numeric_enums_and_all_union_types() {
    let input = serde_json::json!({
        "type": "object",
        "properties": {
            "default": { "type": "string", "default": "x" },
            "examples": { "type": ["integer", "null"], "examples": [1] },
            "level": { "type": "integer", "enum": [1, 2] }
        },
        "required": ["default"]
    });
    let mut request = request();
    request.tools[0].input_schema = input.clone();

    let out = body(&request);
    assert_eq!(
        out["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"],
        input
    );
    assert!(crate::tool::schema::problems(&input, &json!({ "default":"x", "level":1, "examples":null })).is_empty());
    assert!(!crate::tool::schema::problems(&input, &json!({ "default":"x", "level":"1" })).is_empty());

    let union = json!({ "type": ["string", "number", "null"] });
    request.tools[0].input_schema = union.clone();
    assert_eq!(
        body(&request)["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"],
        union
    );
}

#[test]
fn stream_signatures_stay_on_the_function_call() {
    let mut state = StreamState::default();
    let feed = |state: &mut StreamState, json: &str| state.chunks(json).unwrap();
    assert_eq!(
        feed(
            &mut state,
            r#"{"candidates":[{"content":{"parts":[{"text":"th","thought":true}]}}]}"#
        ),
        vec![Chunk::ReasoningStart, Chunk::ReasoningDelta("th".into())]
    );

    let mut call = feed(
        &mut state,
        &json!({
            "candidates": [{
                "content": { "parts": [{
                    "functionCall": { "name": "read", "args": { "path": "a" } },
                    "thoughtSignature": "sig"
                }] },
                "finishReason": "STOP"
            }],
            "usageMetadata": { "promptTokenCount": 10, "candidatesTokenCount": 3, "thoughtsTokenCount": 4 }
        })
        .to_string(),
    );
    let Chunk::ToolUseStart { id, .. } = &mut call[1] else {
        panic!("{call:?}")
    };
    assert!(
        id.starts_with("call_") && id.len() > "call_1".len(),
        "a missing id is an engine id, unique across streams: {id}"
    );
    *id = "call_1".into();
    assert_eq!(
        call,
        vec![
            Chunk::BlockStop,
            Chunk::ToolUseStart {
                id: "call_1".into(),
                name: "read".into()
            },
            Chunk::ToolInputDelta(r#"{"path":"a"}"#.into()),
            Chunk::PartSignature("sig".into()),
            Chunk::BlockStop,
            Chunk::Usage(Usage {
                input: 10,
                output: 7,
                cache_read: 0,
                cache_write: 0
            }),
            Chunk::Stop(StopReason::ToolUse),
        ]
    );

    let mut plain = StreamState::default();
    assert_eq!(
        feed(&mut plain, r#"{"candidates":[{"content":{"parts":[{"text":"Hi"}]}}]}"#),
        vec![Chunk::TextStart, Chunk::TextDelta("Hi".into())]
    );
    assert_eq!(
        feed(
            &mut plain,
            r#"{"candidates":[{"content":{"parts":[]},"finishReason":"MAX_TOKENS"}]}"#
        ),
        vec![Chunk::BlockStop, Chunk::Stop(StopReason::MaxTokens)]
    );

    assert!(matches!(
        StreamState::default().chunks(r#"{"error":{"status":"UNAVAILABLE","message":"x"}}"#),
        Err(Error::Api { retryable: true, .. })
    ));
    assert!(matches!(
        StreamState::default().chunks(r#"{"error":{"status":"INVALID_ARGUMENT","message":"x"}}"#),
        Err(Error::Api { retryable: false, .. })
    ));
}

#[test]
fn a_call_signature_without_thought_text_round_trips_on_its_own_part() {
    let signed = json!({
        "candidates": [{
            "content": { "parts": [{
                "functionCall": { "name": "read", "args": { "path": "a" } },
                "thoughtSignature": "call-sig"
            }] },
            "finishReason": "STOP"
        }]
    });
    let chunks = StreamState::default().chunks(&signed.to_string()).unwrap();
    assert!(chunks.contains(&Chunk::PartSignature("call-sig".into())));
    assert!(
        !chunks
            .iter()
            .any(|chunk| matches!(chunk, Chunk::ReasoningSignature(_) | Chunk::ReasoningStart))
    );
    let sent = content(
        &ChatMessage {
            role: Role::Assistant,
            blocks: vec![
                Block::Text("before".into()),
                Block::Signed {
                    part: Box::new(Block::ToolUse {
                        id: "call_1".into(),
                        name: "read".into(),
                        input: json!({ "path":"a" }),
                    }),
                    signature: "call-sig".into(),
                },
            ],
        },
        &mut HashMap::new(),
    );
    assert!(sent["parts"][0].get("thoughtSignature").is_none());
    assert_eq!(sent["parts"][1]["thoughtSignature"], "call-sig");
    assert_eq!(sent["parts"][1]["functionCall"]["name"], "read");
}
