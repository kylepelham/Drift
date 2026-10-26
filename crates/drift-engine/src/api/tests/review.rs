use super::*;

#[tokio::test]
async fn review_routes_answer_for_a_workspace_and_a_turn() {
    let h = harness().await;
    let (workspace_id, session_id) = session_with_model(&h).await;

    let refs = json_response(h.get(&format!("/workspaces/{workspace_id}/review/refs"))).await;
    assert_eq!(refs["repo"], false, "{refs}");

    let status = response_status(h.get(&format!("/workspaces/{workspace_id}/review?source=uncommitted"))).await;
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
    let files = json_response(h.get(&format!(
        "/workspaces/{workspace_id}/review?source=files&target=missing.rs"
    )))
    .await;
    assert_eq!(files, json!({ "files": [], "omitted": 0 }), "no repository needed");
    let outside = response_status(h.get(&format!(
        "/workspaces/{workspace_id}/review?source=files&target=..%2Fx.rs"
    )))
    .await;
    assert_eq!(outside, reqwest::StatusCode::BAD_REQUEST);
    let missing = response_status(h.get("/workspaces/nope/review/refs")).await;
    assert_eq!(missing, reqwest::StatusCode::NOT_FOUND);

    script_replies(&h, "done", 1);
    h.put("/providers/anthropic/key")
        .json(&json!({ "key": "k" }))
        .send()
        .await
        .unwrap();
    submit_and_wait(&h, &session_id, "hello").await;
    let messages = json_response(h.get(&format!("/sessions/{session_id}/messages"))).await;
    let prompt = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "user")
        .unwrap_or_else(|| panic!("{messages}"));
    let review = json_response(h.get(&format!(
        "/sessions/{session_id}/review/{}",
        prompt["id"].as_str().unwrap()
    )))
    .await;
    assert_eq!(review, json!({ "files": [], "omitted": 0 }));
}
