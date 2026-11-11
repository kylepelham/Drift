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

/// A host whose bundled UI is two pages, and which names itself as the phone did.
struct Bundle;

impl crate::Host for Bundle {
    fn asset(&self, path: &str) -> Option<std::borrow::Cow<'static, [u8]>> {
        match path {
            "index.html" => Some(b"<html><head></head><body>drift</body></html>".as_slice().into()),
            "app.html" => Some(b"<html><head></head><body>computers</body></html>".as_slice().into()),
            _ => None,
        }
    }

    fn device_name(&self) -> String {
        "Android".into()
    }
}

/// The proxy on a free port, with no PC linked, and a client that does not follow redirects.
async fn proxy() -> (Proxy, SocketAddr, reqwest::Client) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let path = crate::test_dir().join(format!("drift-proxy-{}.json", pc::hex(&rand_bytes())));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let proxy = Shared::new(
        Computers::load(path.clone()),
        Arc::new(Bundle),
        SECRET.into(),
        address.port(),
        path.with_extension("storage.json"),
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

#[tokio::test]
async fn the_pages_saved_values_outlive_the_launch_and_come_back_before_its_scripts() {
    let (proxy, address, client) = proxy().await;
    let url = |path: &str| format!("http://{address}{path}");
    let values = json!({ "drift.workspace.collapsed": "[\"w1\"]", "drift.diff.wrap": "true" });
    let put = |sequence: u64, body: &serde_json::Value| {
        client
            .put(url(&format!("/app/api/storage?page=p1&sequence={sequence}")))
            .header("cookie", signed_in())
            .json(body)
            .send()
    };

    assert_eq!(put(2, &values).await.unwrap().status(), StatusCode::NO_CONTENT);
    let stale = put(1, &json!({ "drift.diff.wrap": "false" })).await.unwrap();
    assert_eq!(
        stale.status(),
        StatusCode::NO_CONTENT,
        "a slow earlier snapshot is answered"
    );
    let refused = client
        .put(url("/app/api/storage?page=p1&sequence=3"))
        .json(&values)
        .send()
        .await
        .unwrap();
    assert_eq!(
        refused.status(),
        StatusCode::UNAUTHORIZED,
        "another app cannot plant values"
    );

    let script = next_launch_restores(&proxy).await;
    assert!(script.contains(r#""drift.workspace.collapsed":"[\"w1\"]""#), "{script}");
    assert!(
        script.contains(r#""drift.diff.wrap":"true""#),
        "a slow earlier snapshot is not kept: {script}"
    );
    assert!(script.contains("localStorage.setItem"), "{script}");

    let reloaded = client
        .put(url("/app/api/storage?page=p2&sequence=1"))
        .header("cookie", signed_in())
        .json(&json!({ "drift.diff.wrap": "reloaded" }))
        .send()
        .await
        .unwrap();
    assert_eq!(reloaded.status(), StatusCode::NO_CONTENT);
    put(9, &json!({ "drift.diff.wrap": "late" })).await.unwrap();
    let script = next_launch_restores(&proxy).await;
    assert!(
        script.contains(r#""drift.diff.wrap":"reloaded""#),
        "a page load that was replaced cannot overwrite the new one: {script}"
    );

    let changes = |sequence: u64, base: u64| {
        client
            .put(url(&format!(
                "/app/api/storage?page=p2&sequence={sequence}&base={base}"
            )))
            .header("cookie", signed_in())
            .json(&json!({ "set": { "drift.theme": "dim" }, "removed": ["drift.diff.wrap"] }))
            .send()
    };
    assert_eq!(
        changes(2, 7).await.unwrap().status(),
        StatusCode::CONFLICT,
        "changes from a snapshot not kept"
    );
    assert_eq!(changes(2, 1).await.unwrap().status(), StatusCode::NO_CONTENT);
    let script = next_launch_restores(&proxy).await;
    assert!(
        script.contains(r#""drift.theme":"dim""#) && !script.contains("drift.diff.wrap"),
        "changes apply to what was kept: {script}"
    );

    let page = storage::with_restore("<html><head><script>reads()</script></head></html>");
    assert!(
        page.starts_with(r#"<html><head><script src="/app/api/storage.js"></script><script>reads()"#),
        "{page}"
    );
}

/// The script the next launch, a new proxy on another port reading the same file, restores the page with.
async fn next_launch_restores(proxy: &Proxy) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let proxy = Shared::new(
        Computers::load(crate::test_dir().join("unused.json")),
        Arc::new(Bundle),
        SECRET.into(),
        address.port(),
        proxy.storage.clone(),
    );
    tokio::spawn(serve(listener, proxy));

    reqwest::Client::new()
        .get(format!("http://{address}/app/api/storage.js"))
        .header("cookie", signed_in())
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

/// A desktop as the proxy's host: its own Drift to go back to.
#[derive(Default)]
struct Desk {
    shown: std::sync::atomic::AtomicBool,
    id: Option<String>,
}

impl crate::Host for Desk {
    fn asset(&self, path: &str) -> Option<std::borrow::Cow<'static, [u8]>> {
        Bundle.asset(path)
    }

    fn device_name(&self) -> String {
        "Desk".into()
    }

    fn this_computer(&self) -> Option<String> {
        Some("Desk".into())
    }

    fn this_computer_id(&self) -> Option<String> {
        self.id.clone()
    }

    fn show_this_computer(&self) -> Result<(), String> {
        self.shown.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

async fn desk_proxy(desk: Arc<Desk>) -> (Proxy, String) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let path = crate::test_dir().join(format!("drift-desk-{}.json", pc::hex(&rand_bytes())));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let proxy = Shared::new(
        Computers::load(path.clone()),
        desk,
        SECRET.into(),
        address.port(),
        path.with_extension("storage.json"),
    );
    tokio::spawn(serve(listener, proxy.clone()));

    (proxy, format!("http://{address}"))
}

/// A PC's side of a device asking to connect: it commits to `secret`, takes the device's, and answers `wait` with
/// `answer` once asked twice, so the device's waiting is seen to carry on.
fn asked(secret: drift_pairing::Secret, answer: (StatusCode, serde_json::Value)) -> (Router, Arc<Mutex<Vec<String>>>) {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let waits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let log = |seen: &Arc<Mutex<Vec<String>>>, entry: String| seen.lock().unwrap().push(entry);

    let router = gateway()
        .route(
            "/auth/request",
            axum::routing::post({
                let seen = seen.clone();
                move |axum::Json(body): axum::Json<serde_json::Value>| async move {
                    log(&seen, format!("request {}", body["name"]));
                    let commitment = pc::hex(&drift_pairing::commitment(&secret));
                    axum::Json(json!({ "id": "r1", "commitment": commitment }))
                }
            }),
        )
        .route(
            "/auth/request/reveal",
            axum::routing::post({
                let seen = seen.clone();
                move |axum::Json(body): axum::Json<serde_json::Value>| async move {
                    log(&seen, format!("reveal {}", body["secret"].as_str().unwrap()));
                    axum::Json(json!({ "secret": pc::hex(&secret) }))
                }
            }),
        )
        .route(
            "/auth/request/wait",
            axum::routing::post(move || async move {
                if waits.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    return (StatusCode::OK, axum::Json(json!({ "waiting": true })));
                }
                (answer.0, axum::Json(answer.1))
            }),
        )
        .route(
            "/auth/request/cancel",
            axum::routing::post({
                let seen = seen.clone();
                move || async move { log(&seen, "cancel".into()) }
            }),
        );

    (router, seen)
}

#[tokio::test]
async fn a_desktop_asks_a_pc_to_connect_shows_the_code_its_user_sees_and_goes_back_to_its_own_drift() {
    let authority = Authority::new();
    let pc_secret = [5u8; 32];
    let (gateway, seen) = asked(pc_secret, (StatusCode::OK, json!({ "token": "desk-session" })));
    let address = authority.serve(gateway).await;
    let desk = Arc::new(Desk::default());
    let (proxy, base) = desk_proxy(desk.clone()).await;
    let client = reqwest::Client::new();
    let call = |method: reqwest::Method, path: &str| {
        client
            .request(method, format!("{base}/app/api{path}"))
            .header("cookie", signed_in())
    };

    let waiting: serde_json::Value = call(reqwest::Method::POST, "/ask")
        .json(&json!({ "address": format!(" {address} ") }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(waiting["name"], "PC", "{waiting}");
    let device_secret = seen.lock().unwrap()[1]
        .strip_prefix("reveal ")
        .and_then(drift_pairing::parse_fingerprint)
        .unwrap();
    assert_eq!(
        seen.lock().unwrap()[0],
        r#"request "Desk""#,
        "the PC lists the desktop by its name"
    );
    assert_eq!(
        waiting["code"],
        drift_pairing::matching_code(&authority.fingerprint(), &device_secret, &pc_secret),
        "the code its user sees on the PC"
    );

    let linked: serde_json::Value = call(reqwest::Method::POST, "/answer")
        .json(&json!({ "id": waiting["id"] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(linked["linked"], "PC", "{linked}");
    assert_eq!(
        proxy.route.lock().unwrap().as_ref().map(|route| route.token.clone()),
        Some("desk-session".into()),
        "requests go to the PC just linked"
    );
    let saved = proxy.computers.lock().unwrap().list()[0].clone();
    assert_eq!(
        saved.id,
        pc::hex(&authority.fingerprint()),
        "the authority met first is pinned"
    );
    assert_eq!(saved.addresses, vec![address.ip()]);

    let listed: serde_json::Value = call(reqwest::Method::GET, "/computers")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["local"], "Desk");
    assert_eq!(listed["computers"][0]["active"], true);

    let left = call(reqwest::Method::POST, "/local").send().await.unwrap();
    assert!(left.status().is_success());
    assert!(
        desk.shown.load(std::sync::atomic::Ordering::SeqCst),
        "the window shows this PC again"
    );
    assert!(
        proxy.computers.lock().unwrap().active().is_none(),
        "this PC is what the window shows now"
    );
}

#[tokio::test]
async fn the_sidebar_tabs_are_a_saved_choice_and_only_computers_that_answer_are_online() {
    let authority = Authority::new();
    let (gateway, _) = asked([5u8; 32], (StatusCode::OK, json!({ "token": "desk-session" })));
    let address = authority.serve(gateway).await;
    let (proxy, base) = desk_proxy(Arc::default()).await;
    let waiting = proxy.ask(&address.to_string()).await.unwrap();
    let (answering, _) = proxy.answer(&waiting.id).await.unwrap();
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap();
    let mut gone = proxy.computers.lock().unwrap().get(&answering).unwrap().clone();
    gone.id = "ab".repeat(32);
    gone.addresses = vec![closed.ip()];
    gone.port = closed.port();
    proxy.computers.lock().unwrap().upsert(gone.clone()).unwrap();
    // A route left from before, as after going back to this PC, says nothing of whether the computer answers now.
    proxy.route.lock().unwrap().as_mut().unwrap().id = gone.id;

    let client = reqwest::Client::new();
    let call = |method: reqwest::Method, path: &str| {
        client
            .request(method, format!("{base}/app/api{path}"))
            .header("cookie", signed_in())
    };
    let listed = || async {
        let value: serde_json::Value = call(reqwest::Method::GET, "/computers")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        value
    };
    assert_eq!(listed().await["tabs"], false, "off until turned on");
    let turned = call(reqwest::Method::POST, "/tabs")
        .json(&json!({ "on": true }))
        .send()
        .await
        .unwrap();
    assert!(turned.status().is_success());
    assert_eq!(listed().await["tabs"], true);
    assert!(proxy.computers.lock().unwrap().tabs());

    let online: serde_json::Value = call(reqwest::Method::GET, "/online")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        online["online"],
        json!([answering]),
        "the computer whose port is closed is not online"
    );
}

#[tokio::test]
async fn a_declined_or_cancelled_request_links_nothing() {
    let authority = Authority::new();
    let (declining, _) = asked([5u8; 32], (StatusCode::FORBIDDEN, json!({ "error": "declined" })));
    let declining = authority.serve(declining).await;
    let (proxy, _) = desk_proxy(Arc::default()).await;

    let waiting = proxy.ask(&declining.to_string()).await.unwrap();
    let declined = proxy.answer(&waiting.id).await.unwrap_err();
    assert_eq!(declined, "PC declined the connection");
    assert!(proxy.computers.lock().unwrap().list().is_empty());
    assert!(proxy.answer(&waiting.id).await.is_err(), "an answered request is over");

    let (patient, seen) = asked([5u8; 32], (StatusCode::OK, json!({ "waiting": true })));
    let patient = Authority::new().serve(patient).await;
    let waiting = proxy.ask(&patient.to_string()).await.unwrap();
    proxy.cancel(&waiting.id).await;
    assert_eq!(
        seen.lock().unwrap().last().unwrap(),
        "cancel",
        "the PC stops asking its user"
    );
    assert!(proxy.answer(&waiting.id).await.is_err());
}

#[tokio::test]
async fn a_pc_that_changes_its_secret_after_seeing_the_devices_is_refused() {
    let authority = Authority::new();
    let gateway = gateway()
        .route(
            "/auth/request",
            axum::routing::post(|| async {
                axum::Json(json!({ "id": "r1", "commitment": pc::hex(&drift_pairing::commitment(&[1u8; 32])) }))
            }),
        )
        .route(
            "/auth/request/reveal",
            axum::routing::post(|| async { axum::Json(json!({ "secret": pc::hex(&[2u8; 32]) })) }),
        );
    let address = authority.serve(gateway).await;
    let (proxy, _) = desk_proxy(Arc::default()).await;

    let refused = proxy.ask(&address.to_string()).await.unwrap_err();
    assert!(refused.contains("changed its answer"), "{refused}");
}

#[tokio::test]
async fn a_desktop_never_asks_itself() {
    let authority = Authority::new();
    let (gateway, seen) = asked([5u8; 32], (StatusCode::OK, json!({ "token": "t" })));
    let address = authority.serve(gateway).await;
    let desk = Arc::new(Desk {
        id: Some(pc::hex(&authority.fingerprint())),
        ..Desk::default()
    });
    let (proxy, _) = desk_proxy(desk).await;

    assert_eq!(
        proxy.ask(&address.to_string()).await.unwrap_err(),
        "that is this computer"
    );
    assert!(seen.lock().unwrap().is_empty(), "its user is never asked");
}

#[tokio::test]
async fn a_phone_has_no_drift_of_its_own_to_go_back_to() {
    let (_, address, client) = proxy().await;
    let call = |method: reqwest::Method, path: &str| {
        client
            .request(method, format!("http://{address}/app/api{path}"))
            .header("cookie", signed_in())
    };

    let listed: serde_json::Value = call(reqwest::Method::GET, "/computers")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(listed["local"].is_null());
    let refused = call(reqwest::Method::POST, "/local").send().await.unwrap();
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
}
