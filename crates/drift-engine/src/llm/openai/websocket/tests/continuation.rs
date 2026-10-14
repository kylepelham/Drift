//! Continuing the previous response on the same socket, and starting over whenever that would be wrong.

use super::*;

#[tokio::test]
async fn a_continued_conversation_reuses_its_socket_and_sends_only_new_input() {
    let h = fixture(Server::default(), None).await;

    let first = collect(&h.provider, &request(), &key()).await;
    collect(&h.provider, &continued(request()), &key()).await;

    assert!(
        first
            .iter()
            .any(|chunk| matches!(chunk, Ok(Chunk::TextDelta(text)) if text == "ok"))
    );
    assert_eq!((h.server.connections(), h.server.posts()), (1, 0));

    let (opening, next) = (h.server.body(0), h.server.body(1));
    assert_eq!(opening["type"], "response.create");
    assert_eq!(opening["store"], false);
    assert!(
        opening.get("stream").is_none(),
        "a WebSocket request carries no transport fields"
    );
    assert_eq!(next["previous_response_id"], "resp_1");
    assert_eq!(
        next["input"].as_array().unwrap().len(),
        1,
        "only the user's new message"
    );
}

#[tokio::test]
async fn a_tool_result_continues_without_replaying_the_call_and_keeps_its_mode() {
    let h = fixture(
        Server {
            tool_first: true,
            ..Server::default()
        },
        None,
    )
    .await;

    // Ultrafast and Daybreak ride in the body, so they must survive the shortened request.
    let mut request = request();
    let body = json!({ "service_tier": "ultrafast", "access_programs": { "cyber": "daybreak_blue" } });
    request.mode = Some(crate::llm::catalog::ModelMode {
        name: "ultrafast-daybreak".into(),
        base: request.model.clone(),
        body: body.as_object().unwrap().clone(),
        headers: Default::default(),
    });
    collect(&h.provider, &request, &key()).await;

    request.messages.push(ChatMessage {
        role: Role::Assistant,
        blocks: vec![Block::ToolUse {
            id: "call".into(),
            name: "read".into(),
            input: json!({ "path": "a.txt" }),
        }],
    });
    request.messages.push(ChatMessage {
        role: Role::User,
        blocks: vec![Block::ToolResult {
            call_id: "call".into(),
            content: "contents".into(),
            is_error: false,
        }],
    });
    collect(&h.provider, &request, &key()).await;

    let next = h.server.body(1);
    assert_eq!(next["previous_response_id"], "resp_1");
    assert_eq!(next["input"].as_array().unwrap().len(), 1);
    assert_eq!(next["input"][0]["type"], "function_call_output");
    assert_eq!(next["service_tier"], "ultrafast");
    assert_eq!(next["access_programs"]["cyber"], "daybreak_blue");
}

#[tokio::test]
async fn changed_settings_or_compacted_history_start_a_new_chain_on_the_same_socket() {
    let h = fixture(Server::default(), None).await;

    let mut changed = continued(request());
    changed.system = "new instructions".into();

    collect(&h.provider, &request(), &key()).await;
    collect(&h.provider, &changed, &key()).await;
    collect(&h.provider, &request(), &key()).await;

    assert_eq!(h.server.connections(), 1);
    assert!(h.server.body(1).get("previous_response_id").is_none());
    assert_eq!(h.server.body(1)["input"].as_array().unwrap().len(), 3);
    assert!(
        h.server.body(2).get("previous_response_id").is_none(),
        "a shorter history cannot continue"
    );
}

#[tokio::test]
async fn a_forgotten_previous_response_is_replayed_whole_once_on_the_same_socket() {
    let h = fixture(Server::default(), None).await;

    collect(&h.provider, &request(), &key()).await;
    h.server.lose_previous.store(true, Ordering::SeqCst);
    let chunks = collect(&h.provider, &continued(request()), &key()).await;

    assert!(chunks.iter().all(Result::is_ok));
    assert_eq!((h.server.requests(), h.server.posts()), (3, 0));
    assert!(h.server.body(1).get("previous_response_id").is_some());
    assert!(h.server.body(2).get("previous_response_id").is_none());
    assert_eq!(h.server.body(2)["input"].as_array().unwrap().len(), 3);
}

#[test]
fn continuation_matches_replayed_reasoning_but_not_edits_or_changed_settings() {
    let first = json!({
        "model": "gpt-6-sol", "instructions": "instructions", "store": false,
        "input": [{ "role": "user", "content": [{ "type": "input_text", "text": "hi" }] }]
    });
    let reasoning = json!({
        "type": "reasoning", "id": "reason", "encrypted_content": "encrypted",
        "summary": [{ "type": "summary_text", "text": "first" }, { "type": "summary_text", "text": "second" }]
    });
    let response = json!({ "id": "resp", "output": [reasoning, message()] });
    let previous = cache::Previous::completed(&first, &response).unwrap();

    // Drift replays the server's output in its own shape: joined summaries, no ids or statuses.
    let mut next = first.clone();
    next["input"].as_array_mut().unwrap().extend([
        json!({ "type": "reasoning", "encrypted_content": "encrypted", "summary": [{ "type": "summary_text", "text": "first\n\nsecond" }] }),
        json!({ "role": "assistant", "content": [{ "type": "output_text", "text": "ok" }] }),
        json!({ "type": "function_call_output", "call_id": "new", "output": "result" }),
    ]);
    assert_eq!(cache::payload(&next, Some(&previous))["previous_response_id"], "resp");

    for field in ["model", "instructions", "service_tier", "tools"] {
        let mut changed = next.clone();
        changed[field] = json!("changed");
        let payload = cache::payload(&changed, Some(&previous));
        assert!(payload.get("previous_response_id").is_none(), "{field}");
    }

    next["input"][0]["content"][0]["text"] = json!("edited");
    assert!(
        cache::payload(&next, Some(&previous))
            .get("previous_response_id")
            .is_none()
    );
}
