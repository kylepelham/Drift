use super::*;

#[tokio::test]
async fn a_turn_keeps_the_tools_it_started_with_and_a_change_reaches_the_next_one() {
    let h = harness().await;
    h.engine.store.save_mcp_server("echo", &echo_server()).unwrap();
    h.engine
        .connect_mcp_in("echo", Some(&crate::tool::canonical(&h._dir.join("ws"))))
        .await
        .unwrap();
    h.provider
        .push_slow(
            Duration::from_millis(500),
            tool_call("echo_echo", r#"{"text": "still here"}"#),
        )
        .push(text("done"));
    h.engine.submit(&h.session.id, prompt("echo")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(150)).await;
    h.engine.mcp.disconnect("echo", &h.engine.store, &h.engine.hub).await;
    until_idle(&h).await;

    let messages = transcript(&h);
    let call = tool(&messages[1].parts[0]);
    assert_eq!(
        (call.status, call.output),
        (ToolStatus::Done, Some("still here")),
        "served by the client the turn began with"
    );
    h.provider.push(text("ok"));
    h.engine.submit(&h.session.id, prompt("again")).await.await_ok();
    until_idle(&h).await;
    let requests = h.provider.requests.lock().unwrap();
    assert!(requests[0].tools.iter().any(|tool| tool.name == "echo_echo"));
    assert!(
        requests[0]
            .system
            .contains("# Instructions from the echo MCP server\n\nEcho repeats what it is given."),
        "the server's instructions come with its tools"
    );
    assert!(
        !requests
            .last()
            .unwrap()
            .tools
            .iter()
            .any(|tool| tool.name == "echo_echo"),
        "the next turn sees the change"
    );
    assert!(
        !requests.last().unwrap().system.contains("echo MCP server"),
        "and its instructions go with them"
    );
}
