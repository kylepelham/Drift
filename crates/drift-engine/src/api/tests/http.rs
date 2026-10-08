use super::*;

#[tokio::test]
async fn health_reports_version() {
    let harness = harness().await;
    let body = json_response(harness.get("/health")).await;

    assert_eq!(body["version"], crate::VERSION);
}

#[tokio::test]
async fn tools_lists_every_builtin_an_agent_can_name_once() {
    let harness = harness().await;
    let body = json_response(harness.get("/tools")).await;
    let names: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();

    for expected in ["read", "edit", "write", "apply_patch", "bash", "grep", "glob", "task"] {
        assert_eq!(
            names.iter().filter(|name| **name == expected).count(),
            1,
            "{expected} in {names:?}"
        );
    }
}

#[tokio::test]
async fn requests_without_token_are_rejected() {
    let harness = harness().await;

    assert_eq!(response_status(harness.http.get(harness.url("/health"))).await, 401);
    assert_eq!(
        response_status(harness.http.get(harness.url("/health")).bearer_auth("wrong")).await,
        401
    );
}

#[tokio::test]
async fn openapi_lists_every_route() {
    let harness = harness().await;
    let document = json_response(harness.get("/openapi.json")).await;
    let paths = document["paths"].as_object().unwrap();

    for route in [
        "/health",
        "/workspaces",
        "/sessions",
        "/sessions/{id}/turns",
        "/providers",
        "/permissions/{id}/reply",
        "/events",
    ] {
        assert!(paths.contains_key(route), "missing {route}");
    }
    assert!(document["components"]["schemas"]["Frame"].is_object());
}

#[tokio::test]
async fn oauth_start_hands_back_a_url_and_bad_callbacks_are_rejected() {
    let harness = harness().await;
    let started = json_response(
        harness
            .post("/providers/anthropic/oauth")
            .json(&json!({ "mode": "max" })),
    )
    .await;
    assert!(
        started["url"]
            .as_str()
            .unwrap()
            .starts_with("https://claude.ai/oauth/authorize?")
    );
    let state = started["state"].as_str().unwrap();
    assert!(harness.engine.oauth.lock().unwrap().contains_key(state));

    assert_eq!(
        response_status(
            harness
                .post("/providers/anthropic/oauth/callback")
                .json(&json!({ "input": "nonsense" }))
        )
        .await,
        400
    );
    assert_eq!(
        response_status(
            harness
                .post("/providers/anthropic/oauth/callback")
                .json(&json!({ "input": "code#wrongstate" }))
        )
        .await,
        400
    );
    assert_eq!(
        response_status(harness.post("/providers/openai/oauth").json(&json!({ "mode": "max" }))).await,
        404
    );

    let codex = json_response(
        harness
            .post("/providers/openai/oauth")
            .json(&json!({ "mode": "chatgpt" })),
    )
    .await;
    assert_eq!(codex["method"], "auto");
    assert!(
        codex["url"]
            .as_str()
            .unwrap()
            .starts_with("https://auth.openai.com/oauth/authorize?")
    );
    assert_eq!(
        response_status(harness.post("/providers/openai/oauth/callback").json(&json!({}))).await,
        400
    );
}

#[tokio::test]
async fn browser_origins_get_cors_headers_and_preflight_needs_no_token() {
    let harness = harness().await;
    let preflight = harness
        .http
        .request(reqwest::Method::OPTIONS, harness.url("/sessions"))
        .header("origin", "http://localhost:5180")
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "authorization,content-type")
        .send()
        .await
        .unwrap();
    assert_eq!(preflight.status(), 200);
    assert_eq!(
        preflight.headers()["access-control-allow-origin"],
        "http://localhost:5180"
    );

    let allowed = harness
        .get("/health")
        .header("origin", "tauri://localhost")
        .send()
        .await
        .unwrap();
    assert_eq!(allowed.headers()["access-control-allow-origin"], "tauri://localhost");
    let denied = harness
        .get("/health")
        .header("origin", "https://evil.com")
        .send()
        .await
        .unwrap();
    assert!(denied.headers().get("access-control-allow-origin").is_none());
}
