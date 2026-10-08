use super::gateway::{host_guard, revoke_on_auth_change, static_path, valid_host_origin};
use super::transport::{accept_loop, discovery_descriptor, plain_redirect};
use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Request};
use axum::http::{HeaderValue, header};
use axum::middleware;
use axum::routing::{any, get};
use futures_util::StreamExt;
use reqwest::redirect::Policy;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tower::ServiceExt;

use super::*;

/// The engine's routes read their own path parameters; the gateway mount must add none of its own.
#[tokio::test]
async fn the_engine_mounted_under_the_gateway_sees_only_its_own_path_parameters() {
    let engine = || {
        Router::new().route(
            "/sessions/{id}/messages",
            get(|Path(id): Path<String>| async move { id }),
        )
    };
    let ask = |router: Router| async move {
        let request = Request::builder()
            .uri("/engine/sessions/ses_1/messages?limit=5")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();

        (status, text)
    };

    let nested = Router::new().nest_service(
        "/engine",
        any(move |request: Request| async move { engine().oneshot(request).await.unwrap() }),
    );
    assert_eq!(ask(nested).await, (StatusCode::OK, "ses_1".to_string()));

    let captured = Router::new().route(
        "/engine/{*path}",
        any(move |request: Request| async move {
            let (mut parts, body) = request.into_parts();
            parts.uri = parts.uri.path().trim_start_matches("/engine").parse().unwrap();
            engine().oneshot(Request::from_parts(parts, body)).await.unwrap()
        }),
    );
    assert_eq!(
        ask(captured).await.0,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a capture route leaks its parameter, which is what broke history on the phone"
    );
}

#[test]
fn disabled_status_has_no_listening_urls_or_code() {
    let status = status_for(
        &RemoteConfig {
            enabled: false,
            error: None,
        },
        false,
    );

    assert!(!status.enabled);
    assert!(!status.listening);
    assert!(status.urls.is_empty());
    assert!(status.address_qr.is_none());
}

#[test]
fn address_qr_is_an_svg_of_the_plain_address() {
    let svg = address_qr("https://192.168.1.20:41718/companion").unwrap();

    assert!(svg.contains("<svg"));
    assert!(!svg.contains("token"));
}

#[test]
fn remote_access_management_is_not_remotely_invokable() {
    for command in [
        "remote_access_enable",
        "remote_access_link",
        "remote_access_revoke",
        "remote_access_set_password",
        "remote_access_status",
    ] {
        assert!(!rpc_allowed(command), "{command} must stay desktop-only");
    }
}

#[tokio::test]
async fn auth_changes_terminate_existing_streams() {
    let (revision, auth) = watch::channel(0u64);
    let source = futures_util::stream::unfold(0, |index| async move {
        if index == 0 {
            Some(("first", 1))
        } else {
            futures_util::future::pending().await
        }
    });
    let mut stream = Box::pin(revoke_on_auth_change(source, auth));

    assert_eq!(stream.next().await, Some("first"));
    revision.send(1).unwrap();
    assert_eq!(stream.next().await, None);
}

#[test]
fn rpc_has_a_finite_allowlist() {
    for command in [
        "store_workspaces",
        "store_expired_archived",
        "voice_transcribe",
        "ui_state_snapshot",
        "ui_state_update",
        "shell_timeout_snapshot",
        "shell_timeout_update",
        "pick_folder",
        "open_file",
        "open_file_in_editor",
        "read_file_preview",
    ] {
        assert!(rpc_allowed(command), "{command}");
    }

    for command in [
        "voice_dictation_set_enabled",
        "remote_access_enable",
        "ui_state_initialize",
        "shell_timeout_initialize",
        "plugin:shell|execute",
    ] {
        assert!(!rpc_allowed(command), "{command}");
    }
}

#[tokio::test]
async fn file_preview_rpc_requires_typed_arguments_and_safe_limits() {
    let valid = json!({ "path": "file", "directory": "workspace", "maxBytes": 0 });
    assert_eq!(arg::<String>(&valid, "path").unwrap(), "file");
    assert_eq!(arg::<String>(&valid, "directory").unwrap(), "workspace");
    assert_eq!(arg::<u64>(&valid, "maxBytes").unwrap(), 0);

    for key in ["path", "directory", "maxBytes"] {
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove(key);
        let error = if key == "maxBytes" {
            arg::<u64>(&missing, key).unwrap_err()
        } else {
            arg::<String>(&missing, key).unwrap_err()
        };
        assert_eq!(error.to_string(), format!("missing argument: {key}"));
    }

    for invalid in [Value::Null, json!(false), json!(1), json!([]), json!({})] {
        for key in ["path", "directory"] {
            let mut args = valid.clone();
            args[key] = invalid.clone();
            let error = arg::<String>(&args, key).unwrap_err();
            assert!(error.to_string().contains("invalid argument"));
        }
    }

    for invalid in [
        Value::Null,
        json!(false),
        json!(-1),
        json!(1.5),
        json!("10"),
        json!([]),
        json!({}),
        json!(18446744073709551616.0),
    ] {
        let mut args = valid.clone();
        args["maxBytes"] = invalid;
        let error = arg::<u64>(&args, "maxBytes").unwrap_err();
        assert!(error.to_string().contains("invalid argument"));
    }
    assert!(arg::<u64>(&json!({ "max_bytes": 1 }), "maxBytes").is_err());

    for limit in [40 * 1024 * 1024 + 1, u64::MAX] {
        let args = json!({ "path": "file", "directory": "workspace", "maxBytes": limit });
        let error = file_preview::read_file_preview(
            arg(&args, "path").unwrap(),
            arg(&args, "directory").unwrap(),
            arg(&args, "maxBytes").unwrap(),
        )
        .await
        .unwrap_err();
        assert!(error.contains("too large"));
    }
}

#[test]
fn static_paths_reject_traversal_and_choose_mime() {
    assert_eq!(static_path("/assets/app.js"), Some("assets/app.js"));
    assert_eq!(static_path("/../secret"), None);
    assert_eq!(static_path("/%2e%2e/secret"), None);
    assert_eq!(mime_guess::from_path("font.woff2").first_raw(), Some("font/woff2"));
}

#[test]
fn discovery_is_branded_without_disclosing_credentials() {
    let descriptor = discovery_descriptor(Ipv4Addr::new(192, 168, 1, 20), "AB:CD");
    let value = serde_json::to_value(descriptor).unwrap();

    assert_eq!(value["kind"], "drift-companion");
    assert_eq!(value["brand"], "Drift");
    assert_eq!(value["version"], 2);
    assert_eq!(value["url"], "https://192.168.1.20:41718/companion");
    assert_eq!(value["certificateSha256"], "AB:CD");
    assert!(!value.to_string().contains("token"));
}

#[tokio::test]
async fn the_gateway_listener_serves_https_and_redirects_plain_http() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).unwrap();
    let directory = std::env::temp_dir().join(format!("drift-gateway-{}", u64::from_ne_bytes(bytes)));
    let tls = Arc::new(Tls::load_or_create(&directory).unwrap());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let router = Router::new()
        .route(
            "/peer",
            get(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move { peer.ip().to_string() }),
        )
        .layer(middleware::from_fn(host_guard));
    let (shutdown, receiver) = watch::channel(false);
    let server = tokio::spawn(accept_loop(listener, router, tls.clone(), receiver));

    let client = trusted_client(&tls, true);
    let secure = client
        .get(format!("https://127.0.0.1:{port}/peer"))
        .send()
        .await
        .unwrap();
    assert_eq!(secure.version(), reqwest::Version::HTTP_2);
    assert_eq!(
        secure.text().await.unwrap(),
        "127.0.0.1",
        "HTTP/2 requests pass the host guard"
    );

    let forged = client
        .get(format!("https://127.0.0.1:{port}/peer"))
        .header(header::ORIGIN, "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), reqwest::StatusCode::FORBIDDEN);

    let http1 = trusted_client(&tls, false);
    let legacy = http1
        .get(format!("https://127.0.0.1:{port}/peer"))
        .send()
        .await
        .unwrap();
    assert_eq!(legacy.version(), reqwest::Version::HTTP_11);
    assert_eq!(
        legacy.status(),
        reqwest::StatusCode::OK,
        "HTTP/1.1 requests pass with a Host header"
    );

    let plain = client
        .get(format!("http://127.0.0.1:{port}/companion?a=1"))
        .send()
        .await
        .unwrap();
    assert_eq!(plain.status(), reqwest::StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        plain.headers()["location"],
        format!("https://127.0.0.1:{port}/companion?a=1")
    );
    assert_untrusted_client_rejected(port).await;

    shutdown.send(true).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
    assert!(
        client
            .get(format!("https://127.0.0.1:{port}/peer"))
            .send()
            .await
            .is_err()
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn plain_http_redirects_to_the_same_path_over_https() {
    let head = "GET /companion?x=1 HTTP/1.1\r\nHost: 192.168.1.20:41718\r\n\r\n";
    let response = plain_redirect(head, "10.0.0.2:41718");
    assert!(response.starts_with("HTTP/1.1 308"));
    assert!(response.contains("Location: https://192.168.1.20:41718/companion?x=1\r\n"));

    let injected = "GET /\r\nSet-Cookie:x HTTP/1.1\r\nHost: evil\r\n x\r\n\r\n";
    assert!(plain_redirect(injected, "10.0.0.2:41718").contains("Location: https://evil/\r\n"));
    let hostless = plain_redirect("GET http://elsewhere/ HTTP/1.1\r\n\r\n", "10.0.0.2:41718");
    assert!(hostless.contains("Location: https://10.0.0.2:41718/\r\n"));
    assert!(plain_redirect("garbage", "10.0.0.2:41718").contains("https://10.0.0.2:41718/"));
}

fn trusted_client(tls: &Tls, http2: bool) -> reqwest::Client {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(rustls::pki_types::CertificateDer::from(tls.ca_der().to_vec()))
        .unwrap();
    let mut config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    if http2 {
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    }

    let builder = reqwest::Client::builder().use_preconfigured_tls(config);
    let builder = if http2 {
        builder.redirect(Policy::none())
    } else {
        builder.http1_only()
    };
    builder.build().unwrap()
}

async fn assert_untrusted_client_rejected(port: u16) {
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    let client = reqwest::Client::builder()
        .use_preconfigured_tls(config)
        .build()
        .unwrap();

    assert!(
        client
            .get(format!("https://127.0.0.1:{port}/peer"))
            .send()
            .await
            .is_err()
    );
}

#[test]
fn origins_must_match_the_https_host() {
    let mut headers = HeaderMap::new();
    assert!(
        !valid_host_origin(&headers, None),
        "a request without any host is rejected"
    );
    headers.insert(header::HOST, HeaderValue::from_static("192.168.1.20:41718"));
    assert!(valid_host_origin(&headers, None));

    headers.insert(header::ORIGIN, HeaderValue::from_static("https://192.168.1.20:41718"));
    assert!(valid_host_origin(&headers, None));
    headers.insert(header::ORIGIN, HeaderValue::from_static("http://192.168.1.20:41718"));
    assert!(!valid_host_origin(&headers, None));
    headers.insert(header::ORIGIN, HeaderValue::from_static("https://evil.example"));
    assert!(!valid_host_origin(&headers, None));

    let mut h2 = HeaderMap::new();
    h2.insert(header::ORIGIN, HeaderValue::from_static("https://192.168.1.20:41718"));
    assert!(
        valid_host_origin(&h2, Some("192.168.1.20:41718")),
        "HTTP/2 sends the host as :authority"
    );
    assert!(!valid_host_origin(&h2, Some("evil.example")));
}
