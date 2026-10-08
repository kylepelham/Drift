use super::*;

#[test]
fn local_routes_wait_longer_and_drift_json_can_set_any_routes_limits() {
    use crate::llm::http::Timeouts;
    assert_eq!(Timeouts::for_route("ollama").headers, Duration::from_secs(600));
    assert_eq!(Timeouts::for_route("lmstudio").idle, Duration::from_secs(600));
    assert_eq!(Timeouts::for_route("anthropic"), Timeouts::default());
    let _ = rustls::crypto::ring::default_provider().install_default();
    let ollama = crate::llm::provider_for("ollama", None).unwrap();
    assert_eq!(
        ollama.timeouts().unwrap().headers,
        Duration::from_secs(600),
        "the route's own defaults apply when it is built"
    );

    let dir = std::env::temp_dir().join(format!("drift-timeouts-{}", crate::random_hex(4)));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("drift.json"),
        r#"{ "timeouts": { "ollama": { "headersSeconds": 1800 }, "anthropic": { "idleSeconds": 60 } } }"#,
    )
    .unwrap();
    let config = Config::load_with_home(&dir, None);
    assert_eq!(
        config.route_timeouts("ollama"),
        Timeouts {
            headers: Duration::from_secs(1800),
            idle: Duration::from_secs(600)
        }
    );
    assert_eq!(
        config.route_timeouts("anthropic"),
        Timeouts {
            headers: Duration::from_secs(120),
            idle: Duration::from_secs(60)
        }
    );
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn a_local_servers_installed_models_appear_and_run_without_a_key_while_it_answers() {
    let app = axum::Router::new().route(
        "/v1/models",
        axum::routing::get(|| async { axum::Json(json!({ "data": [{ "id": "qwen3-coder-local" }] })) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let serving = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let h = harness().await;
    h.engine
        .catalog
        .write()
        .unwrap()
        .providers
        .get_mut("lmstudio")
        .unwrap()
        .api = Some(base);
    h.engine.ask_local().await;

    let local = ModelRef {
        provider: "lmstudio".into(),
        model: "qwen3-coder-local".into(),
    };
    assert!(
        h.engine
            .catalog
            .read()
            .unwrap()
            .model("lmstudio", "qwen3-coder-local")
            .is_some(),
        "the installed model is listed"
    );
    h.provider.push(text("local reply"));
    let mut ask = prompt("hi");
    ask.model = Some(local);
    h.engine.submit(&h.session.id, ask).await.await_ok();
    until_idle(&h).await;
    assert_eq!(transcript(&h)[1].info.status, MessageStatus::Done, "no key was needed");

    serving.abort();
    let _ = serving.await;
    h.engine.ask_local().await;
    assert!(
        h.engine.credentials.resolve("lmstudio", &[]).is_none(),
        "a server that stopped answering is not connected"
    );
}
