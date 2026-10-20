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
    assert_eq!(
        requests.last().unwrap().system,
        requests[0].system,
        "the system prompt is frozen, and a server that is still switched on keeps its instructions while it reconnects"
    );
    assert!(
        !format!("{:?}", requests.last().unwrap().messages).contains("These instructions changed"),
        "a lost connection announces nothing"
    );
}

/// The echo server with ten more tools, connected for the harness's workspace.
async fn large_server(h: &Harness) {
    let mut server = echo_server();
    if let crate::mcp::ServerConfig::Stdio { env, .. } = &mut server {
        env.insert("MANY_TOOLS".into(), "10".into());
    }
    h.engine.store.save_mcp_server("echo", &server).unwrap();
    h.engine
        .connect_mcp_in("echo", Some(&crate::tool::canonical(&h._dir.join("ws"))))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_large_mcp_server_is_loaded_on_request_and_a_search_loads_its_tools() {
    let h = harness().await;
    large_server(&h).await;
    h.provider
        .push(tool_call("tool_search", r#"{"query": "select:echo_note_3"}"#))
        .push(tool_call("echo_note_3", r#"{"text": "filed"}"#))
        .push(text("done"));

    h.engine.submit(&h.session.id, prompt("file a note")).await.await_ok();
    until_idle(&h).await;

    let requests = h.provider.requests.lock().unwrap();
    let first = &requests[0];
    assert!(first.tools.iter().any(|tool| tool.name == "tool_search"));
    assert!(!first.tools.iter().any(|tool| tool.name.starts_with("echo_")));
    assert_eq!(first.deferred_tools.len(), 12);
    assert!(first.system.contains("- echo: echo_echo, echo_shout, echo_note_0"));
    assert_eq!(
        requests[1].tool_references.values().collect::<Vec<_>>(),
        [&vec!["echo_note_3".to_string()]]
    );
    drop(requests);

    let messages = transcript(&h);
    let ran = messages
        .iter()
        .flat_map(|message| message.parts.iter())
        .map(tool_view_of)
        .find(|(name, _)| name == "echo_note_3");
    assert_eq!(ran, Some(("echo_note_3".to_string(), Some("filed".to_string()))));
}

#[tokio::test]
async fn with_tool_search_off_every_tool_is_sent_up_front() {
    let h = harness().await;
    h.engine
        .store
        .set_setting(crate::tool::tool_search::SETTINGS_KEY, &false)
        .unwrap();
    large_server(&h).await;
    h.provider.push(text("done"));

    h.engine.submit(&h.session.id, prompt("file a note")).await.await_ok();
    until_idle(&h).await;

    let requests = h.provider.requests.lock().unwrap();
    assert!(requests[0].deferred_tools.is_empty());
    assert!(requests[0].tools.iter().any(|tool| tool.name == "echo_note_9"));
    assert!(!requests[0].tools.iter().any(|tool| tool.name == "tool_search"));
}

fn tool_view_of(row: &PartRow) -> (String, Option<String>) {
    match &row.part {
        Part::ToolCall { name, output, .. } => (name.clone(), output.clone()),
        _ => (String::new(), None),
    }
}
