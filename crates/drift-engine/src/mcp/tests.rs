use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::event::{Event, Hub};
use crate::tool::{Context, SessionFiles};

mod calls;
mod oauth;
mod protocol;
mod resources;
mod startup;
mod support;
mod transport;
mod workspace;

use support::*;

#[tokio::test]
async fn a_server_this_build_cannot_read_is_listed_failed_and_the_rest_still_connect() {
    let engine = engine();
    let hub = Hub::new(32);
    engine.store.save_mcp_server("echo", &echo_config()).unwrap();
    engine.store.save_mcp_server("newer", &echo_config()).unwrap();
    engine
        .store
        .lock()
        .execute(
            "UPDATE mcp_config SET config_json = '{\"type\":\"future\"}' WHERE name = 'newer'",
            [],
        )
        .unwrap();

    let statuses = engine.mcp.statuses(&engine.store).unwrap();
    assert_eq!(
        statuses
            .iter()
            .map(|s| (s.server.name.as_str(), s.state))
            .collect::<Vec<_>>(),
        [("echo", State::Disconnected), ("newer", State::Failed)]
    );
    assert!(statuses[1].unreadable && !statuses[0].unreadable);
    assert!(
        statuses[1]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("could not be read")),
        "{:?}",
        statuses[1].error
    );

    engine
        .mcp
        .connect(&Key::of("echo", Some(&here())), &engine.store, &hub, Start::User)
        .await
        .unwrap();
    assert!(
        engine
            .mcp
            .tools(&engine.store, Some(&here()))
            .iter()
            .any(|t| t.spec().name == "echo_echo"),
        "the readable server works"
    );
    engine.mcp.disconnect("echo", &engine.store, &hub).await;
}

#[tokio::test]
async fn a_variant_switch_keeps_the_turns_admitted_mcp_catalog() {
    use crate::llm::{Chunk, Provider};
    use crate::session::turn::tests::{model, prompt, text, tool_call};
    use crate::session::types::Visibility;

    let engine = engine();
    engine
        .credentials
        .set("anthropic", &crate::llm::Credential::ApiKey { key: "test".into() })
        .unwrap();

    let dir = engine.data_dir.join("ws");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    let workspace = engine.store.add_workspace(&dir.to_string_lossy(), "ws", "").unwrap();
    let session = engine
        .store
        .create_session(crate::store::NewSession {
            workspace_id: &workspace.id,
            parent_id: None,
            visibility: Visibility::Sibling,
            title: "Test",
            agent: "build",
            model: Some(&model()),
        })
        .unwrap();

    saved(&engine, "first", &echo_config()).await;
    engine.connect_mcp_in("first", Some(&here())).await.unwrap();
    let provider = crate::llm::scripted::Scripted::default();
    *engine.turns.provider_override.lock().unwrap() = Some(Provider::Scripted(provider.clone()));
    provider
        .push_paused(
            vec![Chunk::TextStart, Chunk::TextDelta("working".into())],
            std::time::Duration::from_millis(1000),
            [vec![Chunk::BlockStop], tool_call("read", r#"{"path":"a.txt"}"#)].concat(),
        )
        .push(text("done"));

    engine.submit(&session.id, prompt("start")).await.unwrap();
    saved(&engine, "later", &echo_config()).await;
    engine.connect_mcp_in("later", Some(&here())).await.unwrap();
    engine
        .submit(
            &session.id,
            crate::session::turn::Prompt {
                variant: Some(Some("high".into())),
                ..prompt("switch level")
            },
        )
        .await
        .unwrap();
    for _ in 0..1000 {
        if !engine.turns.is_running(&session.id) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    {
        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].tools.iter().any(|tool| tool.name == "first_echo"));
        assert!(!requests[1].tools.iter().any(|tool| tool.name == "later_echo"));
    }

    for name in ["first", "later"] {
        engine.mcp.disconnect(name, &engine.store, &engine.hub).await;
    }
}

#[tokio::test]
async fn a_saved_server_connects_and_its_tools_appear_prefixed() {
    let engine = engine();
    let hub = Hub::new(32);
    let saved = engine.store.save_mcp_server("echo", &echo_config()).unwrap();
    assert_eq!(
        engine.mcp.status_of(saved.clone()).state,
        State::Disconnected,
        "nothing to approve: it is ready to connect"
    );

    engine
        .mcp
        .connect(&Key::of("echo", Some(&here())), &engine.store, &hub, Start::User)
        .await
        .unwrap();
    let status = engine.mcp.status_of(saved.clone());
    assert_eq!(status.state, State::Connected);
    assert_eq!(
        status
            .tools
            .iter()
            .map(|t| (t.name.as_str(), t.read_only))
            .collect::<Vec<_>>(),
        [("echo", true), ("shout", false)]
    );

    let tools = engine.mcp.tools(&engine.store, Some(&here()));
    let names: Vec<String> = tools.iter().map(|t| t.spec().name).collect();
    assert_eq!(names, ["echo_echo", "echo_shout"]);
    let ctx = Context {
        agent: "build".into(),
        workspace: here(),
        session_id: "s".into(),
        message_id: "m".into(),
        call_id: "c".into(),
        files: Arc::new(SessionFiles::default()),
        abort: Default::default(),
        engine: engine.clone(),
        config: Default::default(),
        progress: Default::default(),
        command_model: None,
    };
    let echo = tools.iter().find(|t| t.spec().name == "echo_echo").unwrap();
    let read_only = echo.ask(&ctx, &json!({})).unwrap();
    assert_eq!(
        engine
            .permissions
            .decide_now("s", &crate::permission::Policy::default(), &read_only),
        crate::permission::Decision::Allow,
        "read-only tools need no approval by default"
    );
    let denied = crate::permission::Policy {
        rules: vec![crate::permission::Rule {
            kind: "mcp".into(),
            pattern: "echo/*".into(),
            decision: crate::permission::Decision::Deny,
        }],
    };
    assert_eq!(
        engine.permissions.decide_now("s", &denied, &read_only),
        crate::permission::Decision::Deny
    );
    assert!(!echo.mutates());

    let out = echo.run(&ctx, json!({ "text": "hi" })).await.unwrap();
    assert_eq!(out.output, "hi");
    let shout = tools.iter().find(|t| t.spec().name == "echo_shout").unwrap();
    assert_eq!(shout.ask(&ctx, &json!({})).unwrap().pattern, "echo/shout");
    assert_eq!(shout.run(&ctx, json!({ "text": "hi" })).await.unwrap().output, "HI");
    assert_eq!(
        shout.run(&ctx, json!({ "text": "fail" })).await.unwrap_err().0,
        "asked to fail"
    );

    assert!(echo.stays_read_only(&ctx, &json!({})), "a new server is trusted");
    engine.store.set_mcp_read_only_trusted("echo", false).unwrap();
    assert!(
        !echo.stays_read_only(&ctx, &json!({})),
        "untrusted, a server's own read-only mark does not open it to read-only agents"
    );
    engine.store.set_mcp_read_only_trusted("echo", true).unwrap();
    assert!(echo.stays_read_only(&ctx, &json!({})), "the user trusts this server");

    assert!(
        !shout.stays_read_only(&ctx, &json!({})),
        "a tool it does not mark read-only still is not"
    );

    let ServerConfig::Stdio {
        command,
        args,
        cwd,
        timeout_seconds,
        ..
    } = echo_config()
    else {
        unreachable!()
    };
    let other = ServerConfig::Stdio {
        command,
        args,
        env: [("TOKEN".to_string(), "other".to_string())].into(),
        cwd,
        timeout_seconds,
    };
    engine.store.save_mcp_server("echo", &other).unwrap();
    assert!(
        !echo.stays_read_only(&ctx, &json!({})),
        "a connection opened from another definition, env included, is not trusted"
    );

    assert!(engine.mcp.disconnect("echo", &engine.store, &hub).await);
    assert_eq!(engine.mcp.status_of(saved).state, State::Disconnected);
    assert!(engine.mcp.tools(&engine.store, Some(&here())).is_empty());
}

#[tokio::test]
async fn a_server_that_exits_by_itself_is_reconnected_and_its_tools_come_back() {
    let engine = engine();
    let row = saved(&engine, "echo", &echo_config()).await;
    engine.connect_mcp_in(&row.name, Some(&here())).await.unwrap();
    let ctx = context(&engine);

    assert!(
        tool(&engine, "echo_shout")
            .run(&ctx, json!({ "text": "crash" }))
            .await
            .is_err()
    );
    until("it noticed", || {
        engine.mcp.status_of(row.clone()).state != State::Connected
    })
    .await;
    until("it is back", || {
        engine.mcp.status_of(row.clone()).state == State::Connected && find(&engine, "echo_echo").is_some()
    })
    .await;
    assert_eq!(
        tool(&engine, "echo_echo")
            .run(&ctx, json!({ "text": "hi again" }))
            .await
            .unwrap()
            .output,
        "hi again",
        "a new process serves it"
    );
}

#[tokio::test]
async fn an_ordinary_tool_failure_is_not_a_lost_connection() {
    let engine = engine();
    let row = saved(&engine, "echo", &echo_config()).await;
    engine.connect_mcp_in(&row.name, Some(&here())).await.unwrap();
    let mut events = engine.hub.attach(None).rx;
    let ctx = context(&engine);

    assert_eq!(
        tool(&engine, "echo_shout")
            .run(&ctx, json!({ "text": "fail" }))
            .await
            .unwrap_err()
            .0,
        "asked to fail"
    );

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    while let Ok(envelope) = events.try_recv() {
        assert!(
            !matches!(envelope.event, Event::McpUpdated { .. }),
            "no reconnect: {:?}",
            envelope.event
        );
    }
    assert_eq!(engine.mcp.status_of(row).state, State::Connected);
}

#[tokio::test]
async fn a_deliberate_disconnect_ends_reconnecting() {
    let engine = engine();
    let row = saved(&engine, "echo", &echo_config()).await;
    engine.connect_mcp_in(&row.name, Some(&here())).await.unwrap();
    let _ = tool(&engine, "echo_shout")
        .run(&context(&engine), json!({ "text": "crash" }))
        .await;

    until("it noticed", || {
        engine.mcp.status_of(row.clone()).state != State::Connected
    })
    .await;
    engine.mcp.disconnect("echo", &engine.store, &engine.hub).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(
        engine.mcp.status_of(row).state,
        State::Disconnected,
        "the user's disconnect stands"
    );
    assert!(find(&engine, "echo_echo").is_none());
}

#[tokio::test]
async fn a_turn_waits_briefly_for_a_server_still_connecting() {
    let engine = engine();
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/slow-server.cjs");
    let slow = |ms: &str| ServerConfig::Stdio {
        command: "node".into(),
        args: vec![script.into()],
        env: [("SLOW_MS".to_string(), ms.to_string())].into(),
        cwd: None,
        timeout_seconds: None,
    };
    saved(&engine, "quick", &slow("600")).await;
    let connecting = tokio::spawn({
        let engine = engine.clone();
        async move { engine.connect_mcp_in("quick", Some(&here())).await }
    });
    until("connecting", || engine.mcp.connecting()).await;
    engine.mcp.wait_ready(READY_WAIT).await;
    assert!(
        find(&engine, "quick_old_tool").is_some(),
        "the turn being planned sees it"
    );
    connecting.await.unwrap().unwrap();

    saved(&engine, "stuck", &slow("5000")).await;
    let _connecting = tokio::spawn({
        let engine = engine.clone();
        async move { engine.connect_mcp_in("stuck", Some(&here())).await }
    });
    until("connecting", || engine.mcp.connecting()).await;
    let started = std::time::Instant::now();
    engine.mcp.wait_ready(std::time::Duration::from_millis(400)).await;
    let waited = started.elapsed();
    assert!(
        waited >= std::time::Duration::from_millis(350) && waited < std::time::Duration::from_secs(2),
        "bounded: {waited:?}"
    );
    assert!(find(&engine, "stuck_old_tool").is_none());
}

#[tokio::test]
async fn a_bad_command_reports_failed() {
    let engine = engine();
    let hub = Hub::new(8);
    let config = ServerConfig::Stdio {
        command: "definitely-not-a-program".into(),
        args: vec![],
        env: Default::default(),
        cwd: None,
        timeout_seconds: None,
    };
    let row = engine.store.save_mcp_server("broken", &config).unwrap();

    assert!(
        engine
            .mcp
            .connect(&Key::of("broken", Some(&here())), &engine.store, &hub, Start::User)
            .await
            .is_err()
    );
    let status = engine.mcp.status_of(row);
    assert_eq!(status.state, State::Failed);
    assert!(
        status
            .error
            .unwrap()
            .contains("definitely-not-a-program was not found on PATH")
    );
}

#[tokio::test]
async fn a_config_change_during_connect_discards_the_late_connection() {
    let engine = engine();
    let hub = Hub::new(32);
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/slow-server.cjs");
    let slow = ServerConfig::Stdio {
        command: "node".into(),
        args: vec![script.into()],
        env: [("SLOW_MS".to_string(), "1500".to_string())].into(),
        cwd: None,
        timeout_seconds: None,
    };
    engine.store.save_mcp_server("probe", &slow).unwrap();
    let started = std::time::Instant::now();
    let connecting = tokio::spawn({
        let engine = engine.clone();
        let hub = Hub::new(8);
        async move {
            engine
                .mcp
                .connect(&Key::of("probe", Some(&here())), &engine.store, &hub, Start::User)
                .await
                .map(|_| ())
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let replaced = ServerConfig::Stdio {
        command: "node".into(),
        args: vec![script.into()],
        env: [("TOOL_NAME".to_string(), "new_tool".to_string())].into(),
        cwd: None,
        timeout_seconds: None,
    };
    let row = engine
        .mcp
        .change("probe", &engine.store, &hub, |store| {
            store.save_mcp_server("probe", &replaced)
        })
        .await
        .unwrap();
    assert!(connecting.await.unwrap().is_err(), "the stale connect must not succeed");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(1200),
        "cancelled, not left to finish"
    );
    let status = engine.mcp.status_of(row);
    assert_ne!(status.state, State::Connected);
    assert!(status.tools.is_empty(), "no tools from the discarded connection");
    assert!(engine.mcp.tools(&engine.store, Some(&here())).is_empty());
}

#[test]
fn reconnect_backoff_resets_only_after_a_connection_that_held() {
    let carried = FIRST_RETRY * 8;
    assert_eq!(
        after_loss(STABLE / 2, carried),
        carried,
        "a connection that dropped at once keeps backing off"
    );
    assert_eq!(after_loss(STABLE, carried), FIRST_RETRY);
}
