use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::event::Hub;
use crate::tool::{Context, SessionFiles};

fn echo_config() -> ServerConfig {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/echo-server.cjs");
    ServerConfig::Stdio { command: "node".into(), args: vec![script.into()], env: Default::default(), cwd: None, timeout_seconds: None }
}

fn engine() -> Arc<crate::Engine> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = std::env::temp_dir().join(format!("drift-mcp-{}", crate::random_hex(4)));
    crate::Engine::open_with(&dir, crate::Options { file_credentials: true, ..Default::default() }).unwrap()
}

#[tokio::test]
async fn a_server_this_build_cannot_read_is_listed_failed_and_the_rest_still_connect() {
    let engine = engine();
    let hub = Hub::new(32);
    engine.store.save_mcp_server("echo", &echo_config()).unwrap();
    engine.store.save_mcp_server("newer", &echo_config()).unwrap();
    engine.store.lock().execute("UPDATE mcp_config SET config_json = '{\"type\":\"future\"}' WHERE name = 'newer'", []).unwrap();
    let statuses = engine.mcp.statuses(&engine.store).unwrap();
    assert_eq!(statuses.iter().map(|s| (s.server.name.as_str(), s.state)).collect::<Vec<_>>(), [("echo", State::Disconnected), ("newer", State::Failed)]);
    assert!(statuses[1].error.as_deref().is_some_and(|e| e.contains("could not be read")), "{:?}", statuses[1].error);
    engine.mcp.connect("echo", &engine.store, &hub, Start::User).await.unwrap();
    assert!(engine.mcp.tools(&engine.store).iter().any(|t| t.spec().name == "echo_echo"), "the readable server works");
    engine.mcp.disconnect("echo", &engine.store, &hub).await;
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

    let tools = engine.mcp.tools(&engine.store);
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
        progress: Default::default(),
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

    assert!(!echo.stays_read_only(&ctx, &json!({})), "a server's own read-only mark does not open it to read-only agents");
    engine.store.set_mcp_read_only_trusted("echo", true).unwrap();
    assert!(echo.stays_read_only(&ctx, &json!({})), "the user trusts this server");
    assert!(!shout.stays_read_only(&ctx, &json!({})), "a tool it does not mark read-only still is not");
    let ServerConfig::Stdio { command, args, cwd, timeout_seconds, .. } = echo_config() else { unreachable!() };
    let other = ServerConfig::Stdio { command, args, env: [("TOKEN".to_string(), "other".to_string())].into(), cwd, timeout_seconds };
    engine.store.save_mcp_server("echo", &other).unwrap();
    assert!(!echo.stays_read_only(&ctx, &json!({})), "another definition under the name, env included, is not trusted");

    assert!(engine.mcp.disconnect("echo", &engine.store, &hub).await);
    assert_eq!(engine.mcp.status_of(saved).state, State::Disconnected);
    assert!(engine.mcp.tools(&engine.store).is_empty());
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
        progress: Default::default(),
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
    let slow = |ms: &str| ServerConfig::Stdio { command: "node".into(), args: vec![script.into()], env: [("SLOW_MS".to_string(), ms.to_string())].into(), cwd: None, timeout_seconds: None };
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
    let config = ServerConfig::Stdio { command: "definitely-not-a-program".into(), args: vec![], env: Default::default(), cwd: None, timeout_seconds: None };
    let row = engine.store.save_mcp_server("broken", &config).unwrap();
    assert!(engine.mcp.connect("broken", &engine.store, &hub, Start::User).await.is_err());
    let status = engine.mcp.status_of(row);
    assert_eq!(status.state, State::Failed);
    assert!(status.error.unwrap().contains("definitely-not-a-program was not found on PATH"));
}

#[tokio::test]
async fn a_config_change_during_connect_discards_the_late_connection() {
    let engine = engine();
    let hub = Hub::new(32);
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/slow-server.cjs");
    let slow = ServerConfig::Stdio { command: "node".into(), args: vec![script.into()], env: [("SLOW_MS".to_string(), "1500".to_string())].into(), cwd: None, timeout_seconds: None };
    engine.store.save_mcp_server("probe", &slow).unwrap();
    let started = std::time::Instant::now();
    let connecting = tokio::spawn({
        let engine = engine.clone();
        let hub = Hub::new(8);
        async move { engine.mcp.connect("probe", &engine.store, &hub, Start::User).await.map(|_| ()) }
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let replaced = ServerConfig::Stdio { command: "node".into(), args: vec![script.into()], env: [("TOOL_NAME".to_string(), "new_tool".to_string())].into(), cwd: None, timeout_seconds: None };
    let row = engine.mcp.change("probe", &engine.store, &hub, |store| store.save_mcp_server("probe", &replaced)).await.unwrap();
    assert!(connecting.await.unwrap().is_err(), "the stale connect must not succeed");
    assert!(started.elapsed() < std::time::Duration::from_millis(1200), "cancelled, not left to finish");
    let status = engine.mcp.status_of(row);
    assert_ne!(status.state, State::Connected);
    assert!(status.tools.is_empty(), "no tools from the discarded connection");
    assert!(engine.mcp.tools(&engine.store).is_empty());
}

/// A slow server that records its pid and a grandchild's in a file, so a test can see them die.
fn traced(env: &[(&str, &str)]) -> (ServerConfig, std::path::PathBuf) {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/slow-server.cjs");
    let pids = std::env::temp_dir().join(format!("drift-mcp-pids-{}", crate::random_hex(4)));
    let mut vars: BTreeMap<String, String> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    vars.insert("PID_FILE".into(), pids.to_string_lossy().into());
    vars.insert("GRANDCHILD".into(), "1".into());
    (ServerConfig::Stdio { command: "node".into(), args: vec![script.into()], env: vars, cwd: None, timeout_seconds: None }, pids)
}

/// Saved as a server already known to use the handshake, so one process starts and no probe comes first.
async fn saved_legacy(engine: &Arc<crate::Engine>, name: &str, config: &ServerConfig) -> ServerRow {
    saved(engine, name, config).await;
    engine.store.set_mcp_era(name, config, Some(Era::Legacy)).unwrap();
    engine.store.mcp_server(name).unwrap().unwrap()
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
    // Known from before, so the wait is the start limit alone, and a server too slow to answer is not probed again.
    let row = saved_legacy(&engine, "mute", &config).await;
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
    saved_legacy(&engine, "quiet", &config).await;
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
    let row = saved_legacy(&engine, "slow", &config).await;
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
    let row = saved_legacy(&engine, "dropped", &config).await;
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
    let slow = ServerConfig::Stdio { command: "node".into(), args: vec![script.into()], env: [("SLOW_MS".to_string(), "500".to_string())].into(), cwd: None, timeout_seconds: None };
    let row = saved(&engine, "slow", &slow).await;
    let connecting = tokio::spawn({
        let engine = engine.clone();
        async move { engine.connect_mcp("slow").await }
    });
    until("connecting", || engine.mcp.connecting()).await;
    engine.connect_all_mcp();
    assert_eq!(connecting.await.unwrap(), Ok(()), "the user's connect lands");
    assert_eq!(engine.mcp.status_of(row).state, State::Connected);
}

#[tokio::test]
async fn at_startup_every_server_connects_at_once_and_a_dead_one_holds_up_no_other() {
    let engine = engine();
    let (mute, _) = traced(&[("SLOW_MS", "600000")]);
    saved_legacy(&engine, "mute", &mute).await;
    let fine = saved(&engine, "echo", &echo_config()).await;
    let started = std::time::Instant::now();
    engine.connect_all_mcp();
    assert!(engine.mcp.connecting(), "every connect has begun before the sweep returns");
    until("echo connects", || engine.mcp.status_of(fine.clone()).state == State::Connected).await;
    assert!(started.elapsed() < STEP_LIMIT, "echo did not wait for the server that never answers: {:?}", started.elapsed());
    assert!(engine.mcp.connecting(), "the dead one is still trying on its own");
}

#[tokio::test]
async fn a_newer_connect_supersedes_one_in_flight() {
    let engine = engine();
    let (config, file) = traced(&[("SLOW_MS", "600000")]);
    saved_legacy(&engine, "twice", &config).await;
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
    (ServerConfig::Stdio { command, args, env: env.into(), cwd: None, timeout_seconds: None }, dir.join("calls"))
}

#[tokio::test]
async fn resources_are_listed_and_read_and_prompts_become_commands() {
    let engine = engine();
    let ServerConfig::Stdio { command, args, .. } = echo_config() else { unreachable!() };
    saved(&engine, "notes", &ServerConfig::Stdio { command, args, env: [("RICH".to_string(), "1".to_string())].into(), cwd: None, timeout_seconds: None }).await;
    engine.connect_mcp("notes").await.unwrap();
    let ctx = context(&engine);
    let names: Vec<String> = engine.mcp.tools(&engine.store).iter().map(|t| t.spec().name).collect();
    assert!(names.contains(&"mcp_resources".to_string()) && names.contains(&"mcp_read_resource".to_string()), "{names:?}");
    let listed = tool(&engine, "mcp_resources").run(&ctx, json!({})).await.unwrap().output;
    assert!(listed.contains("notes note://readme readme (text/plain): The notes"), "{listed}");
    let read = tool(&engine, "mcp_read_resource");
    assert!(read.run(&ctx, json!({ "server": "notes", "uri": "note://readme" })).await.unwrap().output.contains("remember the milk"));
    let shot = read.run(&ctx, json!({ "server": "notes", "uri": "note://shot" })).await.unwrap();
    assert_eq!(crate::tool::image::returned(&shot.metadata)[0].mime, "image/png", "an image resource comes back to look at");

    let commands = engine.mcp.prompt_commands();
    assert_eq!((commands[0].name.as_str(), commands[0].arguments.clone()), ("notes:review", vec!["file".to_string(), "focus".to_string()]));
    let filled = engine.mcp.get_prompt("notes", "review", commands[0].named_arguments("src/a.rs error handling")).await.unwrap();
    assert_eq!(filled, "Review src/a.rs for error handling", "one word each, the last taking the rest");
}

#[tokio::test]
async fn a_stdio_server_runs_where_it_is_told_and_a_call_past_its_timeout_fails() {
    let engine = engine();
    let ServerConfig::Stdio { command, args, .. } = echo_config() else { unreachable!() };
    let dir = crate::tool::canonical(&std::env::temp_dir().join(format!("drift-mcp-cwd-{}", crate::random_hex(4))));
    std::fs::create_dir_all(&dir).unwrap();
    let config = ServerConfig::Stdio { command, args, env: Default::default(), cwd: Some(dir.to_string_lossy().into_owned()), timeout_seconds: Some(1) };
    let row = saved(&engine, "echo", &config).await;
    engine.connect_mcp("echo").await.unwrap();
    let ctx = context(&engine);
    let shout = tool(&engine, "echo_shout");
    let cwd = shout.run(&ctx, json!({ "text": "cwd" })).await.unwrap().output;
    let same = |path: &str| path.to_lowercase().replace('\\', "/").trim_end_matches('/').to_string();
    assert_eq!(same(&crate::tool::canonical(std::path::Path::new(&cwd)).to_string_lossy()), same(&dir.to_string_lossy()));
    let started = std::time::Instant::now();
    let hung = shout.run(&ctx, json!({ "text": "hang" })).await.unwrap_err().0;
    assert!(hung.contains("1s timeout") && started.elapsed() < std::time::Duration::from_secs(5), "{hung}");
    let status = engine.mcp.status_of(row);
    assert_eq!(status.transport, Transport::Stdio);
    assert_eq!(status.protocol.as_deref(), Some("2025-06-18"), "the version the server agreed to");
}

#[tokio::test]
async fn a_server_on_the_older_sse_transport_connects_and_answers() {
    let base = legacy_sse_server().await;
    let engine = engine();
    let row = saved(&engine, "legacy", &ServerConfig::Sse { url: format!("{base}/sse"), headers: Default::default(), oauth: None, timeout_seconds: None }).await;
    engine.connect_mcp("legacy").await.unwrap();
    let out = tool(&engine, "legacy_echo").run(&context(&engine), json!({ "text": "over sse" })).await.unwrap();
    assert_eq!(out.output, "over sse");
    assert_eq!(engine.mcp.status_of(row).transport, Transport::Sse);
}

/// A server speaking the 2024-11-05 HTTP+SSE transport: the GET stream names the POST endpoint, and
/// replies to posted messages come back on the stream.
async fn legacy_sse_server() -> String {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, legacy_sse_routes()).await.unwrap() });
    base
}

/// The HTTP+SSE transport's two routes: `/sse` streams replies, `/messages` takes requests.
fn legacy_sse_routes() -> axum::Router {
    use axum::routing::{get, post};
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let rx = Arc::new(tokio::sync::Mutex::new(Some(rx)));
    let stream = get(move || {
        let rx = rx.clone();
        async move {
            let rx = rx.lock().await.take().expect("one stream");
            let first = futures_util::stream::once(async { Ok::<_, std::convert::Infallible>("event: endpoint\ndata: /messages?session=1\n\n".to_string()) });
            let rest = futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|m| (Ok(format!("event: message\ndata: {m}\n\n")), rx)) });
            ([("content-type", "text/event-stream")], axum::body::Body::from_stream(futures_util::StreamExt::chain(first, rest)))
        }
    });
    let messages = post(move |axum::Json(message): axum::Json<serde_json::Value>| {
        let tx = tx.clone();
        async move {
            let result = match message["method"].as_str() {
                Some("initialize") => json!({ "protocolVersion": "2024-11-05", "capabilities": { "tools": {} }, "serverInfo": { "name": "legacy", "version": "0" } }),
                Some("tools/list") => json!({ "tools": [{ "name": "echo", "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } } }, "annotations": { "readOnlyHint": true } }] }),
                Some("tools/call") => json!({ "content": [{ "type": "text", "text": message["params"]["arguments"]["text"] }] }),
                _ => serde_json::Value::Null,
            };
            if !message["id"].is_null() {
                let _ = tx.send(json!({ "jsonrpc": "2.0", "id": message["id"], "result": result }).to_string());
            }
            axum::http::StatusCode::ACCEPTED
        }
    });
    axum::Router::new().route("/sse", stream).route("/messages", messages)
}

#[tokio::test]
async fn a_server_that_needs_a_sign_in_says_so_then_connects_once_signed_in() {
    let (base, seen) = oauth_mcp_server(None).await;
    let engine = engine();
    let row = saved(&engine, "secure", &ServerConfig::Http { url: format!("{base}/mcp"), headers: Default::default(), oauth: None, timeout_seconds: None }).await;
    assert!(seen.lock().unwrap().is_empty());
    let _ = engine.connect_mcp("secure").await;
    let before = engine.mcp.status_of(row.clone());
    assert!(before.needs_sign_in && !before.signed_in, "{before:?}");

    let page = engine.sign_in_mcp("secure").await.unwrap();
    assert!(page.starts_with(&format!("{base}/authorize?")), "{page}");
    // The browser: the server signs the user in at once and sends it back to Drift's callback.
    let landed = crate::llm::http::client().get(&page).send().await.unwrap();
    assert!(landed.status().is_success(), "{}", landed.status());
    until("it connects signed in", || engine.mcp.status_of(row.clone()).state == State::Connected).await;
    let after = engine.mcp.status_of(row.clone());
    assert!(after.signed_in && !after.needs_sign_in);
    assert_eq!(tool(&engine, "secure_echo").run(&context(&engine), json!({ "text": "authorized" })).await.unwrap().output, "authorized");

    assert_eq!(seen.lock().unwrap().first().map(String::as_str), Some("register"), "with no app configured Drift registers itself");

    engine.sign_out_mcp("secure").await.unwrap();
    assert!(!engine.mcp.status_of(row).signed_in, "signing out forgets the tokens");
}

/// What a browser does when the user signs in at once: follows the page, which sends it back to Drift.
async fn browse(page: &str) {
    let landed = crate::llm::http::client().get(page).send().await.unwrap();
    assert!(landed.status().is_success(), "{}", landed.status());
}

#[tokio::test]
async fn a_server_that_will_not_register_drift_signs_in_with_the_configured_app() {
    let (base, seen) = oauth_mcp_server(Some("drift-team-app")).await;
    let engine = engine();
    let app = OAuthClient { client_id: "drift-team-app".into(), client_secret: Some("team-secret".into()), scopes: vec!["read".into(), "write".into()] };
    let row = saved(&engine, "team", &ServerConfig::Http { url: format!("{base}/mcp"), headers: Default::default(), oauth: Some(app), timeout_seconds: None }).await;
    let page = engine.sign_in_mcp("team").await.unwrap();
    assert!(page.contains("client_id=drift-team-app") && page.contains("scope=read+write"), "{page}");
    browse(&page).await;
    until("it connects signed in", || engine.mcp.status_of(row.clone()).state == State::Connected).await;
    let seen = seen.lock().unwrap().clone();
    assert!(!seen.iter().any(|line| line == "register"), "{seen:?}");
    let token = seen.iter().find(|line| line.starts_with("token ")).unwrap();
    assert!(token.contains("client_secret=team-secret") || token.contains("Basic "), "the app's secret goes with the code: {token}");
}

#[tokio::test]
async fn a_sign_in_the_user_refuses_says_why_and_still_asks_to_sign_in() {
    let (base, _) = oauth_mcp_server(None).await;
    let engine = engine();
    let row = saved(&engine, "secure", &ServerConfig::Http { url: format!("{base}/mcp"), headers: Default::default(), oauth: None, timeout_seconds: None }).await;
    let page = engine.sign_in_mcp("secure").await.unwrap();
    let redirect = reqwest::Url::parse(&page).unwrap().query_pairs().find(|(key, _)| key == "redirect_uri").unwrap().1.to_string();
    let refused = crate::llm::http::client().get(format!("{redirect}?error=access_denied&error_description=The+user+said+no")).send().await.unwrap();
    assert_eq!(refused.status(), 400, "the browser hears it too");
    until("the refusal is reported", || engine.mcp.status_of(row.clone()).error.is_some()).await;
    let status = engine.mcp.status_of(row);
    assert_eq!((status.state, status.needs_sign_in), (State::Failed, true));
    assert_eq!(status.error.as_deref(), Some("Sign-in did not finish: access_denied: The user said no"));
}

#[tokio::test]
async fn a_server_on_the_older_sse_transport_signs_in_and_sends_its_token() {
    let (base, _) = oauth_mcp_server(None).await;
    let engine = engine();
    let row = saved(&engine, "legacy", &ServerConfig::Sse { url: format!("{base}/sse"), headers: Default::default(), oauth: None, timeout_seconds: None }).await;
    let _ = engine.connect_mcp("legacy").await;
    let before = engine.mcp.status_of(row.clone());
    assert!(before.needs_sign_in, "a 401 on the stream asks for a sign-in: {before:?}");
    browse(&engine.sign_in_mcp("legacy").await.unwrap()).await;
    until("it connects signed in", || engine.mcp.status_of(row.clone()).state == State::Connected).await;
    assert_eq!(tool(&engine, "legacy_echo").run(&context(&engine), json!({ "text": "over sse" })).await.unwrap().output, "over sse");
}

#[tokio::test]
async fn a_sign_in_follows_a_rename_but_never_a_new_url() {
    let engine = engine();
    let at = |url: &str| ServerConfig::Http { url: url.into(), headers: Default::default(), oauth: None, timeout_seconds: None };
    engine.credentials.set_secret("mcp:old", "{}").unwrap();
    crate::mcp::move_sign_in(&engine.credentials, "old", "new");
    assert!(engine.credentials.secret("mcp:old").is_none() && engine.credentials.secret("mcp:new").is_some());
    crate::mcp::forget_if_moved(&engine.credentials, "new", &at("https://a.example/mcp"), &at("https://a.example/mcp"));
    assert!(engine.credentials.secret("mcp:new").is_some(), "a save on the same URL keeps it");
    crate::mcp::forget_if_moved(&engine.credentials, "new", &at("https://a.example/mcp"), &at("https://b.example/mcp"));
    assert!(engine.credentials.secret("mcp:new").is_none(), "its tokens must not reach another host");
}

/// What the fake authorization server saw, one line per request: `register`, `authorize <query>`, `token <body> <authorization>`.
type AuthLog = Arc<std::sync::Mutex<Vec<String>>>;

/// Lets a request through only with the token the fake authorization server issues.
async fn bearer_only(request: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    use axum::response::IntoResponse;
    if request.headers().get("authorization").and_then(|v| v.to_str().ok()) == Some("Bearer good-token") {
        return next.run(request).await;
    }
    (axum::http::StatusCode::UNAUTHORIZED, [("www-authenticate", "Bearer")]).into_response()
}

/// An MCP server on `/mcp` (streamable HTTP) and `/sse` that wants a bearer token, with the OAuth endpoints to get one; registration only without `app`.
async fn oauth_mcp_server(app: Option<&'static str>) -> (String, AuthLog) {
    use axum::extract::{Path, Query, State as Shared};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Redirect};
    use axum::routing::{get, post};
    let _ = rustls::crypto::ring::default_provider().install_default();
    let seen: AuthLog = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let resource = |path: &'static str| {
        let base = base.clone();
        move || {
            let base = base.clone();
            async move { axum::Json(json!({ "resource": format!("{base}/{path}"), "authorization_servers": [base] })) }
        }
    };
    let resource_at = {
        let base = base.clone();
        move |Path(path): Path<String>| {
            let base = base.clone();
            async move { axum::Json(json!({ "resource": format!("{base}/{path}"), "authorization_servers": [base] })) }
        }
    };
    let metadata = {
        let base = base.clone();
        move || {
            let base = base.clone();
            async move {
                let mut metadata = json!({
                    "issuer": base, "authorization_endpoint": format!("{base}/authorize"), "token_endpoint": format!("{base}/token"),
                    "response_types_supported": ["code"], "code_challenge_methods_supported": ["S256"],
                    "grant_types_supported": ["authorization_code", "refresh_token"], "token_endpoint_auth_methods_supported": ["none", "client_secret_post", "client_secret_basic"]
                });
                if app.is_none() {
                    metadata["registration_endpoint"] = json!(format!("{base}/register"));
                }
                axum::Json(metadata)
            }
        }
    };
    let register = {
        let seen = seen.clone();
        post(move |axum::Json(body): axum::Json<serde_json::Value>| async move {
            seen.lock().unwrap().push("register".into());
            (StatusCode::CREATED, axum::Json(json!({ "client_id": "drift-test-client", "redirect_uris": body["redirect_uris"], "token_endpoint_auth_method": "none" })))
        })
    };
    let authorize = {
        let seen = seen.clone();
        get(move |axum::extract::RawQuery(raw): axum::extract::RawQuery, Query(query): Query<std::collections::HashMap<String, String>>| async move {
            seen.lock().unwrap().push(format!("authorize {}", raw.unwrap_or_default()));
            Redirect::to(&format!("{}?code=granted&state={}", query["redirect_uri"], query["state"]))
        })
    };
    let token = {
        let seen = seen.clone();
        post(move |headers: HeaderMap, body: String| async move {
            let authorization = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
            seen.lock().unwrap().push(format!("token {body} {authorization}"));
            axum::Json(json!({ "access_token": "good-token", "token_type": "Bearer", "expires_in": 3600, "refresh_token": "again" }))
        })
    };
    let mcp = post(move |Shared(base): Shared<String>, headers: HeaderMap, axum::Json(message): axum::Json<serde_json::Value>| async move {
        if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer good-token") {
            let challenge = format!("Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource\"");
            return (StatusCode::UNAUTHORIZED, [("www-authenticate", challenge)]).into_response();
        }
        let result = match message["method"].as_str() {
            Some("initialize") => json!({ "protocolVersion": "2025-06-18", "capabilities": { "tools": {} }, "serverInfo": { "name": "secure", "version": "0" } }),
            Some("tools/list") => json!({ "tools": [{ "name": "echo", "inputSchema": { "type": "object" }, "annotations": { "readOnlyHint": true } }] }),
            Some("tools/call") => json!({ "content": [{ "type": "text", "text": message["params"]["arguments"]["text"] }] }),
            _ if message.get("id").is_some() => return axum::Json(json!({ "jsonrpc": "2.0", "id": message["id"], "error": { "code": -32601, "message": "method not found" } })).into_response(),
            _ => return StatusCode::ACCEPTED.into_response(),
        };
        axum::Json(json!({ "jsonrpc": "2.0", "id": message["id"], "result": result })).into_response()
    });
    let routes = axum::Router::new()
        .route("/.well-known/oauth-protected-resource", get(resource("mcp")))
        .route("/.well-known/oauth-protected-resource/{path}", get(resource_at))
        .route("/.well-known/oauth-authorization-server", get(metadata))
        .route("/register", register)
        .route("/authorize", authorize)
        .route("/token", token)
        .route("/mcp", mcp.get(|| async { StatusCode::METHOD_NOT_ALLOWED }).delete(|| async { StatusCode::ACCEPTED }))
        .with_state(base.clone())
        .merge(legacy_sse_routes().route_layer(axum::middleware::from_fn(bearer_only)));
    tokio::spawn(async move { axum::serve(listener, routes).await.unwrap() });
    (base, seen)
}

/// The echo server in one era, logging every method it receives.
fn era_echo(era: &str) -> (ServerConfig, std::path::PathBuf) {
    let log = std::env::temp_dir().join(format!("drift-mcp-methods-{}.log", crate::random_hex(4)));
    let ServerConfig::Stdio { command, args, .. } = echo_config() else { unreachable!() };
    let env = [("ERA".to_string(), era.to_string()), ("METHOD_LOG".to_string(), log.to_string_lossy().to_string())].into_iter().collect();
    (ServerConfig::Stdio { command, args, env, cwd: None, timeout_seconds: None }, log)
}

#[tokio::test]
async fn a_v2_server_is_found_by_its_probe_and_spoken_to_without_a_handshake() {
    let engine = engine();
    let (config, log) = era_echo("v2");
    let row = saved(&engine, "modern", &config).await;
    engine.connect_mcp("modern").await.unwrap();
    let status = engine.mcp.status_of(row);
    assert_eq!((status.protocol.as_deref(), status.era), (Some("2026-07-28"), Some(Era::Stateless)));
    assert!(engine.mcp.instructions().iter().any(|(server, text)| server == "modern" && text.contains("Echo repeats")), "instructions come with discovery");
    // The fixture refuses any request without its protocol version in _meta, so a working call proves rmcp sends it.
    assert_eq!(tool(&engine, "modern_echo").run(&context(&engine), json!({ "text": "hi" })).await.unwrap().output, "hi");
    let methods = calls(&log);
    assert_eq!(methods.first().map(String::as_str), Some("server/discover"));
    assert!(!methods.iter().any(|m| m == "initialize" || m == "notifications/initialized"), "{methods:?}");
}

#[tokio::test]
async fn an_older_server_refusing_the_probe_gets_the_handshake_instead() {
    for era in ["legacy", "reject"] {
        let engine = engine();
        let (config, log) = era_echo(era);
        let row = saved(&engine, "old", &config).await;
        engine.connect_mcp("old").await.unwrap();
        let status = engine.mcp.status_of(row);
        assert_eq!((status.protocol.as_deref(), status.era), (Some("2025-06-18"), Some(Era::Legacy)), "{era}");
        assert_eq!(tool(&engine, "old_shout").run(&context(&engine), json!({ "text": "hi" })).await.unwrap().output, "HI");
        assert_eq!(calls(&log)[..2], ["server/discover", "initialize"], "{era}");
    }
}

#[tokio::test]
async fn a_slow_starting_v2_server_answers_the_probe_and_is_never_sent_the_handshake() {
    let engine = engine();
    let (mut config, log) = era_echo("v2");
    let ServerConfig::Stdio { env, .. } = &mut config else { unreachable!() };
    env.insert("START_DELAY_MS".into(), "800".into());
    let row = saved(&engine, "pulling", &config).await;
    engine.connect_mcp("pulling").await.unwrap();
    assert_eq!(engine.mcp.status_of(row).era, Some(Era::Stateless));
    assert!(!calls(&log).iter().any(|m| m == "initialize"), "a late answer to the probe is not talked over: {:?}", calls(&log));
}

#[test]
fn stdio_probes_alone_then_starts_afresh_for_the_handshake() {
    let stdio = echo_config();
    let http = ServerConfig::Http { url: "https://x.example/mcp".into(), headers: Default::default(), oauth: None, timeout_seconds: None };
    assert_eq!(attempts(&stdio, None), (vec![Some(Era::Stateless), Some(Era::Legacy)], true), "even a probe that goes unanswered moves on");
    assert_eq!(attempts(&stdio, Some(Era::Legacy)), (vec![Some(Era::Legacy), Some(Era::Stateless)], false));
    assert_eq!(attempts(&http, None), (vec![None], false), "over HTTP a legacy server says so at once, so rmcp's own fallback serves");
    assert_eq!(attempts(&http, Some(Era::Stateless)), (vec![Some(Era::Stateless), None], false));
}

#[tokio::test]
async fn the_era_found_is_kept_so_a_reconnect_skips_the_probe_until_a_save() {
    let engine = engine();
    let (config, log) = era_echo("ignore");
    let row = saved(&engine, "quiet", &config).await;
    engine.connect_mcp("quiet").await.unwrap();
    assert_eq!(engine.store.mcp_server("quiet").unwrap().unwrap().era, Some(Era::Legacy), "found after the probe went unanswered");
    engine.mcp.disconnect("quiet", &engine.store, &engine.hub).await;
    let again = std::time::Instant::now();
    engine.connect_mcp("quiet").await.unwrap();
    assert!(again.elapsed() < PROBE_WAIT / 2, "the reconnect did not wait on a probe: {:?}", again.elapsed());
    assert_eq!(calls(&log).iter().filter(|m| *m == "server/discover").count(), 1);
    assert_eq!(engine.store.save_mcp_server("quiet", &row.config).unwrap().era, None, "a save forgets it");
}

#[tokio::test]
async fn a_kept_era_the_server_no_longer_speaks_is_probed_again() {
    let engine = engine();
    let (config, log) = era_echo("v2");
    saved(&engine, "moved", &config).await;
    engine.store.set_mcp_era("moved", &config, Some(Era::Legacy)).unwrap();
    engine.connect_mcp("moved").await.unwrap();
    assert_eq!(calls(&log)[..2], ["initialize", "server/discover"], "the kept era first, then the probe");
    assert_eq!(engine.store.mcp_server("moved").unwrap().unwrap().era, Some(Era::Stateless));
}

#[tokio::test]
async fn a_tool_list_past_its_ttl_is_listed_again_when_a_turn_is_planned() {
    let engine = engine();
    let (mut config, log) = era_echo("v2");
    let ServerConfig::Stdio { env, .. } = &mut config else { unreachable!() };
    env.extend([("TOOLS_TTL_MS".to_string(), "100".to_string()), ("LATE_TOOL".to_string(), "1".to_string())]);
    let row = saved(&engine, "modern", &config).await;
    engine.connect_mcp("modern").await.unwrap();
    engine.mcp.refresh_stale(&engine.store, &engine.hub).await;
    assert!(find(&engine, "modern_late").is_none(), "still fresh: not asked again");
    let echo = tool(&engine, "modern_echo");
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let published = engine.hub.seq();
    engine.mcp.refresh_stale(&engine.store, &engine.hub).await;
    assert!(find(&engine, "modern_late").is_some(), "the next turn sees the new tool");
    assert!(engine.mcp.status_of(row).tools.iter().any(|t| t.name == "late"));
    assert!(engine.hub.seq() > published, "the menu hears the change");
    assert_eq!(calls(&log).iter().filter(|m| *m == "tools/list").count(), 2);
    assert_eq!(echo.run(&context(&engine), json!({ "text": "still" })).await.unwrap().output, "still", "an unchanged tool a turn holds still runs");

    let (legacy, legacy_log) = era_echo("legacy");
    saved(&engine, "old", &legacy).await;
    engine.connect_mcp("old").await.unwrap();
    engine.mcp.refresh_stale(&engine.store, &engine.hub).await;
    assert_eq!(calls(&legacy_log).iter().filter(|m| *m == "tools/list").count(), 1, "a list with no ttlMs is not asked again");
}

/// What a v2 HTTP server saw: each request's HTTP method and headers, and the JSON-RPC method it carried.
#[derive(Clone, Debug)]
struct Seen {
    verb: String,
    rpc: String,
    headers: BTreeMap<String, String>,
}

/// A 2026-07-28 server over HTTP: the first `flaky` (read-only) or `risky` call gets a 502, and text "ask" answers input_required forever.
async fn v2_http_server() -> (String, Arc<Mutex<Vec<Seen>>>) {
    use axum::http::{HeaderMap, Method, StatusCode};
    use axum::response::IntoResponse;
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let failed: Arc<Mutex<std::collections::HashSet<String>>> = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let schema = json!({ "type": "object", "properties": { "text": { "type": "string" }, "region": { "type": "string", "x-mcp-header": "Region" } } });
    let tool = |name: &str, read_only: bool| json!({ "name": name, "inputSchema": schema, "annotations": { "readOnlyHint": read_only } });
    let tools = json!([tool("echo", true), tool("flaky", true), tool("risky", false)]);
    let handler = {
        let seen = seen.clone();
        move |verb: Method, headers: HeaderMap, body: String| {
            let (seen, failed, tools) = (seen.clone(), failed.clone(), tools.clone());
            async move {
                let message: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let rpc = message["method"].as_str().unwrap_or_default().to_string();
                let headers = headers.iter().map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or_default().to_string())).collect();
                seen.lock().unwrap().push(Seen { verb: verb.to_string(), rpc: rpc.clone(), headers });
                if verb != Method::POST {
                    return StatusCode::METHOD_NOT_ALLOWED.into_response();
                }
                let name = message["params"]["name"].as_str().unwrap_or_default().to_string();
                let result = match rpc.as_str() {
                    "server/discover" => json!({ "resultType": "complete", "supportedVersions": ["2026-07-28"], "capabilities": { "tools": {} }, "ttlMs": 0, "cacheScope": "public", "_meta": { "io.modelcontextprotocol/serverInfo": { "name": "remote", "version": "0" } } }),
                    "tools/list" => json!({ "resultType": "complete", "tools": tools, "ttlMs": 0, "cacheScope": "public" }),
                    "tools/call" if name != "echo" && failed.lock().unwrap().insert(name.clone()) => return StatusCode::BAD_GATEWAY.into_response(),
                    "tools/call" if message["params"]["arguments"]["text"] == "ask" => json!({ "resultType": "input_required", "inputRequests": { "who": { "method": "elicitation/create", "params": { "message": "Who are you?", "requestedSchema": { "type": "object", "properties": { "name": { "type": "string" } } } } } }, "requestState": "s" }),
                    "tools/call" => json!({ "resultType": "complete", "content": [{ "type": "text", "text": format!("{name}: {}", message["params"]["arguments"]["text"].as_str().unwrap_or_default()) }] }),
                    _ if message.get("id").is_some() => return axum::Json(json!({ "jsonrpc": "2.0", "id": message["id"], "error": { "code": -32601, "message": "method not found" } })).into_response(),
                    _ => return StatusCode::ACCEPTED.into_response(),
                };
                axum::Json(json!({ "jsonrpc": "2.0", "id": message["id"], "result": result })).into_response()
            }
        }
    };
    let app = axum::Router::new().route("/mcp", axum::routing::any(handler));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("{base}/mcp"), seen)
}

#[tokio::test]
async fn a_v2_server_over_http_gets_one_post_per_request_with_its_headers_and_no_session() {
    let engine = engine();
    let (url, seen) = v2_http_server().await;
    let row = saved(&engine, "remote", &ServerConfig::Http { url, headers: Default::default(), oauth: None, timeout_seconds: None }).await;
    engine.connect_mcp("remote").await.unwrap();
    assert_eq!(engine.mcp.status_of(row).era, Some(Era::Stateless));
    let out = tool(&engine, "remote_echo").run(&context(&engine), json!({ "text": "hi", "region": "eu-west" })).await.unwrap();
    assert_eq!(out.output, "echo: hi");
    let seen = seen.lock().unwrap().clone();
    assert!(seen.iter().all(|s| s.verb == "POST"), "no GET stream, no DELETE of a session: {seen:?}");
    assert!(seen.iter().all(|s| !s.headers.contains_key("mcp-session-id")));
    assert!(seen.iter().filter(|s| !s.rpc.is_empty()).all(|s| s.headers.get("mcp-protocol-version").map(String::as_str) == Some("2026-07-28") && s.headers.get("mcp-method") == Some(&s.rpc)), "{seen:?}");
    let call = seen.iter().find(|s| s.rpc == "tools/call").unwrap();
    assert_eq!((call.headers.get("mcp-name").map(String::as_str), call.headers.get("mcp-param-region").map(String::as_str)), (Some("echo"), Some("eu-west")));
}

#[tokio::test]
async fn a_failed_post_to_a_stateless_server_is_asked_again_only_when_read_only() {
    let engine = engine();
    let (url, _) = v2_http_server().await;
    saved(&engine, "remote", &ServerConfig::Http { url, headers: Default::default(), oauth: None, timeout_seconds: None }).await;
    engine.connect_mcp("remote").await.unwrap();
    let started = std::time::Instant::now();
    assert_eq!(tool(&engine, "remote_flaky").run(&context(&engine), json!({ "text": "again" })).await.unwrap().output, "flaky: again");
    assert!(started.elapsed() < REPLACEMENT_WAIT, "asked again on the same client, not after waiting for a reconnect");
    let risky = tool(&engine, "remote_risky").run(&context(&engine), json!({ "text": "once" })).await.unwrap_err().0;
    assert!(risky.contains("may or may not have taken effect"), "{risky}");
}

#[tokio::test]
async fn a_server_asking_for_input_is_declined_and_the_call_fails_rather_than_hangs() {
    let engine = engine();
    let (url, seen) = v2_http_server().await;
    saved(&engine, "remote", &ServerConfig::Http { url, headers: Default::default(), oauth: None, timeout_seconds: None }).await;
    engine.connect_mcp("remote").await.unwrap();
    let failed = tokio::time::timeout(std::time::Duration::from_secs(10), tool(&engine, "remote_echo").run(&context(&engine), json!({ "text": "ask" }))).await.expect("it ends").unwrap_err().0;
    assert!(failed.contains("kept asking for input"), "{failed}");
    let retries = seen.lock().unwrap().iter().filter(|s| s.rpc == "tools/call").count();
    assert!(retries > 1, "each ask was answered and the request retried with the answer, not left waiting");
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
    let (ServerConfig::Stdio { command, args, mut env, .. }, log) = logged_echo() else { unreachable!() };
    env.insert("REDEFINE_AFTER_CRASH".into(), "1".into());
    saved(&engine, "echo", &ServerConfig::Stdio { command, args, env, cwd: None, timeout_seconds: None }).await;
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
    let ServerConfig::Stdio { command, args, mut env, .. } = config else { unreachable!() };
    env.insert("CHANGED".into(), "1".into());
    engine.mcp.change("echo", &engine.store, &engine.hub, |store| store.save_mcp_server("echo", &ServerConfig::Stdio { command, args, env, cwd: None, timeout_seconds: None })).await.unwrap();
    assert!(find(&engine, "echo_echo").is_none(), "later turns see the server once it connects on the new command");
    assert_eq!(echo.run(&context(&engine), json!({ "text": "still" })).await.unwrap().output, "still", "the running turn keeps its client");
}

#[test]
fn reconnect_backoff_resets_only_after_a_connection_that_held() {
    let carried = FIRST_RETRY * 8;
    assert_eq!(after_loss(STABLE / 2, carried), carried, "a connection that dropped at once keeps backing off");
    assert_eq!(after_loss(STABLE, carried), FIRST_RETRY);
}
