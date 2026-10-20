use super::gateway::{host_guard, names_host, revoke_on_auth_change, static_path, valid_host_origin};
use super::transport::{accept_loop, plain_redirect};
use super::tunnel::{certificate_key, new_route, read_proxy_header};
use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Request};
use axum::http::{HeaderValue, header};
use axum::middleware;
use axum::routing::{any, get};
use futures_util::StreamExt;
use opentunnel::ProxyProtocol;
use opentunnel::protocol::proxy;
use reqwest::redirect::Policy;
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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

fn config(enabled: bool, browser: bool) -> RemoteConfig {
    RemoteConfig {
        enabled,
        browser,
        tunnel: false,
        error: None,
    }
}

#[test]
fn a_browser_address_is_offered_only_while_browser_access_is_on() {
    for (enabled, listening, browser) in [(false, false, true), (true, false, true), (true, true, false)] {
        let status = status_for(&config(enabled, browser), listening);
        assert!(status.lan_url.is_none(), "{enabled} {listening} {browser}");
    }

    let serving = status_for(&config(true, true), true);
    if let Some(url) = serving.lan_url {
        assert!(url.starts_with("https://") && url.ends_with(":41718"), "{url}");
    }
    assert_eq!(serving.tunnel, TunnelStatus::OFF);
}

#[test]
fn remote_access_management_is_not_remotely_invokable() {
    for command in [
        "remote_access_enable",
        "remote_access_pairing",
        "remote_access_set_browser",
        "remote_access_set_tunnel",
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

#[tokio::test]
async fn the_visitor_comes_from_the_tunnels_proxy_header_whatever_its_family() {
    let read = |bytes: Vec<u8>| async move { read_proxy_header(&mut bytes.as_slice()).await };
    let host = "k2v7qxmd4hzt6wnc.x3fp8rk2mq7a.opentunnel.xyz";

    let ipv4 = proxy::header(ProxyProtocol::V2, "203.0.113.9:51234", host);
    assert_eq!(read(ipv4).await.unwrap(), Some("203.0.113.9:51234".parse().unwrap()));
    let ipv6 = proxy::header(ProxyProtocol::V2, "[2001:db8::1]:443", host);
    assert_eq!(read(ipv6).await.unwrap(), Some("[2001:db8::1]:443".parse().unwrap()));
    let unknown = proxy::header(ProxyProtocol::V2, "not an address", host);
    assert_eq!(read(unknown).await.unwrap(), None, "LOCAL names no visitor");

    assert!(read(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n".to_vec()).await.is_err());
    assert!(
        read(proxy::header(ProxyProtocol::V1, "203.0.113.9:1", host))
            .await
            .is_err()
    );
    let mut oversized = proxy::header(ProxyProtocol::V2, "203.0.113.9:1", "");
    oversized[14..16].copy_from_slice(&4096u16.to_be_bytes());
    assert!(read(oversized).await.is_err());
}

#[tokio::test]
async fn a_tunneled_request_is_served_as_its_visitor_and_only_for_the_tunnels_host() {
    let host = "k2v7qxmd4hzt6wnc.x3fp8rk2mq7a.opentunnel.xyz";
    let router = Router::new()
        .route(
            "/peer",
            get(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move { peer.to_string() }),
        )
        .layer(middleware::from_fn(host_guard));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(super::tunnel::accept_loop(listener, router, host.into()));

    let ask = |named: &'static str| async move {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream
            .write_all(&proxy::header(ProxyProtocol::V2, "198.51.100.7:40000", named))
            .await
            .unwrap();
        let request = format!("GET /peer HTTP/1.1\r\nHost: {named}\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        answer
    };

    let served = ask(host).await;
    assert!(served.starts_with("HTTP/1.1 200"), "{served}");
    assert!(served.ends_with("198.51.100.7:40000"), "{served}");
    let elsewhere = ask("localhost").await;
    assert!(elsewhere.starts_with("HTTP/1.1 403"), "{elsewhere}");

    server.abort();
}

#[test]
fn the_tunnel_host_matches_with_or_without_the_https_port() {
    let expected = "k2v7.x3fp8rk2mq7a.opentunnel.xyz";
    let mut headers = HeaderMap::new();
    headers.insert(
        header::HOST,
        HeaderValue::from_static("K2V7.x3fp8rk2mq7a.opentunnel.xyz:443"),
    );
    assert!(names_host(&headers, None, expected));
    assert!(names_host(&HeaderMap::new(), Some(expected), expected));
    assert!(!names_host(
        &HeaderMap::new(),
        Some("other.x3fp8rk2mq7a.opentunnel.xyz"),
        expected
    ));
    assert!(!names_host(&HeaderMap::new(), None, expected));
}

#[test]
fn the_pinned_tunnel_key_is_the_one_in_its_certificate() {
    let key = rcgen::KeyPair::generate().unwrap();
    let certificate = rcgen::CertificateParams::new(vec!["x3fp8rk2mq7a.opentunnel.xyz".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let expected: [u8; 32] = Sha256::digest(rcgen::PublicKeyData::subject_public_key_info(&key)).into();

    assert_eq!(certificate_key(&certificate.pem()), Some(expected));
    assert_eq!(certificate_key("not a certificate"), None);
}

#[test]
fn routes_are_random_valid_names() {
    let first = new_route();
    assert_eq!(first.len(), 16);
    assert!(opentunnel::protocol::names::is_valid_route(&first), "{first}");
    assert_ne!(first, new_route());
}
