use super::*;

/// The tools a turn in `workspace` is offered from `server`.
fn offered(engine: &Engine, workspace: &std::path::Path, server: &str) -> usize {
    engine
        .mcp
        .tools(&engine.store, Some(workspace))
        .iter()
        .filter(|tool| tool.server() == Some(server))
        .count()
}

struct WorkspaceServer {
    engine: Arc<Engine>,
    dir: std::path::PathBuf,
    re: crate::store::Workspace,
    web: crate::store::Workspace,
    re_path: std::path::PathBuf,
    web_path: std::path::PathBuf,
}

fn workspace_server() -> WorkspaceServer {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = std::env::temp_dir().join(format!("drift-api-mcp-{}", crate::random_hex(4)));
    let (re_dir, web_dir) = (dir.join("re"), dir.join("web"));
    std::fs::create_dir_all(&re_dir).unwrap();
    std::fs::create_dir_all(&web_dir).unwrap();
    let engine = Engine::open_with(
        &dir.join("data"),
        crate::Options {
            file_credentials: true,
            ..Default::default()
        },
    )
    .unwrap();
    let re = engine.store.add_workspace(&re_dir.to_string_lossy(), "re", "").unwrap();
    let web = engine
        .store
        .add_workspace(&web_dir.to_string_lossy(), "web", "")
        .unwrap();
    let (re_path, web_path) = (crate::tool::canonical(&re_dir), crate::tool::canonical(&web_dir));
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/echo-server.cjs");
    let config = crate::mcp::ServerConfig::Stdio {
        command: "node".into(),
        args: vec![script.into()],
        env: Default::default(),
        cwd: None,
        timeout_seconds: None,
    };
    engine.store.save_mcp_server("ida", &config).unwrap();
    engine.store.set_mcp_enabled("ida", false).unwrap();

    WorkspaceServer {
        engine,
        dir,
        re,
        web,
        re_path,
        web_path,
    }
}

#[tokio::test]
async fn a_server_turned_on_in_one_workspace_is_offered_there_only_and_remembered() {
    let WorkspaceServer {
        engine,
        dir,
        re,
        web,
        re_path,
        web_path,
    } = workspace_server();

    let path = || Path("ida".to_string());
    let query = |id: &str| {
        Query(ConnectQuery {
            workspace: Some(id.to_string()),
        })
    };

    let status = connect_route(State(engine.clone()), path(), query(&re.id))
        .await
        .unwrap()
        .0;
    assert_eq!(status.state, crate::mcp::State::Connected, "{:?}", status.error);
    engine.start_workspace_mcp(&web_path);
    engine.mcp.wait_ready(std::time::Duration::from_secs(5)).await;
    assert!(offered(&engine, &re_path, "ida") > 0);
    assert_eq!(
        offered(&engine, &web_path, "ida"),
        0,
        "off by its switch, and the other workspace never chose it"
    );
    assert!(
        !engine.mcp.connecting(),
        "nothing started for the workspace that did not choose it"
    );

    assert_workspace_choice_saved(&dir, &re_path, &web_path);

    let status = disconnect(State(engine.clone()), path(), query(&re.id))
        .await
        .unwrap()
        .0;
    assert_eq!(
        status.state,
        crate::mcp::State::Disabled,
        "off in its only workspace, it is off everywhere"
    );
    assert_eq!(offered(&engine, &re_path, "ida"), 0);

    let _ = set_enabled(
        State(engine.clone()),
        path(),
        query(&web.id),
        Json(EnabledBody { enabled: true }),
    )
    .await
    .unwrap();
    let _ = disconnect(State(engine.clone()), path(), query(&web.id)).await.unwrap();
    engine.start_workspace_mcp(&re_path);
    engine.mcp.wait_ready(std::time::Duration::from_secs(5)).await;
    assert!(
        offered(&engine, &re_path, "ida") > 0,
        "on by its switch everywhere else"
    );
    assert_eq!(
        offered(&engine, &web_path, "ida"),
        0,
        "turned off in this workspace only"
    );
    engine.start_workspace_mcp(&web_path);
    assert!(
        !engine.mcp.connecting(),
        "a workspace that turned it off does not start it again"
    );
    drop(engine);
    std::fs::remove_dir_all(dir).ok();
}

fn assert_workspace_choice_saved(dir: &std::path::Path, re_path: &std::path::Path, web_path: &std::path::Path) {
    let reopened = Engine::open_with(
        &dir.join("data"),
        crate::Options {
            file_credentials: true,
            ..Default::default()
        },
    )
    .unwrap();
    let row = reopened.store.mcp_server("ida").unwrap().unwrap();

    assert!(
        row.on_in(re_path) && !row.on_in(web_path),
        "the choice outlives a restart"
    );
}

#[tokio::test]
async fn switches_on_a_server_that_does_not_parse_write_nothing() {
    let dir = std::env::temp_dir().join(format!("drift-api-mcp-{}", crate::random_hex(4)));
    let engine = Engine::open_with(
        &dir,
        crate::Options {
            file_credentials: true,
            ..Default::default()
        },
    )
    .unwrap();
    let config = crate::mcp::ServerConfig::Stdio {
        command: "npx".into(),
        args: vec![],
        env: Default::default(),
        cwd: None,
        timeout_seconds: None,
    };
    engine.store.save_mcp_server("newer", &config).unwrap();
    engine.store.set_mcp_enabled("newer", false).unwrap();
    engine
        .store
        .lock()
        .execute("UPDATE mcp_config SET config_json = '{\"type\":\"future\"}'", [])
        .unwrap();
    let path = || Path("newer".to_string());
    assert!(
        set_enabled(
            State(engine.clone()),
            path(),
            Query(ConnectQuery { workspace: None }),
            Json(EnabledBody { enabled: true })
        )
        .await
        .is_err()
    );
    let enabled: bool = engine
        .store
        .lock()
        .query_row("SELECT enabled FROM mcp_config", [], |row| row.get(0))
        .unwrap();
    assert!(!enabled, "refused before anything was written");
    std::fs::remove_dir_all(dir).ok();
}
