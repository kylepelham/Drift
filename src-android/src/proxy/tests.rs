use std::net::SocketAddr;

use axum::extract::Request;
use axum::http::HeaderMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use serde_json::json;

use super::*;
use crate::pc::Endpoint;
use crate::tls::Trust;
use crate::tls::tests::Authority;

const SECRET: &str = "launch-secret";

/// The proxy on a free port, with no PC linked, and a client that does not follow redirects.
async fn proxy() -> (Proxy, SocketAddr, reqwest::Client) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let path = crate::test_dir().join(format!("drift-proxy-{}.json", pc::hex(&rand_bytes())));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let proxy = Shared::new(
        Computers::load(path),
        Arc::new(Native {}),
        SECRET.into(),
        address.port(),
    );
    tokio::spawn(serve(listener, proxy.clone()));
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    (proxy, address, client)
}

fn rand_bytes() -> [u8; 6] {
    let mut bytes = [0u8; 6];
    getrandom::fill(&mut bytes).unwrap();
    bytes
}

fn signed_in() -> String {
    format!("{COOKIE}={SECRET}")
}

#[tokio::test]
async fn only_the_webview_holding_the_cookie_of_this_launch_gets_through() {
    let (_, address, client) = proxy().await;
    let url = |path: &str| format!("http://{address}{path}");

    let anyone = client.get(url("/app/computers")).send().await.unwrap();
    assert_eq!(anyone.status(), StatusCode::UNAUTHORIZED, "another app on the phone");

    let rebound = client
        .get(url("/app/computers"))
        .header("host", "evil.example")
        .header("cookie", signed_in())
        .send()
        .await
        .unwrap();
    assert_eq!(
        rebound.status(),
        StatusCode::FORBIDDEN,
        "a page that resolved its own name here"
    );

    let guessed = client.get(url("/app/enter?secret=nope")).send().await.unwrap();
    assert_eq!(guessed.status(), StatusCode::UNAUTHORIZED);

    let entered = client
        .get(url(&format!("/app/enter?secret={SECRET}")))
        .send()
        .await
        .unwrap();
    let cookie = entered.headers()["set-cookie"].to_str().unwrap();
    assert!(
        cookie.starts_with(&signed_in()) && cookie.contains("HttpOnly"),
        "{cookie}"
    );
    assert_eq!(entered.headers()["location"], "/app");

    let home = client
        .get(url("/app"))
        .header("cookie", signed_in())
        .send()
        .await
        .unwrap();
    assert_eq!(home.headers()["location"], "/app/computers", "no PC linked yet");
}

/// A stand-in gateway answering `/engine/echo` with what it was sent, and `/engine/events` as an upgrade.
fn gateway() -> Router {
    Router::new()
        .route(
            "/auth/options",
            get(|| async { axum::Json(json!({ "api": 1, "version": "test", "computer": "PC" })) }),
        )
        .route("/auth/me", get(|| async { "{}" }))
        .route(
            "/engine/echo",
            axum::routing::post(|headers: HeaderMap, body: String| async move {
                let header = |name: &str| {
                    headers
                        .get(name)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("-")
                        .to_string()
                };
                format!(
                    "{} {} {} {body}",
                    header("authorization"),
                    header("cookie"),
                    header("origin")
                )
            }),
        )
        .route("/engine/events", axum::routing::get(echo_upgrade))
}

/// Switches protocols and echoes whatever bytes arrive, as the event socket's server end would carry them.
async fn echo_upgrade(mut request: Request) -> Response {
    let upgrade = hyper::upgrade::on(&mut request);
    tokio::spawn(async move {
        let mut stream = hyper_util::rt::TokioIo::new(upgrade.await.unwrap());
        let mut buffer = [0u8; 64];
        let read = stream.read(&mut buffer).await.unwrap();
        stream.write_all(&buffer[..read]).await.unwrap();
    });

    Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .body(axum::body::Body::empty())
        .unwrap()
}

/// Routes the proxy to a stand-in PC that trusts only its own authority.
async fn routed(proxy: &Proxy) {
    let authority = Authority::new();
    let address = authority.serve(gateway()).await;
    let pc = Pc::new(Endpoint::Lan(address), Trust::Authority(authority.fingerprint()));
    *proxy.route.lock().unwrap() = Some(Route {
        id: pc::hex(&authority.fingerprint()),
        pc,
        token: "phone-session".into(),
    });
}

#[tokio::test]
async fn requests_reach_the_pc_with_this_phones_session_and_without_the_proxy_cookie() {
    let (proxy, address, client) = proxy().await;
    routed(&proxy).await;

    let echoed = client
        .post(format!("http://{address}/engine/echo"))
        .header("cookie", signed_in())
        .header("origin", format!("http://{address}"))
        .body("prompt")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert_eq!(echoed, "Bearer phone-session - - prompt");
}

#[tokio::test]
async fn the_event_socket_passes_through_as_an_upgrade() {
    let (proxy, address, _) = proxy().await;
    routed(&proxy).await;

    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let handshake = format!(
        "GET /engine/events HTTP/1.1\r\nHost: {address}\r\nCookie: {}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        signed_in()
    );
    stream.write_all(handshake.as_bytes()).await.unwrap();

    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).await.unwrap();
        head.push(byte[0]);
    }
    assert!(
        String::from_utf8_lossy(&head).starts_with("HTTP/1.1 101"),
        "{}",
        String::from_utf8_lossy(&head)
    );

    stream.write_all(b"ping").await.unwrap();
    let mut echoed = [0u8; 4];
    stream.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"ping");
}

/// A linked PC answering on `addresses` at `port`, made the active one.
fn link(proxy: &Proxy, authority: &Authority, addresses: &[&str], port: u16) -> String {
    let id = pc::hex(&authority.fingerprint());
    let computer = Computer {
        id: id.clone(),
        name: "Kyle's desktop".into(),
        addresses: addresses.iter().map(|address| address.parse().unwrap()).collect(),
        port,
        mac: None,
        tunnel: None,
        token: "phone-session".into(),
    };
    let mut computers = proxy.computers.lock().unwrap();
    computers.upsert(computer).unwrap();
    computers.set_active(Some(&id)).unwrap();

    id
}

#[tokio::test]
async fn an_open_page_gets_its_pc_back_after_the_connection_dropped() {
    let (proxy, address, client) = proxy().await;
    let authority = Authority::new();
    let pc_address = authority.serve(gateway()).await;
    link(&proxy, &authority, &["127.0.0.1"], pc_address.port());

    let echoed = client
        .post(format!("http://{address}/engine/echo"))
        .header("cookie", signed_in())
        .body("prompt")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert_eq!(echoed, "Bearer phone-session - - prompt");
}

#[tokio::test]
async fn with_no_pc_chosen_the_ui_is_told_so() {
    let (_, address, client) = proxy().await;

    let refused = client
        .get(format!("http://{address}/engine/session"))
        .header("cookie", signed_in())
        .send()
        .await
        .unwrap();

    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn a_pc_is_found_at_whichever_of_its_addresses_answers_and_that_one_is_tried_first_next_time() {
    let (proxy, _, _) = proxy().await;
    let authority = Authority::new();
    let pc_address = authority.serve(gateway()).await;
    // 127.0.0.2 is loopback too on most systems but nothing listens on it at this port.
    let id = link(&proxy, &authority, &["127.0.0.2", "127.0.0.1"], pc_address.port());

    let computer = proxy.computers.lock().unwrap().get(&id).cloned().unwrap();
    proxy.connect(&computer).await.unwrap();

    let route = proxy.route.lock().unwrap().clone().unwrap();
    assert_eq!(route.pc.endpoint, Endpoint::Lan(pc_address));
    let known = proxy.computers.lock().unwrap().get(&id).cloned().unwrap();
    assert_eq!(known.addresses.first(), Some(&pc_address.ip()));
}

#[tokio::test]
async fn a_pc_without_a_tunnel_that_does_not_answer_says_how_to_reach_it_from_anywhere() {
    let (proxy, _, _) = proxy().await;
    let authority = Authority::new();
    let id = link(&proxy, &authority, &["127.0.0.1"], 9);

    let computer = proxy.computers.lock().unwrap().get(&id).cloned().unwrap();
    let error = proxy.connect(&computer).await.unwrap_err().to_string();
    assert!(error.contains("Reach from anywhere"), "{error}");
}
