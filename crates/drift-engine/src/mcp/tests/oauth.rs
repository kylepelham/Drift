use super::transport::legacy_sse_routes;
use super::*;

/// The fake server's log: registration, authorize queries, and token bodies with their authorization headers.
type AuthLog = Arc<std::sync::Mutex<Vec<String>>>;

/// Follows an immediately approved sign-in page back to Drift's callback, as a browser would.
async fn browse(page: &str) {
    let landed = crate::llm::http::client().get(page).send().await.unwrap();
    assert!(landed.status().is_success(), "{}", landed.status());
}

#[tokio::test]
async fn a_server_that_needs_a_sign_in_says_so_then_connects_once_signed_in() {
    let (base, seen) = oauth_mcp_server(None).await;
    let engine = engine();
    let config = ServerConfig::Http {
        url: format!("{base}/mcp"),
        headers: Default::default(),
        oauth: None,
        timeout_seconds: None,
    };
    let row = saved(&engine, "secure", &config).await;
    assert!(seen.lock().unwrap().is_empty());

    let _ = engine.connect_mcp_in("secure", Some(&here())).await;
    let before = engine.mcp.status_of(row.clone());
    assert!(before.needs_sign_in && !before.signed_in, "{before:?}");

    let page = engine.sign_in_mcp("secure").await.unwrap();
    assert!(page.starts_with(&format!("{base}/authorize?")), "{page}");
    browse(&page).await;
    until("it connects signed in", || {
        engine.mcp.status_of(row.clone()).state == State::Connected
    })
    .await;
    let after = engine.mcp.status_of(row.clone());
    assert!(after.signed_in && !after.needs_sign_in);
    let output = tool(&engine, "secure_echo")
        .run(&context(&engine), json!({ "text": "authorized" }))
        .await
        .unwrap();
    assert_eq!(output.output, "authorized");
    assert_eq!(
        seen.lock().unwrap().first().map(String::as_str),
        Some("register"),
        "with no app configured Drift registers itself"
    );

    engine.sign_out_mcp("secure").await.unwrap();
    assert!(!engine.mcp.status_of(row).signed_in, "signing out forgets the tokens");
}

#[tokio::test]
async fn a_server_that_will_not_register_drift_signs_in_with_the_configured_app() {
    let (base, seen) = oauth_mcp_server(Some("drift-team-app")).await;
    let engine = engine();
    let app = OAuthClient {
        client_id: "drift-team-app".into(),
        client_secret: Some("team-secret".into()),
        scopes: vec!["read".into(), "write".into()],
    };
    let config = ServerConfig::Http {
        url: format!("{base}/mcp"),
        headers: Default::default(),
        oauth: Some(app),
        timeout_seconds: None,
    };
    let row = saved(&engine, "team", &config).await;

    let page = engine.sign_in_mcp("team").await.unwrap();
    assert!(
        page.contains("client_id=drift-team-app") && page.contains("scope=read+write"),
        "{page}"
    );
    browse(&page).await;
    until("it connects signed in", || {
        engine.mcp.status_of(row.clone()).state == State::Connected
    })
    .await;

    let seen = seen.lock().unwrap().clone();
    assert!(!seen.iter().any(|line| line == "register"), "{seen:?}");
    let token = seen.iter().find(|line| line.starts_with("token ")).unwrap();
    assert!(
        token.contains("client_secret=team-secret") || token.contains("Basic "),
        "the app's secret goes with the code: {token}"
    );
}

#[tokio::test]
async fn a_sign_in_the_user_refuses_says_why_and_still_asks_to_sign_in() {
    let (base, _) = oauth_mcp_server(None).await;
    let engine = engine();
    let config = ServerConfig::Http {
        url: format!("{base}/mcp"),
        headers: Default::default(),
        oauth: None,
        timeout_seconds: None,
    };
    let row = saved(&engine, "secure", &config).await;
    let page = engine.sign_in_mcp("secure").await.unwrap();
    let parsed = reqwest::Url::parse(&page).unwrap();
    let redirect = parsed
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .unwrap()
        .1
        .to_string();

    let refused = crate::llm::http::client()
        .get(format!(
            "{redirect}?error=access_denied&error_description=The+user+said+no"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400, "the browser hears it too");
    until("the refusal is reported", || {
        engine.mcp.status_of(row.clone()).error.is_some()
    })
    .await;
    let status = engine.mcp.status_of(row);
    assert_eq!((status.state, status.needs_sign_in), (State::Failed, true));
    assert_eq!(
        status.error.as_deref(),
        Some("Sign-in did not finish: access_denied: The user said no")
    );
}

#[tokio::test]
async fn a_server_on_the_older_sse_transport_signs_in_and_sends_its_token() {
    let (base, _) = oauth_mcp_server(None).await;
    let engine = engine();
    let config = ServerConfig::Sse {
        url: format!("{base}/sse"),
        headers: Default::default(),
        oauth: None,
        timeout_seconds: None,
    };
    let row = saved(&engine, "legacy", &config).await;

    let _ = engine.connect_mcp_in("legacy", Some(&here())).await;
    let before = engine.mcp.status_of(row.clone());
    assert!(
        before.needs_sign_in,
        "a 401 on the stream asks for a sign-in: {before:?}"
    );
    browse(&engine.sign_in_mcp("legacy").await.unwrap()).await;
    until("it connects signed in", || {
        engine.mcp.status_of(row.clone()).state == State::Connected
    })
    .await;
    let output = tool(&engine, "legacy_echo")
        .run(&context(&engine), json!({ "text": "over sse" }))
        .await
        .unwrap();
    assert_eq!(output.output, "over sse");
}

#[tokio::test]
async fn a_sign_in_follows_a_rename_but_never_a_new_url() {
    let engine = engine();
    let at = |url: &str| ServerConfig::Http {
        url: url.into(),
        headers: Default::default(),
        oauth: None,
        timeout_seconds: None,
    };
    engine.credentials.set_secret("mcp:old", "{}").unwrap();

    crate::mcp::move_sign_in(&engine.credentials, "old", "new");
    assert!(engine.credentials.secret("mcp:old").is_none() && engine.credentials.secret("mcp:new").is_some());
    crate::mcp::forget_if_moved(
        &engine.credentials,
        "new",
        &at("https://a.example/mcp"),
        &at("https://a.example/mcp"),
    );
    assert!(
        engine.credentials.secret("mcp:new").is_some(),
        "a save on the same URL keeps it"
    );

    crate::mcp::forget_if_moved(
        &engine.credentials,
        "new",
        &at("https://a.example/mcp"),
        &at("https://b.example/mcp"),
    );
    assert!(
        engine.credentials.secret("mcp:new").is_none(),
        "its tokens must not reach another host"
    );
}

/// Lets a request through only with the token the fake authorization server issues.
async fn bearer_only(request: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    use axum::response::IntoResponse;

    if request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        == Some("Bearer good-token")
    {
        return next.run(request).await;
    }

    (axum::http::StatusCode::UNAUTHORIZED, [("www-authenticate", "Bearer")]).into_response()
}

/// Serves authenticated MCP over HTTP and SSE; registers a client only when no app is configured.
async fn oauth_mcp_server(app: Option<&'static str>) -> (String, AuthLog) {
    use axum::extract::{Path, State as Shared};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
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
                    "grant_types_supported": ["authorization_code", "refresh_token"],
                    "token_endpoint_auth_methods_supported": ["none", "client_secret_post", "client_secret_basic"],
                });
                if app.is_none() {
                    metadata["registration_endpoint"] = json!(format!("{base}/register"));
                }

                axum::Json(metadata)
            }
        }
    };

    let mcp = post(
        move |Shared(base): Shared<String>, headers: HeaderMap, axum::Json(message): axum::Json<serde_json::Value>| async move {
            if headers.get("authorization").and_then(|value| value.to_str().ok()) != Some("Bearer good-token") {
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
        },
    );
    let routes = axum::Router::new()
        .route("/.well-known/oauth-protected-resource", get(resource("mcp")))
        .route("/.well-known/oauth-protected-resource/{path}", get(resource_at))
        .route("/.well-known/oauth-authorization-server", get(metadata))
        .merge(oauth_token_routes(&seen))
        .route(
            "/mcp",
            mcp.get(|| async { StatusCode::METHOD_NOT_ALLOWED })
                .delete(|| async { StatusCode::ACCEPTED }),
        )
        .with_state(base.clone())
        .merge(legacy_sse_routes().route_layer(axum::middleware::from_fn(bearer_only)));
    tokio::spawn(async move { axum::serve(listener, routes).await.unwrap() });

    (base, seen)
}

/// Registration, browser authorization and token endpoints for the fake OAuth server.
fn oauth_token_routes(seen: &AuthLog) -> axum::Router<String> {
    use axum::extract::Query;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::Redirect;
    use axum::routing::{get, post};

    let register = {
        let seen = seen.clone();
        post(move |axum::Json(body): axum::Json<serde_json::Value>| async move {
            seen.lock().unwrap().push("register".into());
            (
                StatusCode::CREATED,
                axum::Json(
                    json!({ "client_id": "drift-test-client", "redirect_uris": body["redirect_uris"], "token_endpoint_auth_method": "none" }),
                ),
            )
        })
    };

    let authorize = {
        let seen = seen.clone();
        get(
            move |axum::extract::RawQuery(raw): axum::extract::RawQuery,
                  Query(query): Query<std::collections::HashMap<String, String>>| async move {
                seen.lock()
                    .unwrap()
                    .push(format!("authorize {}", raw.unwrap_or_default()));
                Redirect::to(&format!(
                    "{}?code=granted&state={}",
                    query["redirect_uri"], query["state"]
                ))
            },
        )
    };

    let token = {
        let seen = seen.clone();
        post(move |headers: HeaderMap, body: String| async move {
            let authorization = headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            seen.lock().unwrap().push(format!("token {body} {authorization}"));
            axum::Json(
                json!({ "access_token": "good-token", "token_type": "Bearer", "expires_in": 3600, "refresh_token": "again" }),
            )
        })
    };

    axum::Router::new()
        .route("/register", register)
        .route("/authorize", authorize)
        .route("/token", token)
}
