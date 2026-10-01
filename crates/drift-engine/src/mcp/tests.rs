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
    assert!(engine.mcp.connect("echo", &engine.store, &hub, None).await.is_err());

    engine.store.approve_mcp_server("echo", &row.hash()).unwrap();
    let approved = engine.store.mcp_server("echo").unwrap().unwrap();
    engine.mcp.connect("echo", &engine.store, &hub, None).await.unwrap();
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
        config: Default::default(),
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

fn context(engine: &Arc<crate::Engine>) -> Context {
    Context {
        workspace: std::env::temp_dir(),
        session_id: "s".into(),
        message_id: "m".into(),
        call_id: "c".into(),
        files: Arc::new(SessionFiles::default()),
        abort: Default::default(),
        engine: engine.clone(),
        config: Default::default(),
    }
}

async fn approved(engine: &Arc<crate::Engine>, name: &str, config: &ServerConfig) -> ServerRow {
    let row = engine.store.save_mcp_server(name, config).unwrap();
    engine.store.approve_mcp_server(name, &row.hash()).unwrap();
    engine.store.mcp_server(name).unwrap().unwrap()
}

async fn until<F: Fn() -> bool>(what: &str, done: F) {
    for _ in 0..300 {
        if done() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("never: {what}");
}

fn find(engine: &crate::Engine, name: &str) -> Option<Arc<dyn crate::tool::Tool>> {
    engine.offered_tools(crate::llm::catalog::ToolProfile::Edit).into_iter().find(|t| t.spec().name == name)
}

fn tool(engine: &crate::Engine, name: &str) -> Arc<dyn crate::tool::Tool> {
    find(engine, name).unwrap()
}

#[tokio::test]
async fn a_server_that_exits_by_itself_is_reconnected_and_its_tools_come_back() {
    let engine = engine();
    let row = approved(&engine, "echo", &echo_config()).await;
    engine.connect_mcp(&row.name).await.unwrap();
    let ctx = context(&engine);
    assert!(tool(&engine, "echo_echo").run(&ctx, json!({ "text": "crash" })).await.is_err());
    until("it noticed", || engine.mcp.status_of(row.clone()).state != State::Connected).await;
    until("it is back", || engine.mcp.status_of(row.clone()).state == State::Connected && find(&engine, "echo_echo").is_some()).await;
    assert_eq!(tool(&engine, "echo_echo").run(&ctx, json!({ "text": "hi again" })).await.unwrap().output, "hi again", "a new process serves it");
}

#[tokio::test]
async fn an_ordinary_tool_failure_is_not_a_lost_connection() {
    let engine = engine();
    let row = approved(&engine, "echo", &echo_config()).await;
    engine.connect_mcp(&row.name).await.unwrap();
    let mut events = engine.hub.attach(None).rx;
    let ctx = context(&engine);
    assert_eq!(tool(&engine, "echo_shout").run(&ctx, json!({ "text": "fail" })).await.unwrap_err().0, "asked to fail");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    while let Ok(envelope) = events.try_recv() {
        assert!(!matches!(envelope.event, Event::McpUpdated { .. }), "no reconnect: {:?}", envelope.event);
    }
    assert_eq!(engine.mcp.status_of(row).state, State::Connected);
}

#[tokio::test]
async fn a_deliberate_disconnect_ends_reconnecting() {
    let engine = engine();
    let row = approved(&engine, "echo", &echo_config()).await;
    engine.connect_mcp(&row.name).await.unwrap();
    let _ = tool(&engine, "echo_echo").run(&context(&engine), json!({ "text": "crash" })).await;
    until("it noticed", || engine.mcp.status_of(row.clone()).state != State::Connected).await;
    engine.mcp.disconnect("echo", &engine.store, &engine.hub).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(engine.mcp.status_of(row).state, State::Disconnected, "the user's disconnect stands");
    assert!(find(&engine, "echo_echo").is_none());
}

#[tokio::test]
async fn a_turn_waits_briefly_for_a_server_still_connecting() {
    let engine = engine();
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/slow-server.cjs");
    let slow = |ms: &str| ServerConfig::Stdio { command: "node".into(), args: vec![script.into()], env: [("SLOW_MS".to_string(), ms.to_string())].into() };
    approved(&engine, "quick", &slow("600")).await;
    let connecting = tokio::spawn({
        let engine = engine.clone();
        async move { engine.connect_mcp("quick").await }
    });
    until("connecting", || engine.mcp.connecting()).await;
    engine.mcp.wait_ready(READY_WAIT).await;
    assert!(find(&engine, "quick_old_tool").is_some(), "the turn being planned sees it");
    connecting.await.unwrap().unwrap();

    approved(&engine, "stuck", &slow("5000")).await;
    let _connecting = tokio::spawn({
        let engine = engine.clone();
        async move { engine.connect_mcp("stuck").await }
    });
    until("connecting", || engine.mcp.connecting()).await;
    let started = std::time::Instant::now();
    engine.mcp.wait_ready(std::time::Duration::from_millis(400)).await;
    let waited = started.elapsed();
    assert!(waited >= std::time::Duration::from_millis(350) && waited < std::time::Duration::from_secs(2), "bounded: {waited:?}");
    assert!(find(&engine, "stuck_old_tool").is_none());
}

#[tokio::test]
async fn a_bad_command_reports_failed() {
    let engine = engine();
    let hub = Hub::new(8);
    let config = ServerConfig::Stdio { command: "definitely-not-a-program".into(), args: vec![], env: Default::default() };
    let row = engine.store.save_mcp_server("broken", &config).unwrap();
    engine.store.approve_mcp_server("broken", &row.hash()).unwrap();
    let row = engine.store.mcp_server("broken").unwrap().unwrap();
    assert!(engine.mcp.connect("broken", &engine.store, &hub, None).await.is_err());
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
    let started = std::time::Instant::now();
    let connecting = tokio::spawn({
        let engine = engine.clone();
        let hub = Hub::new(8);
        async move { engine.mcp.connect("probe", &engine.store, &hub, None).await.map(|_| ()) }
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let replaced = ServerConfig::Stdio { command: "node".into(), args: vec![script.into()], env: [("TOOL_NAME".to_string(), "new_tool".to_string())].into() };
    let row = engine.mcp.change("probe", &engine.store, &hub, |store| store.save_mcp_server("probe", &replaced)).await.unwrap();
    assert!(connecting.await.unwrap().is_err(), "the stale connect must not succeed");
    assert!(started.elapsed() < std::time::Duration::from_millis(1200), "cancelled, not left to finish");
    let status = engine.mcp.status_of(row);
    assert_eq!(status.state, State::NeedsApproval);
    assert!(status.tools.is_empty(), "no tools from the discarded connection");
    assert!(engine.mcp.tools().is_empty());
}

/// A slow server that records its pid and a grandchild's in a file, so a test can see them die.
fn traced(env: &[(&str, &str)]) -> (ServerConfig, std::path::PathBuf) {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/slow-server.cjs");
    let pids = std::env::temp_dir().join(format!("drift-mcp-pids-{}", crate::random_hex(4)));
    let mut vars: BTreeMap<String, String> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    vars.insert("PID_FILE".into(), pids.to_string_lossy().into());
    vars.insert("GRANDCHILD".into(), "1".into());
    (ServerConfig::Stdio { command: "node".into(), args: vec![script.into()], env: vars }, pids)
}

async fn pids_in(file: &std::path::Path) -> Vec<u32> {
    let read = || std::fs::read_to_string(file).ok().filter(|text| text.split_whitespace().count() == 2);
    until("the server wrote its pids", || read().is_some()).await;
    let pids: Vec<u32> = read().unwrap().split_whitespace().map(|pid| pid.parse().unwrap()).collect();
    assert!(pids.iter().all(|pid| alive(*pid)), "the server and its child are running");
    pids
}

fn alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let listed = std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
        String::from_utf8_lossy(&listed.stdout).contains(&format!(" {pid} "))
    }
    #[cfg(unix)]
    {
        std::process::Command::new("kill").args(["-0", &pid.to_string()]).status().is_ok_and(|s| s.success())
    }
}

async fn all_dead(pids: &[u32]) {
    until("the server and its child are gone", || pids.iter().all(|pid| !alive(*pid))).await;
}

#[tokio::test]
async fn a_server_that_never_finishes_starting_times_out_and_its_process_tree_dies() {
    let engine = engine();
    let (config, file) = traced(&[("SLOW_MS", "600000")]);
    let row = approved(&engine, "mute", &config).await;
    let connecting = tokio::spawn({
        let engine = engine.clone();
        async move { engine.connect_mcp("mute").await }
    });
    let pids = pids_in(&file).await;
    let failed = connecting.await.unwrap().unwrap_err();
    assert!(failed.contains("did not start within"), "{failed}");
    let status = engine.mcp.status_of(row);
    assert_eq!(status.state, State::Failed);
    assert!(!engine.mcp.connecting(), "waiters are not held up by it");
    all_dead(&pids).await;
}

#[tokio::test]
async fn a_server_that_never_lists_its_tools_times_out() {
    let engine = engine();
    let (config, file) = traced(&[("SLOW_MS", "0"), ("LIST_SLOW_MS", "600000")]);
    approved(&engine, "quiet", &config).await;
    let connecting = tokio::spawn({
        let engine = engine.clone();
        async move { engine.connect_mcp("quiet").await }
    });
    let pids = pids_in(&file).await;
    let failed = connecting.await.unwrap().unwrap_err();
    assert!(failed.contains("did not list its tools within"), "{failed}");
    all_dead(&pids).await;
}

#[tokio::test]
async fn disconnecting_cancels_a_connect_in_flight_and_kills_what_it_started() {
    let engine = engine();
    let (config, file) = traced(&[("SLOW_MS", "600000")]);
    let row = approved(&engine, "slow", &config).await;
    let connecting = tokio::spawn({
        let engine = engine.clone();
        async move { engine.connect_mcp("slow").await }
    });
    let pids = pids_in(&file).await;
    engine.mcp.disconnect("slow", &engine.store, &engine.hub).await;
    let ended = tokio::time::timeout(std::time::Duration::from_secs(1), connecting).await.expect("the connect ends at once");
    assert!(ended.unwrap().is_err());
    assert_eq!(engine.mcp.status_of(row).state, State::Disconnected);
    all_dead(&pids).await;
}

#[tokio::test]
async fn a_connect_dropped_midway_settles_and_kills_what_it_started() {
    let engine = engine();
    let (config, file) = traced(&[("SLOW_MS", "600000")]);
    let row = approved(&engine, "dropped", &config).await;
    let connecting = tokio::spawn({
        let engine = engine.clone();
        async move { engine.connect_mcp("dropped").await }
    });
    let pids = pids_in(&file).await;
    assert!(engine.mcp.connecting());
    connecting.abort();
    until("the attempt settled", || !engine.mcp.connecting()).await;
    assert_eq!(engine.mcp.status_of(row).state, State::Disconnected, "not left connecting");
    let started = std::time::Instant::now();
    engine.mcp.wait_ready(READY_WAIT).await;
    assert!(started.elapsed() < std::time::Duration::from_millis(100), "a planning turn does not wait on it");
    all_dead(&pids).await;
}

#[tokio::test]
async fn a_newer_connect_supersedes_one_in_flight() {
    let engine = engine();
    let (config, file) = traced(&[("SLOW_MS", "600000")]);
    approved(&engine, "twice", &config).await;
    let first = tokio::spawn({
        let engine = engine.clone();
        async move { engine.connect_mcp("twice").await }
    });
    let pids = pids_in(&file).await;
    std::fs::remove_file(&file).unwrap();
    let _second = tokio::spawn({
        let engine = engine.clone();
        async move { engine.connect_mcp("twice").await }
    });
    let ended = tokio::time::timeout(std::time::Duration::from_secs(1), first).await.expect("the first ends at once");
    assert!(ended.unwrap().is_err());
    all_dead(&pids).await;
}

#[test]
fn reconnect_backoff_resets_only_after_a_connection_that_held() {
    let carried = FIRST_RETRY * 8;
    assert_eq!(after_loss(STABLE / 2, carried), carried, "a connection that dropped at once keeps backing off");
    assert_eq!(after_loss(STABLE, carried), FIRST_RETRY);
}
