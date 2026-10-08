use super::{Arc, Context, ServerConfig, ServerRow, SessionFiles};

pub(super) fn echo_config() -> ServerConfig {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/echo-server.cjs");

    ServerConfig::Stdio {
        command: "node".into(),
        args: vec![script.into()],
        env: Default::default(),
        cwd: None,
        timeout_seconds: None,
    }
}

/// The JSON-RPC reply a fake server gives a request for a method it does not serve.
pub(super) fn method_not_found(id: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "method not found" } })
}

pub(super) fn engine() -> Arc<crate::Engine> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let directory = std::env::temp_dir().join(format!("drift-mcp-{}", crate::random_hex(4)));
    let options = crate::Options {
        file_credentials: true,
        ..Default::default()
    };

    crate::Engine::open_with(&directory, options).unwrap()
}

/// The workspace the tests' stdio servers run in.
pub(super) fn here() -> std::path::PathBuf {
    crate::tool::canonical(&std::env::temp_dir())
}

pub(super) fn context(engine: &Arc<crate::Engine>) -> Context {
    Context {
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
    }
}

pub(super) async fn saved(engine: &Arc<crate::Engine>, name: &str, config: &ServerConfig) -> ServerRow {
    engine.store.save_mcp_server(name, config).unwrap()
}

pub(super) async fn until<F: Fn() -> bool>(what: &str, done: F) {
    for _ in 0..300 {
        if done() {
            return;
        }

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    panic!("never: {what}");
}

pub(super) fn find(engine: &crate::Engine, name: &str) -> Option<Arc<dyn crate::tool::Tool>> {
    engine
        .offered_tools(crate::llm::catalog::ToolProfile::Edit, Some(&here()))
        .into_iter()
        .find(|tool| tool.spec().name == name)
}

pub(super) fn tool(engine: &crate::Engine, name: &str) -> Arc<dyn crate::tool::Tool> {
    find(engine, name).unwrap()
}

pub(super) fn calls(log: &std::path::Path) -> Vec<String> {
    let contents = std::fs::read_to_string(log).unwrap_or_default();

    contents.lines().map(String::from).collect()
}
