use super::*;
use crate::llm::catalog::ToolProfile;
use crate::mcp::ServerConfig;

#[tokio::test]
async fn mcp_servers_connect_as_soon_as_they_are_saved_and_their_tools_reach_the_model() {
    let harness = harness().await;
    let mut socket = harness.ws("").await;
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/echo-server.cjs");
    let directory = harness.directory.0.join("ws");
    std::fs::create_dir_all(&directory).unwrap();
    let workspace = harness
        .engine
        .store
        .add_workspace(&directory.to_string_lossy(), "ws", "")
        .unwrap();
    let here = crate::tool::canonical(&directory);

    let saved = json_response(
        harness
            .put(&format!("/mcp/echo?workspace={}", workspace.id))
            .json(&json!({ "type": "stdio", "command": "node", "args": [script] })),
    )
    .await;
    assert_eq!(saved["state"], "connected", "nothing to approve: {}", saved["error"]);
    assert_eq!(saved["tools"][0]["name"], "echo");
    assert!(saved.get("hash").is_none() && saved.get("approved").is_none());
    assert_eq!(until(&mut socket, "mcp.updated").await["server"]["name"], "echo");
    let names: Vec<String> = harness
        .engine
        .tool_specs(ToolProfile::Edit, Some(&here))
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    assert!(names.contains(&"echo_shout".to_string()));
    assert!(
        !harness
            .engine
            .tool_specs(ToolProfile::Edit, None)
            .iter()
            .any(|spec| spec.name == "echo_shout"),
        "a stdio server serves only the workspace it runs in"
    );

    harness
        .engine
        .mcp
        .disconnect("echo", &harness.engine.store, &harness.engine.hub)
        .await;
    let back = json_response(harness.post("/mcp/echo/connect")).await;
    assert_eq!(
        back["state"], "connected",
        "with no workspace named, it connects again where it ran"
    );
    harness
        .put("/mcp/fresh")
        .json(&json!({ "type": "stdio", "command": "node", "args": [script] }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response_status(harness.post("/mcp/fresh/connect")).await,
        409,
        "a stdio server running nowhere needs a workspace"
    );
    harness.delete("/mcp/fresh").send().await.unwrap();

    let listed = json_response(harness.get("/mcp")).await;
    assert_eq!(listed[0]["state"], "connected");
    let off = json_response(harness.put("/mcp/echo/enabled").json(&json!({ "enabled": false }))).await;
    assert_eq!(off["state"], "disabled");
    assert!(
        !harness
            .engine
            .tool_specs(ToolProfile::Edit, Some(&here))
            .iter()
            .any(|spec| spec.name == "echo_shout")
    );
    assert_eq!(response_status(harness.delete("/mcp/echo")).await, 204);
    assert_eq!(
        response_status(
            harness
                .put("/mcp/bad name")
                .json(&json!({ "type": "stdio", "command": "x" }))
        )
        .await,
        400
    );
}

#[tokio::test]
async fn mcp_secrets_go_in_but_never_out_and_no_save_or_rename_replaces_another_server() {
    let harness = harness().await;
    let save = |name: &str, query: &str, body: Value| harness.put(&format!("/mcp/{name}{query}")).json(&body).send();
    // Closed local ports and missing programs make connection failures immediate.
    let secret = json!({ "type": "http", "url": "http://127.0.0.1:9/mcp", "headers": { "Authorization": "Bearer secret-token" } });
    let saved = save("docs", "?create=true", secret.clone())
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        saved.contains("Authorization") && !saved.contains("secret-token"),
        "{saved}"
    );
    let listed = harness.get("/mcp").send().await.unwrap().text().await.unwrap();
    assert!(!listed.contains("secret-token"));
    assert_eq!(
        save("docs", "?create=true", secret.clone()).await.unwrap().status(),
        409,
        "adding never replaces"
    );

    let kept = json!({ "type": "http", "url": "http://127.0.0.1:9/v2", "headers": { "Authorization": null } });
    assert_eq!(save("docs", "", kept).await.unwrap().status(), 200);
    let ServerConfig::Http { headers, .. } = harness.engine.store.mcp_server("docs").unwrap().unwrap().config else {
        panic!()
    };
    assert_eq!(
        headers["Authorization"], "Bearer secret-token",
        "a null value keeps the saved secret"
    );
    let unknown = json!({ "type": "http", "url": "http://127.0.0.1:9", "headers": { "X-Key": null } });
    let refused: Value = save("docs", "", unknown).await.unwrap().json().await.unwrap();
    assert_eq!(refused["code"], "secret");

    save(
        "other",
        "",
        json!({ "type": "stdio", "command": "definitely-not-a-program" }),
    )
    .await
    .unwrap();
    let rename = |from: &str, to: &str| {
        harness
            .post(&format!("/mcp/{from}/rename"))
            .json(&json!({ "to": to }))
            .send()
    };
    let onto_taken: Value = rename("docs", "other").await.unwrap().json().await.unwrap();
    assert_eq!(onto_taken["code"], "taken");
    assert!(
        matches!(
            harness.engine.store.mcp_server("other").unwrap().unwrap().config,
            ServerConfig::Stdio { .. }
        ),
        "the other server is untouched"
    );
    assert_eq!(rename("docs", "docs2").await.unwrap().status(), 200);
    assert!(harness.engine.store.mcp_server("docs").unwrap().is_none());
    let ServerConfig::Http { headers, .. } = harness.engine.store.mcp_server("docs2").unwrap().unwrap().config else {
        panic!()
    };
    assert_eq!(
        headers["Authorization"], "Bearer secret-token",
        "the secret moves with it"
    );
}
