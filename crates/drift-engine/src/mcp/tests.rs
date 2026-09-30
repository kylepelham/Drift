use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::event::Hub;
use crate::tool::{Context, SessionFiles};

fn echo_config() -> ServerConfig {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/echo-server.cjs");
    ServerConfig::Stdio { command: "node".into(), args: vec![script.into()], env: Default::default() }
}

fn engine() -> Arc<crate::Engine> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = std::env::temp_dir().join(format!("drift-mcp-{}", crate::random_hex(4)));
    crate::Engine::open_with(&dir, crate::Options { file_credentials: true, ..Default::default() }).unwrap()
}

#[tokio::test]
async fn approval_gates_connection_and_tools_appear_prefixed() {
    let engine = engine();
    let hub = Hub::new(32);
    let row = engine.store.save_mcp_server("echo", &echo_config()).unwrap();
    assert_eq!(engine.mcp.status_of(row.clone()).state, State::NeedsApproval);
    assert!(engine.mcp.connect(row.clone(), &hub).await.is_err());

    engine.store.approve_mcp_server("echo", &row.hash()).unwrap();
    let approved = engine.store.mcp_server("echo").unwrap().unwrap();
    engine.mcp.connect(approved.clone(), &hub).await.unwrap();
    let status = engine.mcp.status_of(approved.clone());
    assert_eq!(status.state, State::Connected);
    assert_eq!(status.tools.iter().map(|t| (t.name.as_str(), t.read_only)).collect::<Vec<_>>(), [("echo", true), ("shout", false)]);

    let tools = engine.mcp.tools();
    let names: Vec<String> = tools.iter().map(|t| t.spec().name).collect();
    assert_eq!(names, ["echo_echo", "echo_shout"]);
    let ctx = Context {
        workspace: std::env::temp_dir(),
        session_id: "s".into(),
        message_id: "m".into(),
        call_id: "c".into(),
        files: Arc::new(SessionFiles::default()),
        abort: Default::default(),
        engine: engine.clone(),
    };
    let echo = tools.iter().find(|t| t.spec().name == "echo_echo").unwrap();
    assert!(echo.ask(&ctx, &json!({})).is_none(), "read-only tools need no ask");
    assert!(!echo.mutates());
    let out = echo.run(&ctx, json!({ "text": "hi" })).await.unwrap();
    assert_eq!(out.output, "hi");
    let shout = tools.iter().find(|t| t.spec().name == "echo_shout").unwrap();
    assert_eq!(shout.ask(&ctx, &json!({})).unwrap().pattern, "echo/shout");
    assert_eq!(shout.run(&ctx, json!({ "text": "hi" })).await.unwrap().output, "HI");
    assert_eq!(shout.run(&ctx, json!({ "text": "fail" })).await.unwrap_err().0, "asked to fail");

    assert!(engine.mcp.disconnect("echo", &engine.store, &hub).await);
    assert_eq!(engine.mcp.status_of(approved).state, State::Disconnected);
    assert!(engine.mcp.tools().is_empty());
}

#[tokio::test]
async fn a_bad_command_reports_failed() {
    let engine = engine();
    let hub = Hub::new(8);
    let config = ServerConfig::Stdio { command: "definitely-not-a-program".into(), args: vec![], env: Default::default() };
    let row = engine.store.save_mcp_server("broken", &config).unwrap();
    engine.store.approve_mcp_server("broken", &row.hash()).unwrap();
    let row = engine.store.mcp_server("broken").unwrap().unwrap();
    assert!(engine.mcp.connect(row.clone(), &hub).await.is_err());
    let status = engine.mcp.status_of(row);
    assert_eq!(status.state, State::Failed);
    assert!(status.error.unwrap().contains("could not start"));
}

#[tokio::test]
async fn a_config_change_during_connect_discards_the_late_connection() {
    let engine = engine();
    let hub = Hub::new(32);
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/slow-server.cjs");
    let slow = ServerConfig::Stdio { command: "node".into(), args: vec![script.into()], env: [("SLOW_MS".to_string(), "1500".to_string())].into() };
    let row = engine.store.save_mcp_server("probe", &slow).unwrap();
    engine.store.approve_mcp_server("probe", &row.hash()).unwrap();
    let approved = engine.store.mcp_server("probe").unwrap().unwrap();
    let connecting = tokio::spawn({
        let engine = engine.clone();
        let hub = Hub::new(8);
        async move { engine.mcp.connect(approved, &hub).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    // Replace the definition the way the save route does: disconnect (invalidates), then store the new config.
    engine.mcp.disconnect("probe", &engine.store, &hub).await;
    let replaced = ServerConfig::Stdio { command: "node".into(), args: vec![script.into()], env: [("TOOL_NAME".to_string(), "new_tool".to_string())].into() };
    let row = engine.store.save_mcp_server("probe", &replaced).unwrap();
    assert!(connecting.await.unwrap().is_err(), "the stale connect must not succeed");
    let status = engine.mcp.status_of(row);
    assert_eq!(status.state, State::NeedsApproval);
    assert!(status.tools.is_empty(), "no tools from the discarded connection");
    assert!(engine.mcp.tools().is_empty());
}
