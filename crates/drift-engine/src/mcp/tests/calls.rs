use super::*;

/// Logs every echo-server call and makes `crash-once` terminate only the first process.
fn logged_echo() -> (ServerConfig, std::path::PathBuf) {
    let ServerConfig::Stdio { command, args, .. } = echo_config() else {
        unreachable!()
    };
    let directory = std::env::temp_dir().join(format!("drift-mcp-log-{}", crate::random_hex(4)));
    std::fs::create_dir_all(&directory).unwrap();
    let env = [
        ("CALL_LOG", directory.join("calls")),
        ("CRASH_MARKER", directory.join("crashed")),
    ]
    .map(|(key, value)| (key.to_string(), value.to_string_lossy().into_owned()));

    (
        ServerConfig::Stdio {
            command,
            args,
            env: env.into(),
            cwd: None,
            timeout_seconds: None,
        },
        directory.join("calls"),
    )
}

#[tokio::test]
async fn a_running_turns_tool_follows_a_reconnect_but_never_replays_a_call_that_may_have_landed() {
    let engine = engine();
    let (config, log) = logged_echo();
    let row = saved(&engine, "echo", &config).await;
    engine.connect_mcp_in("echo", Some(&here())).await.unwrap();
    let context = context(&engine);
    let shout = tool(&engine, "echo_shout");

    let lost = shout.run(&context, json!({ "text": "crash" })).await.unwrap_err().0;
    assert!(
        lost.contains("may or may not have taken effect") && lost.contains("not retried"),
        "{lost}"
    );
    until("it noticed", || {
        engine.mcp.status_of(row.clone()).state != State::Connected
    })
    .await;
    until("it is back", || {
        engine.mcp.status_of(row.clone()).state == State::Connected
    })
    .await;

    assert_eq!(
        calls(&log),
        ["shout crash"],
        "sent once, not replayed on the new process"
    );
    assert_eq!(
        shout.run(&context, json!({ "text": "hi" })).await.unwrap().output,
        "HI",
        "the captured tool reaches the reconnected server"
    );
}

#[tokio::test]
async fn a_read_only_call_cut_off_by_a_lost_connection_is_asked_again_once() {
    let engine = engine();
    let (config, log) = logged_echo();
    saved(&engine, "echo", &config).await;
    engine.connect_mcp_in("echo", Some(&here())).await.unwrap();

    let output = tool(&engine, "echo_echo")
        .run(&context(&engine), json!({ "text": "crash-once" }))
        .await
        .unwrap();
    assert_eq!(output.output, "crash-once", "answered by the reconnected server");
    assert_eq!(calls(&log), ["echo crash-once", "echo crash-once"]);
}

#[tokio::test]
async fn a_captured_tool_is_not_run_on_a_reconnected_server_that_redefined_it() {
    let engine = engine();
    let (
        ServerConfig::Stdio {
            command, args, mut env, ..
        },
        log,
    ) = logged_echo()
    else {
        unreachable!()
    };
    env.insert("REDEFINE_AFTER_CRASH".into(), "1".into());
    let config = ServerConfig::Stdio {
        command,
        args,
        env,
        cwd: None,
        timeout_seconds: None,
    };
    saved(&engine, "echo", &config).await;
    engine.connect_mcp_in("echo", Some(&here())).await.unwrap();
    let echo = tool(&engine, "echo_echo");

    let refused = echo
        .run(&context(&engine), json!({ "text": "crash-once" }))
        .await
        .unwrap_err()
        .0;
    assert!(refused.contains("changed its echo tool"), "{refused}");
    assert_eq!(
        calls(&log),
        ["echo crash-once"],
        "not asked again of a server that now calls it mutating"
    );
}

#[tokio::test]
async fn disabling_ends_calls_under_way_and_refuses_captured_tools_until_reenabled() {
    let engine = engine();
    let (config, log) = logged_echo();
    saved(&engine, "echo", &config).await;
    engine.connect_mcp_in("echo", Some(&here())).await.unwrap();
    let (echo, shout) = (tool(&engine, "echo_echo"), tool(&engine, "echo_shout"));
    let hanging = tokio::spawn({
        let (engine, shout) = (engine.clone(), shout.clone());
        async move {
            shout
                .run(&context(&engine), json!({ "text": "hang" }))
                .await
                .map(|output| output.output)
                .map_err(|error| error.0)
        }
    });
    until("the call reached the server", || calls(&log) == ["shout hang"]).await;

    engine
        .mcp
        .close("echo", &engine.store, &engine.hub, |store| {
            store.set_mcp_enabled("echo", false)
        })
        .await
        .unwrap();
    let ended = tokio::time::timeout(std::time::Duration::from_secs(1), hanging)
        .await
        .expect("the call ends at once")
        .unwrap();
    assert!(ended.unwrap_err().contains("disabled while the call ran"));
    let context = context(&engine);
    let refused = echo.run(&context, json!({ "text": "hi" })).await.unwrap_err().0;
    assert!(refused.contains("disabled or removed"), "a captured tool refuses");

    engine.store.set_mcp_enabled("echo", true).unwrap();
    engine.connect_mcp_in("echo", Some(&here())).await.unwrap();
    assert_eq!(
        tool(&engine, "echo_echo")
            .run(&context, json!({ "text": "back" }))
            .await
            .unwrap()
            .output,
        "back"
    );
    assert!(
        echo.run(&context, json!({ "text": "hi" })).await.is_err(),
        "re-enabling does not revive tools captured before the disable"
    );
}

#[tokio::test]
async fn a_save_leaves_running_turns_on_the_client_they_were_given() {
    let engine = engine();
    let (config, _log) = logged_echo();
    saved(&engine, "echo", &config).await;
    engine.connect_mcp_in("echo", Some(&here())).await.unwrap();
    let echo = tool(&engine, "echo_echo");
    let ServerConfig::Stdio {
        command, args, mut env, ..
    } = config
    else {
        unreachable!()
    };
    env.insert("CHANGED".into(), "1".into());
    let replacement = ServerConfig::Stdio {
        command,
        args,
        env,
        cwd: None,
        timeout_seconds: None,
    };

    engine
        .mcp
        .change("echo", &engine.store, &engine.hub, |store| {
            store.save_mcp_server("echo", &replacement)
        })
        .await
        .unwrap();
    assert!(
        find(&engine, "echo_echo").is_none(),
        "later turns see the server once it connects on the new command"
    );
    assert_eq!(
        echo.run(&context(&engine), json!({ "text": "still" }))
            .await
            .unwrap()
            .output,
        "still",
        "the running turn keeps its client"
    );
}
