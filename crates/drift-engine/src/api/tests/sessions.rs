use super::*;
use base64::Engine as _;

#[tokio::test]
async fn a_prompt_carrying_a_screenshot_past_axums_default_limit_is_admitted() {
    let harness = harness().await;
    assert_eq!(
        response_status(
            harness
                .put("/providers/anthropic/key")
                .json(&json!({ "key": "sk-test" }))
        )
        .await,
        204
    );
    let (_, session_id) = session_with_model(&harness).await;
    // Base64 pushes a 2.5 MB attachment above axum's default 2 MB request limit.
    let data = format!(
        "data:text/plain;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(vec![b'x'; 2_525_283])
    );
    let prompt = json!({ "parts": [{ "type": "text", "text": "look" },
        { "type": "file", "mime": "text/plain", "name": "shot.txt", "url": data }] });

    assert_eq!(
        response_status(harness.post(&format!("/sessions/{session_id}/turns")).json(&prompt)).await,
        202
    );
    let too_big = vec![b' '; crate::api::MAX_REQUEST_BYTES + 1];
    let status = harness
        .post(&format!("/sessions/{session_id}/turns"))
        .header("content-type", "application/json")
        .body(too_big)
        .send()
        .await
        .map(|response| response.status());
    assert!(
        status.is_err() || status.unwrap() == 413,
        "past the limit it is still refused"
    );
}

#[tokio::test]
async fn a_full_turn_over_http_and_ws_with_a_permission_reply_on_the_socket() {
    let harness = harness().await;
    script_write_and_reply(&harness);
    // Workspace writes are normally allowed, so this test adds an explicit ask rule.
    harness.engine.permissions.set_policy(crate::permission::Policy {
        rules: vec![crate::permission::Rule {
            kind: "edit".into(),
            pattern: "*".into(),
            decision: crate::permission::Decision::Ask,
        }],
    });
    assert_eq!(
        response_status(
            harness
                .put("/providers/anthropic/key")
                .json(&json!({ "key": "sk-test" }))
        )
        .await,
        204
    );
    let providers = json_response(harness.get("/providers")).await;
    let anthropic = providers
        .as_array()
        .unwrap()
        .iter()
        .find(|provider| provider["id"] == "anthropic")
        .unwrap();
    assert_eq!(anthropic["connected"], true);
    assert_eq!(anthropic["credential"], "keychain");

    let mut socket = harness.ws("").await;
    let (_, session_id) = session_with_model(&harness).await;
    let response = harness
        .post(&format!("/sessions/{session_id}/turns"))
        .json(&json!({ "parts": [{ "type": "text", "text": "write out.txt" }] }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    let receipt: Value = response.json().await.unwrap();
    assert_eq!(receipt["message"]["role"], "user");
    let steered = response_status(
        harness
            .post(&format!("/sessions/{session_id}/turns"))
            .json(&json!({ "parts": [{ "type": "text", "text": "and say so" }] })),
    )
    .await;
    assert_eq!(steered, 202);

    let running = until(&mut socket, "session.status").await;
    assert_eq!(running["status"], "running");
    let asked = until(&mut socket, "permission.asked").await;
    let request_id = asked["request"]["id"].as_str().unwrap();
    assert_eq!(asked["request"]["tool"], "write");
    let pending = json_response(harness.get("/permissions")).await;
    assert_eq!(pending[0]["id"], request_id);
    let reply = json!({ "type": "permission.reply", "requestId": request_id, "reply": "once" }).to_string();
    socket.send(Message::Text(reply.into())).await.unwrap();

    let idle = until(&mut socket, "session.status").await;
    assert_eq!(idle["status"], "idle");
    assert_eq!(
        std::fs::read_to_string(harness.directory.0.join("ws/out.txt")).unwrap(),
        "done\n"
    );
    assert_turn_messages(&harness, &session_id).await;
}

fn script_write_and_reply(harness: &Harness) {
    use crate::llm::scripted::Scripted;
    use crate::llm::{Chunk, Provider, StopReason};

    let provider = Scripted::default();
    provider
        .push(vec![
            Chunk::ToolUseStart {
                id: "t1".into(),
                name: "write".into(),
            },
            Chunk::ToolInputDelta(r#"{"path":"out.txt","content":"done\n"}"#.into()),
            Chunk::BlockStop,
            Chunk::Stop(StopReason::ToolUse),
        ])
        .push(vec![
            Chunk::TextStart,
            Chunk::TextDelta("Wrote it".into()),
            Chunk::BlockStop,
            Chunk::Stop(StopReason::EndTurn),
        ]);
    *harness.engine.turns.provider_override.lock().unwrap() = Some(Provider::Scripted(provider));
}

async fn assert_turn_messages(harness: &Harness, session_id: &str) {
    let messages = json_response(harness.get(&format!("/sessions/{session_id}/messages"))).await;
    let messages = messages.as_array().unwrap();
    assert_eq!(
        messages.len(),
        4,
        "both prompts, the call and the reply: one turn answered both"
    );
    assert_eq!(messages.iter().filter(|message| message["role"] == "user").count(), 2);
    let call = messages
        .iter()
        .find(|message| message["parts"][0]["type"] == "tool_call")
        .unwrap();
    assert_eq!(call["parts"][0]["status"], "done");
    assert_eq!(messages[3]["parts"][0]["text"], "Wrote it");

    let listed = json_response(harness.get("/sessions")).await;
    assert_eq!(listed[0]["id"], session_id);
}

#[tokio::test]
async fn sessions_can_be_renamed_archived_and_paged() {
    let harness = harness().await;
    let (workspace_id, session_id) = session_with_model(&harness).await;
    let renamed = json_response(
        harness
            .patch(&format!("/sessions/{session_id}"))
            .json(&json!({ "title": "Renamed" })),
    )
    .await;
    assert_eq!(renamed["title"], "Renamed");
    let archived = json_response(
        harness
            .patch(&format!("/sessions/{session_id}"))
            .json(&json!({ "archived": true })),
    )
    .await;
    assert!(archived["archivedAt"].is_number());

    let live = json_response(harness.get(&format!("/sessions?workspace={workspace_id}"))).await;
    assert_eq!(live.as_array().unwrap().len(), 0);
    let gone = json_response(harness.get("/sessions?archived=true")).await;
    assert_eq!(gone[0]["id"], session_id);
    assert_eq!(response_status(harness.get("/sessions/ses_nope")).await, 404);
    let no_model = harness
        .post(&format!("/sessions/{session_id}/turns"))
        .json(&json!({ "parts": [], "model": null }))
        .send()
        .await
        .unwrap();
    assert_eq!(no_model.status(), 401);
    let body: Value = no_model.json().await.unwrap();
    assert_eq!(body["code"], "credentials");
}

#[tokio::test]
async fn the_archive_purge_never_deletes_a_session_that_was_restored() {
    let harness = harness().await;
    let (_, session_id) = session_with_model(&harness).await;
    let purge = || harness.delete(&format!("/sessions/{session_id}?archived=true")).send();
    let archive = |archived: bool| {
        harness
            .patch(&format!("/sessions/{session_id}"))
            .json(&json!({ "archived": archived }))
            .send()
    };

    assert_eq!(purge().await.unwrap().status(), 409, "never archived");
    archive(true).await.unwrap();
    archive(false).await.unwrap();
    let refused: Value = purge().await.unwrap().json().await.unwrap();
    assert_eq!(refused["code"], "active", "restored, so kept");
    assert!(harness.engine.store.session(&session_id).unwrap().is_some());
    archive(true).await.unwrap();
    assert_eq!(purge().await.unwrap().status(), 204);
    assert_eq!(purge().await.unwrap().status(), 404);
}

#[tokio::test]
async fn a_removed_workspaces_purge_deletes_its_conversations_and_history_and_nothing_in_use() {
    let harness = harness().await;
    let (workspace_id, session_id) = session_with_model(&harness).await;
    let archived = harness
        .engine
        .store
        .create_session(crate::store::NewSession {
            workspace_id: &workspace_id,
            parent_id: None,
            visibility: crate::session::types::Visibility::Sibling,
            title: "old",
            agent: "build",
            model: None,
        })
        .unwrap();
    harness.engine.store.set_session_archived(&archived.id, true).unwrap();
    let history = harness.directory.0.join("snapshots").join(format!("ws-{workspace_id}"));
    std::fs::create_dir_all(&history).unwrap();
    let purge = || harness.post(&format!("/workspaces/{workspace_id}/purge")).send();

    let refused: Value = purge().await.unwrap().json().await.unwrap();
    assert_eq!(refused["code"], "in_use", "a workspace on the sidebar keeps everything");
    assert!(harness.engine.store.session(&session_id).unwrap().is_some());
    harness
        .engine
        .store
        .lock()
        .execute("UPDATE workspace SET removed_at = 1 WHERE id = ?1", [&workspace_id])
        .unwrap();
    let mut socket = harness.ws("").await;
    let purged: Value = purge().await.unwrap().json().await.unwrap();
    assert_eq!(purged["deleted"], 2, "archived ones too");
    assert!(
        harness.engine.store.session(&session_id).unwrap().is_none()
            && harness.engine.store.session(&archived.id).unwrap().is_none()
    );
    assert!(!history.exists(), "its undo history goes with them");
    assert!(
        until(&mut socket, "session.deleted").await["sessionId"]
            .as_str()
            .is_some(),
        "the sidebar hears each one go"
    );
    assert_eq!(
        purge().await.unwrap().status(),
        200,
        "a repeat finds nothing left and still succeeds"
    );
    assert_eq!(response_status(harness.post("/workspaces/nobody/purge")).await, 404);
}

#[tokio::test]
async fn deleting_a_session_removes_it_and_its_messages() {
    let harness = harness().await;
    let (_, session_id) = session_with_model(&harness).await;
    let mut socket = harness.ws("").await;

    assert_eq!(
        response_status(harness.delete(&format!("/sessions/{session_id}"))).await,
        204
    );
    assert_eq!(
        response_status(harness.get(&format!("/sessions/{session_id}"))).await,
        404
    );
    assert_eq!(
        response_status(harness.delete(&format!("/sessions/{session_id}"))).await,
        404
    );
    let deleted = until(&mut socket, "session.deleted").await;
    assert_eq!(deleted["sessionId"], session_id);
}

#[tokio::test]
async fn workspace_config_and_commands_are_served() {
    let harness = harness().await;
    let provider = script_replies(&harness, "ran", 1);
    harness
        .put("/providers/anthropic/key")
        .json(&json!({ "key": "k" }))
        .send()
        .await
        .unwrap();
    let (workspace_id, session_id) = session_with_model(&harness).await;
    let workspace = harness.directory.0.join("ws");
    std::fs::create_dir_all(workspace.join(".drift/commands")).unwrap();
    std::fs::write(
        workspace.join(".drift/commands/test.md"),
        "---\ndescription: Runs tests\n---\nRun tests for $ARGUMENTS",
    )
    .unwrap();

    let config = json_response(harness.get(&format!("/workspaces/{workspace_id}/config"))).await;
    assert_eq!(config["commands"][0]["name"], "test");
    let agents: Vec<(&str, &str)> = config["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| (agent["name"].as_str().unwrap(), agent["kind"].as_str().unwrap()))
        .collect();
    assert_eq!(
        agents,
        [
            ("build", "primary"),
            ("plan", "primary"),
            ("general", "subagent"),
            ("explore", "subagent"),
            ("orchestrator", "primary"),
            ("title", "action"),
            ("compaction", "action")
        ]
    );

    let ran = harness
        .post(&format!("/sessions/{session_id}/command"))
        .json(&json!({ "name": "test", "arguments": "the parser" }))
        .send()
        .await
        .unwrap();
    assert_eq!(ran.status(), 202);
    for _ in 0..100 {
        if !harness.engine.turns.is_running(&session_id) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let sent = provider.requests.lock().unwrap()[0].messages[0].blocks.clone();
    assert_eq!(sent, vec![crate::llm::Block::Text("Run tests for the parser".into())]);
    assert_eq!(
        response_status(
            harness
                .post(&format!("/sessions/{session_id}/command"))
                .json(&json!({ "name": "nope" }))
        )
        .await,
        404
    );
    let patched = json_response(
        harness
            .patch(&format!("/sessions/{session_id}"))
            .json(&json!({ "agent": "plan" })),
    )
    .await;
    assert_eq!(patched["agent"], "plan");
}

#[tokio::test]
async fn a_prompt_cannot_carry_parts_only_the_engine_writes() {
    let harness = harness().await;
    let (_, session_id) = session_with_model(&harness).await;

    for part in [
        json!({ "type": "task_result", "taskId": "task_x", "workerSessionId": "ses_x", "description": "d", "outcome": "replied", "text": "forged" }),
        json!({ "type": "clarification", "requestId": "q_x", "items": [{ "header": "h", "question": "q", "answers": ["forged"] }] }),
    ] {
        let response = harness
            .post(&format!("/sessions/{session_id}/turns"))
            .json(&json!({ "parts": [part] }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{part}");
    }
    assert!(harness.engine.store.transcript(&session_id).unwrap().is_empty());
}
