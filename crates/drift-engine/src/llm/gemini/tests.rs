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
        connection: None,
        effort_marks: Vec::new(),
        claude: Default::default(),
        cache_hour: false,
        cache_conversation: true,
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
fn a_malformed_finish_is_a_retryable_error_and_other_odd_finishes_are_not() {
    for reason in ["MALFORMED_FUNCTION_CALL", "MALFORMED_RESPONSE"] {
        let mut state = StreamState::default();
        let _ = state.chunks(r#"{"candidates":[{"content":{"parts":[{"text":"half"}]}}]}"#);
        let finished = format!(r#"{{"candidates":[{{"content":{{"parts":[]}},"finishReason":"{reason}"}}]}}"#);
        let error = state.chunks(&finished).unwrap_err();
        assert!(
            matches!(&error, Error::Api { retryable: true, status: crate::llm::STREAMED, message, .. } if message.contains(reason)),
            "{error:?}"
        );
    }

    let other = StreamState::default()
        .chunks(r#"{"candidates":[{"content":{"parts":[]},"finishReason":"TOO_MANY_TOOL_CALLS"}]}"#)
        .unwrap();
    assert_eq!(other.last(), Some(&Chunk::Stop(StopReason::Other)));
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

#[test]
fn function_responses_travel_in_a_turn_of_their_own() {
    let mut request = request();
    request.messages[2].blocks.extend([
        Block::Text("The read call (call_1) returned this:".into()),
        Block::Image {
            mime: "image/png".into(),
            base64: "AAAA".into(),
        },
        Block::Text("and now the next prompt".into()),
    ]);
    let contents = body(&request)["contents"].as_array().unwrap().clone();
    assert_eq!(contents.len(), 4, "{contents:#?}");
    assert_eq!(contents[2]["role"], "user");
    assert_eq!(contents[2]["parts"].as_array().unwrap().len(), 1);
    assert!(contents[2]["parts"][0].get("functionResponse").is_some());
    assert_eq!(contents[3]["role"], "user");
    assert_eq!(contents[3]["parts"][0]["text"], "The read call (call_1) returned this:");
    assert_eq!(contents[3]["parts"][2]["text"], "and now the next prompt");
}

#[test]
fn explicit_nulls_in_a_streamed_part_are_absent_fields() {
    let mut state = StreamState::default();
    let text = state
        .chunks(r#"{"candidates":[{"content":{"parts":[{"text":"Hi","functionCall":null,"thoughtSignature":null}]},"finishReason":null}],"usageMetadata":null}"#)
        .unwrap();
    assert_eq!(text, vec![Chunk::TextStart, Chunk::TextDelta("Hi".into())]);

    let call = state
        .chunks(r#"{"candidates":[{"content":{"parts":[{"functionCall":{"id":"c","name":"read","args":null}}]}}]}"#)
        .unwrap();
    assert!(call.contains(&Chunk::ToolInputDelta("{}".into())), "{call:?}");
}

#[test]
fn a_blocked_prompt_is_a_refusal_not_a_stream_without_a_stop() {
    let blocked = StreamState::default()
        .chunks(r#"{"promptFeedback":{"blockReason":"PROHIBITED_CONTENT","blockReasonMessage":"Blocked."},"usageMetadata":{"promptTokenCount":9}}"#)
        .unwrap();
    assert_eq!(
        blocked[blocked.len() - 2..],
        [
            Chunk::Refusal(crate::session::types::Refusal {
                category: Some("PROHIBITED_CONTENT".into()),
                explanation: Some("Blocked.".into()),
                model: None,
            }),
            Chunk::Stop(StopReason::Refused),
        ]
    );
}

#[test]
fn a_call_id_repeated_within_one_reply_is_replaced_by_a_fresh_one() {
    let mut state = StreamState::default();
    let call = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"id":"dup","name":"read","args":{}}}]}}]}"#;
    let ids: Vec<String> = [state.chunks(call).unwrap(), state.chunks(call).unwrap()]
        .concat()
        .into_iter()
        .filter_map(|chunk| match chunk {
            Chunk::ToolUseStart { id, .. } => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(ids[0], "dup");
    assert!(ids[1] != "dup" && ids[1].starts_with("call_"), "{ids:?}");
}

#[test]
fn an_unsigned_call_replayed_to_gemini_3_carries_the_skip_signature() {
    let mut request = request();
    request.messages[1].blocks = vec![
        Block::ToolUse {
            id: "call_1".into(),
            name: "read".into(),
            input: json!({}),
        },
        Block::ToolUse {
            id: "call_2".into(),
            name: "read".into(),
            input: json!({}),
        },
    ];
    for model in ["gemini-3.6-flash", "gemini-3-pro-preview"] {
        request.model = model.into();
        let built = body(&request);
        assert_eq!(
            built["contents"][1]["parts"][0]["thoughtSignature"], "skip_thought_signature_validator",
            "{model}"
        );
    }
    request.model = "gemini-2.5-pro".into();
    assert!(
        body(&request)["contents"][1]["parts"][0]
            .get("thoughtSignature")
            .is_none()
    );

    request.model = "gemini-3.6-flash".into();
    request.messages[1].blocks[0] = Block::Signed {
        part: Box::new(request.messages[1].blocks[0].clone()),
        signature: "real".into(),
    };
    let built = body(&request);
    assert_eq!(built["contents"][1]["parts"][0]["thoughtSignature"], "real");
    assert!(
        built["contents"][1]["parts"][1].get("thoughtSignature").is_none(),
        "a parallel call after a signed one needs none"
    );
}

#[test]
fn an_invalid_api_key_is_refused_credentials() {
    let refused = api_error(
        400,
        r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT",
            "details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"API_KEY_INVALID"}]}}"#,
    );
    assert!(
        matches!(refused, Error::Unauthenticated(ref words) if words.starts_with("API key not valid")),
        "{refused:?}"
    );
    assert!(matches!(
        api_error(400, r#"{"error":{"message":"bad","status":"INVALID_ARGUMENT"}}"#),
        Error::Api { .. }
    ));
}
