use super::*;

#[tokio::test]
async fn a_stdio_server_runs_where_it_is_told_and_a_call_past_its_timeout_fails() {
    let engine = engine();
    let ServerConfig::Stdio { command, args, .. } = echo_config() else {
        unreachable!()
    };
    let directory =
        crate::tool::canonical(&std::env::temp_dir().join(format!("drift-mcp-cwd-{}", crate::random_hex(4))));
    std::fs::create_dir_all(&directory).unwrap();
    let config = ServerConfig::Stdio {
        command,
        args,
        env: Default::default(),
        cwd: Some(directory.to_string_lossy().into_owned()),
        timeout_seconds: Some(1),
    };
    let row = saved(&engine, "echo", &config).await;
    engine.connect_mcp_in("echo", Some(&here())).await.unwrap();
    let context = context(&engine);
    let shout = tool(&engine, "echo_shout");

    let cwd = shout.run(&context, json!({ "text": "cwd" })).await.unwrap().output;
    let normalized = |path: &str| path.to_lowercase().replace('\\', "/").trim_end_matches('/').to_string();
    let actual = crate::tool::canonical(std::path::Path::new(&cwd));
    assert_eq!(
        normalized(&actual.to_string_lossy()),
        normalized(&directory.to_string_lossy())
    );

    let started = std::time::Instant::now();
    let hung = shout.run(&context, json!({ "text": "hang" })).await.unwrap_err().0;
    assert!(
        hung.contains("1s timeout") && started.elapsed() < std::time::Duration::from_secs(5),
        "{hung}"
    );
    let status = engine.mcp.status_of(row);
    assert_eq!(status.transport, Transport::Stdio);
    assert_eq!(
        status.protocol.as_deref(),
        Some("2025-06-18"),
        "the version the server agreed to"
    );
}

#[tokio::test]
async fn a_stdio_server_runs_in_each_workspace_that_uses_it_and_is_told_it_as_its_root() {
    let engine = engine();
    let row = saved(&engine, "echo", &echo_config()).await;
    let one = crate::tool::canonical(&std::env::temp_dir().join(format!("drift-mcp-ws1-{}", crate::random_hex(3))));
    let two = crate::tool::canonical(&std::env::temp_dir().join(format!("drift-mcp-ws2-{}", crate::random_hex(3))));
    for directory in [&one, &two] {
        std::fs::create_dir_all(directory).unwrap();
        engine.start_workspace_mcp(directory);
    }
    until("both connect", || {
        engine.mcp.lock().live(&Key::of("echo", Some(&one))).is_some()
            && engine.mcp.lock().live(&Key::of("echo", Some(&two))).is_some()
    })
    .await;
    assert_eq!(
        engine.mcp.status_of(row.clone()).state,
        State::Connected,
        "one row for the server"
    );

    let ask = |directory: &std::path::PathBuf, text: &'static str| {
        let tools = engine.mcp.tools(&engine.store, Some(directory));
        let echo = tools.into_iter().find(|tool| tool.spec().name == "echo_echo").unwrap();
        let context = Context {
            workspace: directory.clone(),
            ..context(&engine)
        };

        async move { echo.run(&context, json!({ "text": text })).await.unwrap().output }
    };
    let same =
        |actual: &str, expected: &std::path::Path| crate::tool::canonical(std::path::Path::new(actual)) == expected;
    assert!(
        same(&ask(&one, "cwd").await, &one) && same(&ask(&two, "cwd").await, &two),
        "each runs in its own workspace"
    );
    let roots = ask(&one, "roots").await;
    let uri = reqwest::Url::from_directory_path(&one).unwrap().to_string();
    assert!(roots.contains(&uri), "the workspace is its root: {roots}");

    assert!(engine.mcp.disconnect("echo", &engine.store, &engine.hub).await);
    engine.start_workspace_mcp(&one);
    assert!(
        !engine.mcp.connecting(),
        "a server the user disconnected stays disconnected until they connect it"
    );

    for directory in [one, two] {
        std::fs::remove_dir_all(directory).ok();
    }
}

#[tokio::test]
async fn a_workspace_never_borrows_another_place_s_stdio_connection_and_one_closed_and_idle_or_removed_stops() {
    let engine = engine();
    saved(&engine, "echo", &echo_config()).await;
    assert!(
        matches!(engine.connect_mcp("echo").await, Err(Error::NeedsWorkspace)),
        "no shared stdio connection, where Drift runs"
    );
    engine.connect_mcp_in("echo", Some(&here())).await.unwrap();
    let elsewhere =
        crate::tool::canonical(&std::env::temp_dir().join(format!("drift-mcp-other-{}", crate::random_hex(3))));
    assert!(
        engine.mcp.tools(&engine.store, Some(&elsewhere)).is_empty(),
        "another workspace waits for its own"
    );
    assert!(engine.mcp.tools(&engine.store, None).is_empty());

    engine
        .mcp
        .stop_idle(std::time::Duration::from_secs(60), &engine.store, &engine.hub);
    assert!(
        !engine.mcp.tools(&engine.store, Some(&here())).is_empty(),
        "just used, so kept"
    );
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    engine.mcp.set_open(7, Some(here()));
    engine
        .mcp
        .stop_idle(std::time::Duration::from_millis(100), &engine.store, &engine.hub);
    assert!(
        !engine.mcp.tools(&engine.store, Some(&here())).is_empty(),
        "a workspace a client has open keeps its servers however long unused"
    );

    engine.mcp.set_open(7, None);
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    engine
        .mcp
        .stop_idle(std::time::Duration::from_millis(100), &engine.store, &engine.hub);
    assert!(
        engine.mcp.tools(&engine.store, Some(&here())).is_empty(),
        "closed everywhere and left idle, it stopped"
    );
    let row = engine.store.mcp_server("echo").unwrap().unwrap();
    assert_eq!(engine.mcp.status_of(row).state, State::Disconnected);

    engine.start_workspace_mcp(&here());
    until("started again by the next use", || {
        engine.mcp.lock().live(&Key::of("echo", Some(&here()))).is_some()
    })
    .await;
    engine.mcp.stop_workspace(&here(), &engine.store, &engine.hub);
    assert!(
        engine.mcp.lock().live(&Key::of("echo", Some(&here()))).is_none(),
        "a removed workspace's servers stop"
    );
}
