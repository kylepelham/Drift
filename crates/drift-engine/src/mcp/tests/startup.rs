use super::*;

/// Records a slow server's pid and its grandchild's pid so tests can verify process-tree cleanup.
fn traced(env: &[(&str, &str)]) -> (ServerConfig, std::path::PathBuf) {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/slow-server.cjs");
    let pids = std::env::temp_dir().join(format!("drift-mcp-pids-{}", crate::random_hex(4)));
    let mut variables: BTreeMap<String, String> = env
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    variables.insert("PID_FILE".into(), pids.to_string_lossy().into());
    variables.insert("GRANDCHILD".into(), "1".into());

    (
        ServerConfig::Stdio {
            command: "node".into(),
            args: vec![script.into()],
            env: variables,
            cwd: None,
            timeout_seconds: None,
        },
        pids,
    )
}

/// Saves a known legacy era so startup uses one handshake process rather than probing first.
async fn saved_legacy(engine: &Arc<crate::Engine>, name: &str, config: &ServerConfig) -> ServerRow {
    saved(engine, name, config).await;
    engine.store.set_mcp_era(name, config, Some(Era::Legacy)).unwrap();

    engine.store.mcp_server(name).unwrap().unwrap()
}

async fn pids_in(file: &std::path::Path) -> Vec<u32> {
    let read = || {
        std::fs::read_to_string(file)
            .ok()
            .filter(|text| text.split_whitespace().count() == 2)
    };
    until("the server wrote its pids", || read().is_some()).await;
    let pids: Vec<u32> = read()
        .unwrap()
        .split_whitespace()
        .map(|pid| pid.parse().unwrap())
        .collect();
    assert!(
        pids.iter().all(|pid| alive(*pid)),
        "the server and its child are running"
    );

    pids
}

fn alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let listed = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&listed.stdout).contains(&format!(" {pid} "))
    }
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .is_ok_and(|status| status.success())
    }
}

async fn all_dead(pids: &[u32]) {
    until("the server and its child are gone", || {
        pids.iter().all(|pid| !alive(*pid))
    })
    .await;
}

#[tokio::test]
async fn a_server_that_never_finishes_starting_times_out_and_its_process_tree_dies() {
    let engine = engine();
    let (config, file) = traced(&[("SLOW_MS", "600000")]);
    // A remembered legacy era makes this test exercise only the startup timeout.
    let row = saved_legacy(&engine, "mute", &config).await;
    let connecting = tokio::spawn({
        let engine = engine.clone();
        async move { engine.connect_mcp_in("mute", Some(&here())).await }
    });
    let pids = pids_in(&file).await;

    let failed = connecting.await.unwrap().unwrap_err();
    assert!(failed.to_string().contains("did not start within"), "{failed}");
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
        async move { engine.connect_mcp_in("quiet", Some(&here())).await }
    });
    let pids = pids_in(&file).await;

    let failed = connecting.await.unwrap().unwrap_err();
    assert!(failed.to_string().contains("did not list its tools within"), "{failed}");

    all_dead(&pids).await;
}

#[tokio::test]
async fn disconnecting_cancels_a_connect_in_flight_and_kills_what_it_started() {
    let engine = engine();
    let (config, file) = traced(&[("SLOW_MS", "600000")]);
    let row = saved_legacy(&engine, "slow", &config).await;
    let connecting = tokio::spawn({
        let engine = engine.clone();
        async move { engine.connect_mcp_in("slow", Some(&here())).await }
    });
    let pids = pids_in(&file).await;

    engine.mcp.disconnect("slow", &engine.store, &engine.hub).await;
    let ended = tokio::time::timeout(std::time::Duration::from_secs(1), connecting)
        .await
        .expect("the connect ends at once");
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
        async move { engine.connect_mcp_in("dropped", Some(&here())).await }
    });
    let pids = pids_in(&file).await;
    assert!(engine.mcp.connecting());

    connecting.abort();
    until("the attempt settled", || !engine.mcp.connecting()).await;
    assert_eq!(
        engine.mcp.status_of(row).state,
        State::Disconnected,
        "not left connecting"
    );
    let started = std::time::Instant::now();
    engine.mcp.wait_ready(READY_WAIT).await;
    assert!(
        started.elapsed() < std::time::Duration::from_millis(100),
        "a planning turn does not wait on it"
    );

    all_dead(&pids).await;
}

#[tokio::test]
async fn the_startup_sweep_never_cancels_a_connect_already_under_way() {
    let engine = engine();
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/slow-server.cjs");
    let slow = ServerConfig::Stdio {
        command: "node".into(),
        args: vec![script.into()],
        env: [("SLOW_MS".to_string(), "500".to_string())].into(),
        cwd: None,
        timeout_seconds: None,
    };
    let row = saved(&engine, "slow", &slow).await;
    let connecting = tokio::spawn({
        let engine = engine.clone();
        async move { engine.connect_mcp_in("slow", Some(&here())).await }
    });
    until("connecting", || engine.mcp.connecting()).await;

    engine.connect_all_mcp();
    connecting.await.unwrap().expect("the user's connect lands");
    assert_eq!(engine.mcp.status_of(row).state, State::Connected);
}

#[tokio::test]
async fn a_workspaces_servers_connect_at_once_and_a_dead_one_holds_up_no_other() {
    let engine = engine();
    let (mute, _) = traced(&[("SLOW_MS", "600000")]);
    saved_legacy(&engine, "mute", &mute).await;
    let fine = saved(&engine, "echo", &echo_config()).await;
    let started = std::time::Instant::now();

    engine.connect_all_mcp();
    assert!(
        !engine.mcp.connecting(),
        "the startup sweep starts remote servers only; stdio ones start per workspace"
    );
    engine.start_workspace_mcp(&here());
    assert!(
        engine.mcp.connecting(),
        "every connect has begun before the sweep returns"
    );
    until("echo connects", || {
        engine.mcp.status_of(fine.clone()).state == State::Connected
    })
    .await;
    assert!(
        started.elapsed() < STEP_LIMIT,
        "echo did not wait for the server that never answers: {:?}",
        started.elapsed()
    );
    assert!(engine.mcp.connecting(), "the dead one is still trying on its own");
}

#[tokio::test]
async fn a_newer_connect_supersedes_one_in_flight() {
    let engine = engine();
    let (config, file) = traced(&[("SLOW_MS", "600000")]);
    saved_legacy(&engine, "twice", &config).await;
    let first = tokio::spawn({
        let engine = engine.clone();
        async move { engine.connect_mcp_in("twice", Some(&here())).await }
    });
    let pids = pids_in(&file).await;
    std::fs::remove_file(&file).unwrap();

    let _second = tokio::spawn({
        let engine = engine.clone();
        async move { engine.connect_mcp_in("twice", Some(&here())).await }
    });
    let ended = tokio::time::timeout(std::time::Duration::from_secs(1), first)
        .await
        .expect("the first ends at once");
    assert!(ended.unwrap().is_err());

    all_dead(&pids).await;
}
