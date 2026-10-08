use super::*;

/// The HTTP method, JSON-RPC method and headers recorded for one stateless request.
#[derive(Clone, Debug)]
struct Seen {
    verb: String,
    rpc: String,
    headers: BTreeMap<String, String>,
}

/// Runs echo in one protocol era and logs every received method.
fn era_echo(era: &str) -> (ServerConfig, std::path::PathBuf) {
    let log = std::env::temp_dir().join(format!("drift-mcp-methods-{}.log", crate::random_hex(4)));
    let ServerConfig::Stdio { command, args, .. } = echo_config() else {
        unreachable!()
    };
    let env = [
        ("ERA".to_string(), era.to_string()),
        ("METHOD_LOG".to_string(), log.to_string_lossy().to_string()),
    ]
    .into_iter()
    .collect();

    (
        ServerConfig::Stdio {
            command,
            args,
            env,
            cwd: None,
            timeout_seconds: None,
        },
        log,
    )
}

#[tokio::test]
async fn a_v2_server_is_found_by_its_probe_and_spoken_to_without_a_handshake() {
    let engine = engine();
    let (config, log) = era_echo("v2");
    let row = saved(&engine, "modern", &config).await;
    engine.connect_mcp_in("modern", Some(&here())).await.unwrap();

    let status = engine.mcp.status_of(row);
    assert_eq!(
        (status.protocol.as_deref(), status.era),
        (Some("2026-07-28"), Some(Era::Stateless))
    );
    assert!(
        engine
            .mcp
            .instructions(Some(&here()))
            .iter()
            .any(|(server, text)| server == "modern" && text.contains("Echo repeats")),
        "instructions come with discovery"
    );
    // The fixture rejects missing protocol metadata, so a successful call verifies rmcp sends it.
    assert_eq!(
        tool(&engine, "modern_echo")
            .run(&context(&engine), json!({ "text": "hi" }))
            .await
            .unwrap()
            .output,
        "hi"
    );

    let methods = calls(&log);
    assert_eq!(methods.first().map(String::as_str), Some("server/discover"));
    assert!(
        !methods
            .iter()
            .any(|method| method == "initialize" || method == "notifications/initialized"),
        "{methods:?}"
    );
}

#[tokio::test]
async fn an_older_server_refusing_the_probe_gets_the_handshake_instead() {
    for era in ["legacy", "reject"] {
        let engine = engine();
        let (config, log) = era_echo(era);
        let row = saved(&engine, "old", &config).await;
        engine.connect_mcp_in("old", Some(&here())).await.unwrap();

        let status = engine.mcp.status_of(row);
        assert_eq!(
            (status.protocol.as_deref(), status.era),
            (Some("2025-06-18"), Some(Era::Legacy)),
            "{era}"
        );
        assert_eq!(
            tool(&engine, "old_shout")
                .run(&context(&engine), json!({ "text": "hi" }))
                .await
                .unwrap()
                .output,
            "HI"
        );
        assert_eq!(calls(&log)[..2], ["server/discover", "initialize"], "{era}");
    }
}

#[tokio::test]
async fn a_slow_starting_v2_server_answers_the_probe_and_is_never_sent_the_handshake() {
    let engine = engine();
    let (mut config, log) = era_echo("v2");
    let ServerConfig::Stdio { env, .. } = &mut config else {
        unreachable!()
    };
    env.insert("START_DELAY_MS".into(), "800".into());
    let row = saved(&engine, "pulling", &config).await;

    engine.connect_mcp_in("pulling", Some(&here())).await.unwrap();
    assert_eq!(engine.mcp.status_of(row).era, Some(Era::Stateless));
    assert!(
        !calls(&log).iter().any(|method| method == "initialize"),
        "a late answer to the probe is not talked over: {:?}",
        calls(&log)
    );
}

#[test]
fn stdio_probes_alone_then_starts_afresh_for_the_handshake() {
    let stdio = echo_config();
    let http = ServerConfig::Http {
        url: "https://x.example/mcp".into(),
        headers: Default::default(),
        oauth: None,
        timeout_seconds: None,
    };

    assert_eq!(
        attempts(&stdio, None),
        (vec![Some(Era::Stateless), Some(Era::Legacy)], true),
        "even a probe that goes unanswered moves on"
    );
    assert_eq!(
        attempts(&stdio, Some(Era::Legacy)),
        (vec![Some(Era::Legacy), Some(Era::Stateless)], false)
    );
    assert_eq!(
        attempts(&http, None),
        (vec![None], false),
        "over HTTP a legacy server says so at once, so rmcp's own fallback serves"
    );
    assert_eq!(
        attempts(&http, Some(Era::Stateless)),
        (vec![Some(Era::Stateless), None], false)
    );
}

#[tokio::test]
async fn the_era_found_is_kept_so_a_reconnect_skips_the_probe_until_a_save() {
    let engine = engine();
    let (config, log) = era_echo("ignore");
    let row = saved(&engine, "quiet", &config).await;
    engine.connect_mcp_in("quiet", Some(&here())).await.unwrap();
    assert_eq!(
        engine.store.mcp_server("quiet").unwrap().unwrap().era,
        Some(Era::Legacy),
        "found after the probe went unanswered"
    );

    engine.mcp.disconnect("quiet", &engine.store, &engine.hub).await;
    let again = std::time::Instant::now();
    engine.connect_mcp_in("quiet", Some(&here())).await.unwrap();
    assert!(
        again.elapsed() < PROBE_WAIT / 2,
        "the reconnect did not wait on a probe: {:?}",
        again.elapsed()
    );
    assert_eq!(
        calls(&log).iter().filter(|method| *method == "server/discover").count(),
        1
    );

    assert_eq!(
        engine.store.save_mcp_server("quiet", &row.config).unwrap().era,
        None,
        "a save forgets it"
    );
}

#[tokio::test]
async fn a_kept_era_the_server_no_longer_speaks_is_probed_again() {
    let engine = engine();
    let (config, log) = era_echo("v2");
    saved(&engine, "moved", &config).await;
    engine.store.set_mcp_era("moved", &config, Some(Era::Legacy)).unwrap();

    engine.connect_mcp_in("moved", Some(&here())).await.unwrap();
    assert_eq!(
        calls(&log)[..2],
        ["initialize", "server/discover"],
        "the kept era first, then the probe"
    );
    assert_eq!(
        engine.store.mcp_server("moved").unwrap().unwrap().era,
        Some(Era::Stateless)
    );
}

#[tokio::test]
async fn a_tool_list_past_its_ttl_is_listed_again_when_a_turn_is_planned() {
    let engine = engine();
    let (mut config, log) = era_echo("v2");
    let ServerConfig::Stdio { env, .. } = &mut config else {
        unreachable!()
    };
    env.extend([
        ("TOOLS_TTL_MS".to_string(), "100".to_string()),
        ("LATE_TOOL".to_string(), "1".to_string()),
    ]);
    let row = saved(&engine, "modern", &config).await;
    engine.connect_mcp_in("modern", Some(&here())).await.unwrap();

    engine.mcp.refresh_stale(&engine.store, &engine.hub).await;
    assert!(find(&engine, "modern_late").is_none(), "still fresh: not asked again");
    let echo = tool(&engine, "modern_echo");
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let published = engine.hub.seq();
    engine.mcp.refresh_stale(&engine.store, &engine.hub).await;
    assert!(
        find(&engine, "modern_late").is_some(),
        "the next turn sees the new tool"
    );
    assert!(engine.mcp.status_of(row).tools.iter().any(|tool| tool.name == "late"));
    assert!(engine.hub.seq() > published, "the menu hears the change");
    assert_eq!(calls(&log).iter().filter(|method| *method == "tools/list").count(), 2);
    assert_eq!(
        echo.run(&context(&engine), json!({ "text": "still" }))
            .await
            .unwrap()
            .output,
        "still",
        "an unchanged tool a turn holds still runs"
    );

    let (legacy, legacy_log) = era_echo("legacy");
    saved(&engine, "old", &legacy).await;
    engine.connect_mcp_in("old", Some(&here())).await.unwrap();
    engine.mcp.refresh_stale(&engine.store, &engine.hub).await;
    assert_eq!(
        calls(&legacy_log)
            .iter()
            .filter(|method| *method == "tools/list")
            .count(),
        1,
        "a list with no ttlMs is not asked again"
    );
}

/// Returns one 502 on each non-echo tool's first call and repeated `input_required` for text "ask".
async fn v2_http_server() -> (String, Arc<Mutex<Vec<Seen>>>) {
    use axum::http::{HeaderMap, Method, StatusCode};
    use axum::response::IntoResponse;

    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let failed: Arc<Mutex<std::collections::HashSet<String>>> = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let schema = json!({ "type": "object", "properties": {
        "text": { "type": "string" },
        "region": { "type": "string", "x-mcp-header": "Region" },
    } });
    let tool = |name: &str, read_only: bool| {
        json!({
            "name": name,
            "inputSchema": schema,
            "annotations": { "readOnlyHint": read_only }
        })
    };
    let tools = json!([tool("echo", true), tool("flaky", true), tool("risky", false)]);

    let handler = {
        let seen = seen.clone();
        move |verb: Method, headers: HeaderMap, body: String| {
            let (seen, failed, tools) = (seen.clone(), failed.clone(), tools.clone());
            async move {
                let message: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let rpc = message["method"].as_str().unwrap_or_default().to_string();
                let headers = headers
                    .iter()
                    .map(|(name, value)| {
                        (
                            name.as_str().to_string(),
                            value.to_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect();
                seen.lock().unwrap().push(Seen {
                    verb: verb.to_string(),
                    rpc: rpc.clone(),
                    headers,
                });
                if verb != Method::POST {
                    return StatusCode::METHOD_NOT_ALLOWED.into_response();
                }

                let name = message["params"]["name"].as_str().unwrap_or_default().to_string();
                let text = message["params"]["arguments"]["text"].as_str().unwrap_or_default();
                let result = match rpc.as_str() {
                    "server/discover" => json!({
                        "resultType": "complete",
                        "supportedVersions": ["2026-07-28"],
                        "capabilities": { "tools": {} },
                        "ttlMs": 0,
                        "cacheScope": "public",
                        "_meta": { "io.modelcontextprotocol/serverInfo": { "name": "remote", "version": "0" } },
                    }),
                    "tools/list" => {
                        json!({ "resultType": "complete", "tools": tools, "ttlMs": 0, "cacheScope": "public" })
                    }
                    "tools/call" if name != "echo" && failed.lock().unwrap().insert(name.clone()) => {
                        return StatusCode::BAD_GATEWAY.into_response();
                    }
                    "tools/call" if text == "ask" => json!({
                        "resultType": "input_required",
                        "inputRequests": { "who": { "method": "elicitation/create", "params": {
                            "message": "Who are you?",
                            "requestedSchema": { "type": "object", "properties": { "name": { "type": "string" } } },
                        } } },
                        "requestState": "s",
                    }),
                    "tools/call" => {
                        let content = json!([{ "type": "text", "text": format!("{name}: {text}") }]);
                        json!({ "resultType": "complete", "content": content })
                    }
                    _ if message.get("id").is_some() => {
                        return axum::Json(method_not_found(&message["id"])).into_response();
                    }
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
    let config = ServerConfig::Http {
        url,
        headers: Default::default(),
        oauth: None,
        timeout_seconds: None,
    };
    let row = saved(&engine, "remote", &config).await;
    engine.connect_mcp_in("remote", Some(&here())).await.unwrap();
    assert_eq!(engine.mcp.status_of(row).era, Some(Era::Stateless));

    let output = tool(&engine, "remote_echo")
        .run(&context(&engine), json!({ "text": "hi", "region": "eu-west" }))
        .await
        .unwrap();
    assert_eq!(output.output, "echo: hi");
    let seen = seen.lock().unwrap().clone();
    assert!(
        seen.iter().all(|request| request.verb == "POST"),
        "no GET stream, no DELETE of a session: {seen:?}"
    );
    assert!(
        seen.iter()
            .all(|request| !request.headers.contains_key("mcp-session-id"))
    );
    assert!(
        seen.iter().filter(|request| !request.rpc.is_empty()).all(|request| {
            request.headers.get("mcp-protocol-version").map(String::as_str) == Some("2026-07-28")
                && request.headers.get("mcp-method") == Some(&request.rpc)
        }),
        "{seen:?}"
    );

    let call = seen.iter().find(|request| request.rpc == "tools/call").unwrap();
    assert_eq!(
        (
            call.headers.get("mcp-name").map(String::as_str),
            call.headers.get("mcp-param-region").map(String::as_str)
        ),
        (Some("echo"), Some("eu-west"))
    );
}

#[tokio::test]
async fn a_failed_post_to_a_stateless_server_is_asked_again_only_when_read_only() {
    let engine = engine();
    let (url, _) = v2_http_server().await;
    let config = ServerConfig::Http {
        url,
        headers: Default::default(),
        oauth: None,
        timeout_seconds: None,
    };
    saved(&engine, "remote", &config).await;
    engine.connect_mcp_in("remote", Some(&here())).await.unwrap();

    let started = std::time::Instant::now();
    let output = tool(&engine, "remote_flaky")
        .run(&context(&engine), json!({ "text": "again" }))
        .await
        .unwrap();
    assert_eq!(output.output, "flaky: again");
    assert!(
        started.elapsed() < REPLACEMENT_WAIT,
        "asked again on the same client, not after waiting for a reconnect"
    );

    let risky = tool(&engine, "remote_risky")
        .run(&context(&engine), json!({ "text": "once" }))
        .await
        .unwrap_err()
        .0;
    assert!(risky.contains("may or may not have taken effect"), "{risky}");
}

#[tokio::test]
async fn a_server_asking_for_input_is_declined_and_the_call_fails_rather_than_hangs() {
    let engine = engine();
    let (url, seen) = v2_http_server().await;
    let config = ServerConfig::Http {
        url,
        headers: Default::default(),
        oauth: None,
        timeout_seconds: None,
    };
    saved(&engine, "remote", &config).await;
    engine.connect_mcp_in("remote", Some(&here())).await.unwrap();

    let failed = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tool(&engine, "remote_echo").run(&context(&engine), json!({ "text": "ask" })),
    )
    .await
    .expect("it ends")
    .unwrap_err()
    .0;
    assert!(failed.contains("kept asking for input"), "{failed}");
    let retries = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.rpc == "tools/call")
        .count();
    assert!(
        retries > 1,
        "each ask was answered and the request retried with the answer, not left waiting"
    );
}
