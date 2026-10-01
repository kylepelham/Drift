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
async fn a_saved_server_connects_and_its_tools_appear_prefixed() {
    let engine = engine();
    let hub = Hub::new(32);
    let saved = engine.store.save_mcp_server("echo", &echo_config()).unwrap();
    assert_eq!(engine.mcp.status_of(saved.clone()).state, State::Disconnected, "nothing to approve: it is ready to connect");
    engine.mcp.connect("echo", &engine.store, &hub, Start::User).await.unwrap();
    let status = engine.mcp.status_of(saved.clone());
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
    assert_eq!(engine.mcp.status_of(saved).state, State::Disconnected);
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

async fn saved(engine: &Arc<crate::Engine>, name: &str, config: &ServerConfig) -> ServerRow {
    engine.store.save_mcp_server(name, config).unwrap()
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
    let row = saved(&engine, "echo", &echo_config()).await;
    engine.connect_mcp(&row.name).await.unwrap();
    let ctx = context(&engine);
    assert!(tool(&engine, "echo_shout").run(&ctx, json!({ "text": "crash" })).await.is_err());
    until("it noticed", || engine.mcp.status_of(row.clone()).state != State::Connected).await;
    until("it is back", || engine.mcp.status_of(row.clone()).state == State::Connected && find(&engine, "echo_echo").is_some()).await;
    assert_eq!(tool(&engine, "echo_echo").run(&ctx, json!({ "text": "hi again" })).await.unwrap().output, "hi again", "a new process serves it");
}

#[tokio::test]
async fn an_ordinary_tool_failure_is_not_a_lost_connection() {
    let engine = engine();
    let row = saved(&engine, "echo", &echo_config()).await;
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
    let row = saved(&engine, "echo", &echo_config()).await;
    engine.connect_mcp(&row.name).await.unwrap();
    let _ = tool(&engine, "echo_shout").run(&context(&engine), json!({ "text": "crash" })).await;
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
    saved(&engine, "quick", &slow("600")).await;
    let connecting = tokio::spawn({
        let engine = engine.clone();
        async move { engine.connect_mcp("quick").await }
    });
    until("connecting", || engine.mcp.connecting()).await;
    engine.mcp.wait_ready(READY_WAIT).await;
    assert!(find(&engine, "quick_old_tool").is_some(), "the turn being planned sees it");
    connecting.await.unwrap().unwrap();

    saved(&engine, "stuck", &slow("5000")).await;
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
    assert!(engine.mcp.connect("broken", &engine.store, &hub, Start::User).await.is_err());
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
    engine.store.save_mcp_server("probe", &slow).unwrap();
    let started = std::time::Instant::now();
    let connecting = tokio::spawn({
        let engine = engine.clone();
        let hub = Hub::new(8);
        async move { engine.mcp.connect("probe", &engine.store, &hub, Start::User).await.map(|_| ()) }
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let replaced = ServerConfig::Stdio { command: "node".into(), args: vec![script.into()], env: [("TOOL_NAME".to_string(), "new_tool".to_string())].into() };
    let row = engine.mcp.change("probe", &engine.store, &hub, |store| store.save_mcp_server("probe", &replaced)).await.unwrap();
    assert!(connecting.await.unwrap().is_err(), "the stale connect must not succeed");
    assert!(started.elapsed() < std::time::Duration::from_millis(1200), "cancelled, not left to finish");
    let status = engine.mcp.status_of(row);
    assert_ne!(status.state, State::Connected);
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
    let row = saved(&engine, "mute", &config).await;
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
    saved(&engine, "quiet", &config).await;
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
    let row = saved(&engine, "slow", &config).await;
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
    let row = saved(&engine, "dropped", &config).await;
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
async fn the_startup_sweep_never_cancels_a_connect_already_under_way() {
    let engine = engine();
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/slow-server.cjs");
    let slow = ServerConfig::Stdio { command: "node".into(), args: vec![script.into()], env: [("SLOW_MS".to_string(), "500".to_string())].into() };
    let row = saved(&engine, "slow", &slow).await;
    let connecting = tokio::spawn({
        let engine = engine.clone();
        async move { engine.connect_mcp("slow").await }
    });
    until("connecting", || engine.mcp.connecting()).await;
    engine.connect_all_mcp().await;
    assert_eq!(connecting.await.unwrap(), Ok(()), "the user's connect lands");
    assert_eq!(engine.mcp.status_of(row).state, State::Connected);
}

#[tokio::test]
async fn a_newer_connect_supersedes_one_in_flight() {
    let engine = engine();
    let (config, file) = traced(&[("SLOW_MS", "600000")]);
    saved(&engine, "twice", &config).await;
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

/// The echo server, logging every call to the returned file and crashing `crash-once` only the first time.
fn logged_echo() -> (ServerConfig, std::path::PathBuf) {
    let ServerConfig::Stdio { command, args, .. } = echo_config() else { unreachable!() };
    let dir = std::env::temp_dir().join(format!("drift-mcp-log-{}", crate::random_hex(4)));
    std::fs::create_dir_all(&dir).unwrap();
    let env = [("CALL_LOG", dir.join("calls")), ("CRASH_MARKER", dir.join("crashed"))].map(|(k, v)| (k.to_string(), v.to_string_lossy().into_owned()));
    (ServerConfig::Stdio { command, args, env: env.into() }, dir.join("calls"))
}

fn calls(log: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(log).unwrap_or_default().lines().map(String::from).collect()
}

#[tokio::test]
async fn a_running_turns_tool_follows_a_reconnect_but_never_replays_a_call_that_may_have_landed() {
    let engine = engine();
    let (config, log) = logged_echo();
    let row = saved(&engine, "echo", &config).await;
    engine.connect_mcp("echo").await.unwrap();
    let (ctx, shout) = (context(&engine), tool(&engine, "echo_shout"));
    let lost = shout.run(&ctx, json!({ "text": "crash" })).await.unwrap_err().0;
    assert!(lost.contains("may or may not have taken effect") && lost.contains("not retried"), "{lost}");
    until("it noticed", || engine.mcp.status_of(row.clone()).state != State::Connected).await;
    until("it is back", || engine.mcp.status_of(row.clone()).state == State::Connected).await;
    assert_eq!(calls(&log), ["shout crash"], "sent once, not replayed on the new process");
    assert_eq!(shout.run(&ctx, json!({ "text": "hi" })).await.unwrap().output, "HI", "the captured tool reaches the reconnected server");
}

#[tokio::test]
async fn a_read_only_call_cut_off_by_a_lost_connection_is_asked_again_once() {
    let engine = engine();
    let (config, log) = logged_echo();
    saved(&engine, "echo", &config).await;
    engine.connect_mcp("echo").await.unwrap();
    let out = tool(&engine, "echo_echo").run(&context(&engine), json!({ "text": "crash-once" })).await.unwrap();
    assert_eq!(out.output, "crash-once", "answered by the reconnected server");
    assert_eq!(calls(&log), ["echo crash-once", "echo crash-once"]);
}

#[tokio::test]
async fn a_captured_tool_is_not_run_on_a_reconnected_server_that_redefined_it() {
    let engine = engine();
    let (ServerConfig::Stdio { command, args, mut env }, log) = logged_echo() else { unreachable!() };
    env.insert("REDEFINE_AFTER_CRASH".into(), "1".into());
    saved(&engine, "echo", &ServerConfig::Stdio { command, args, env }).await;
    engine.connect_mcp("echo").await.unwrap();
    let echo = tool(&engine, "echo_echo");
    let refused = echo.run(&context(&engine), json!({ "text": "crash-once" })).await.unwrap_err().0;
    assert!(refused.contains("changed its echo tool"), "{refused}");
    assert_eq!(calls(&log), ["echo crash-once"], "not asked again of a server that now calls it mutating");
}

#[tokio::test]
async fn disabling_ends_calls_under_way_and_refuses_captured_tools_until_reenabled() {
    let engine = engine();
    let (config, log) = logged_echo();
    saved(&engine, "echo", &config).await;
    engine.connect_mcp("echo").await.unwrap();
    let (echo, shout) = (tool(&engine, "echo_echo"), tool(&engine, "echo_shout"));
    let hanging = tokio::spawn({
        let (engine, shout) = (engine.clone(), shout.clone());
        async move { shout.run(&context(&engine), json!({ "text": "hang" })).await.map(|out| out.output).map_err(|e| e.0) }
    });
    until("the call reached the server", || calls(&log) == ["shout hang"]).await;
    engine.mcp.close("echo", &engine.store, &engine.hub, |store| store.set_mcp_enabled("echo", false)).await.unwrap();
    let ended = tokio::time::timeout(std::time::Duration::from_secs(1), hanging).await.expect("the call ends at once").unwrap();
    assert!(ended.unwrap_err().contains("disabled while the call ran"));
    let ctx = context(&engine);
    assert!(echo.run(&ctx, json!({ "text": "hi" })).await.unwrap_err().0.contains("disabled or removed"), "a captured tool refuses");

    engine.store.set_mcp_enabled("echo", true).unwrap();
    engine.connect_mcp("echo").await.unwrap();
    assert_eq!(tool(&engine, "echo_echo").run(&ctx, json!({ "text": "back" })).await.unwrap().output, "back");
    assert!(echo.run(&ctx, json!({ "text": "hi" })).await.is_err(), "re-enabling does not revive tools captured before the disable");
}

#[tokio::test]
async fn a_save_leaves_running_turns_on_the_client_they_were_given() {
    let engine = engine();
    let (config, _log) = logged_echo();
    saved(&engine, "echo", &config).await;
    engine.connect_mcp("echo").await.unwrap();
    let echo = tool(&engine, "echo_echo");
    let ServerConfig::Stdio { command, args, mut env } = config else { unreachable!() };
    env.insert("CHANGED".into(), "1".into());
    engine.mcp.change("echo", &engine.store, &engine.hub, |store| store.save_mcp_server("echo", &ServerConfig::Stdio { command, args, env })).await.unwrap();
    assert!(find(&engine, "echo_echo").is_none(), "later turns see the server once it connects on the new command");
    assert_eq!(echo.run(&context(&engine), json!({ "text": "still" })).await.unwrap().output, "still", "the running turn keeps its client");
}

#[test]
fn reconnect_backoff_resets_only_after_a_connection_that_held() {
    let carried = FIRST_RETRY * 8;
    assert_eq!(after_loss(STABLE / 2, carried), carried, "a connection that dropped at once keeps backing off");
    assert_eq!(after_loss(STABLE, carried), FIRST_RETRY);
}
