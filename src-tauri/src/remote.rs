use crate::remote_auth::{self, Auth, PendingLink};
use crate::store::{RemoteDevice, Store};
use crate::{commands, config, editor, engine, file_preview, mcp, tool_routing, ui_state, voice};
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Extension, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use reqwest::redirect::Policy;
use rust_embed::RustEmbed;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::remote_tls::Tls;
use axum::extract::ConnectInfo;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::service::TowerToHyperService;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Component, Path};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::Manager;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, watch, Mutex as AsyncMutex};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;

pub(crate) const HTTP_PORT: u16 = 41718;
pub(crate) const DISCOVERY_PORT: u16 = 41717;
const DISCOVERY_PROBE: &[u8] = b"OPENCODE_COMPANION_DISCOVERY";
const MAX_CONCURRENT_PASSWORD_CHECKS: usize = 2;
const TLS_HANDSHAKE_RECORD: u8 = 0x16;
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const MAX_PROXY_BODY: usize = 32 * 1024 * 1024;
const MAX_RPC_BODY: usize = 10 * 1024 * 1024;

#[derive(RustEmbed)]
#[folder = "../dist"]
struct FrontendAssets;

#[derive(Clone)]
struct RemoteConfig {
    enabled: bool,
    error: Option<String>,
}

struct Running {
    shutdown: watch::Sender<bool>,
    http: JoinHandle<()>,
    discovery: JoinHandle<()>,
}

pub(crate) struct RemoteAccess {
    config: Mutex<RemoteConfig>,
    auth: Mutex<Auth>,
    tls: Arc<Tls>,
    password_checks: tokio::sync::Semaphore,
    running: AsyncMutex<Option<Running>>,
    transition: AsyncMutex<()>,
    auth_revision: watch::Sender<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteStatus {
    enabled: bool,
    listening: bool,
    port: u16,
    discovery_port: u16,
    listening_address: Option<String>,
    urls: Vec<String>,
    address_qr: Option<String>,
    devices: Vec<RemoteDevice>,
    pending_links: Vec<PendingLink>,
    password_username: Option<String>,
    certificate_fingerprint: String,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscoveryDescriptor {
    kind: &'static str,
    name: &'static str,
    brand: &'static str,
    protocol: &'static str,
    version: u8,
    url: String,
    host: String,
    port: u16,
    certificate_sha256: String,
}

impl RemoteAccess {
    pub(crate) fn load(store: &Store, data_dir: &Path) -> Result<Self, String> {
        let enabled = store.remote_access_enabled().map_err(|error| error.to_string())?;
        let (auth_revision, _) = watch::channel(0);
        Ok(Self {
            config: Mutex::new(RemoteConfig { enabled, error: None }),
            auth: Mutex::new(Auth::load(store)?),
            tls: Arc::new(Tls::load_or_create(&data_dir.join("remote-tls"))?),
            password_checks: tokio::sync::Semaphore::new(MAX_CONCURRENT_PASSWORD_CHECKS),
            running: AsyncMutex::new(None),
            transition: AsyncMutex::new(()),
            auth_revision,
        })
    }

    pub(crate) fn should_start(&self) -> bool {
        self.config.lock().unwrap().enabled
    }

    pub(crate) async fn start(&self, app: tauri::AppHandle) -> Result<(), String> {
        let mut running = self.running.lock().await;
        if !self.config.lock().unwrap().enabled {
            return Err("remote access is disabled".into());
        }
        if running.is_some() {
            return Ok(());
        }
        let http_listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, HTTP_PORT))
            .await
            .map_err(|error| format!("could not listen on port {HTTP_PORT}: {error}"))?;
        let discovery_socket = tokio::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT))
            .await
            .map_err(|error| format!("could not listen for LAN discovery: {error}"))?;
        discovery_socket
            .set_broadcast(true)
            .map_err(|error| error.to_string())?;
        let (shutdown, http_shutdown) = watch::channel(false);
        let discovery_shutdown = shutdown.subscribe();
        let http = tokio::spawn(accept_loop(http_listener, router(app.clone()), self.tls.clone(), http_shutdown));
        let fingerprint = self.tls.fingerprint().to_string();
        let discovery = tokio::spawn(discovery_loop(discovery_socket, fingerprint, discovery_shutdown));
        *running = Some(Running {
            shutdown,
            http,
            discovery,
        });
        self.config.lock().unwrap().error = None;
        Ok(())
    }

    pub(crate) async fn stop(&self) {
        let running = self.running.lock().await.take();
        if let Some(mut running) = running {
            let _ = running.shutdown.send(true);
            if tokio::time::timeout(std::time::Duration::from_millis(500), &mut running.http)
                .await
                .is_err()
            {
                running.http.abort();
            }
            if tokio::time::timeout(
                std::time::Duration::from_millis(500),
                &mut running.discovery,
            )
            .await
            .is_err()
            {
                running.discovery.abort();
            }
        }
    }

    pub(crate) fn stop_on_exit(&self) {
        if let Ok(running) = self.running.try_lock() {
            if let Some(running) = running.as_ref() {
                let _ = running.shutdown.send(true);
            }
        }
    }

    async fn status(&self) -> RemoteStatus {
        let config = self.config.lock().unwrap().clone();
        let listening = self.running.lock().await.is_some();
        let mut status = status_for(&config, listening);
        let mut auth = self.auth();
        status.devices = auth.devices();
        status.pending_links = auth.pending();
        status.password_username = auth.password_username();
        status.certificate_fingerprint = self.tls.fingerprint().to_string();
        status
    }

    pub(crate) fn certificate(&self) -> Vec<u8> {
        self.tls.ca_der().to_vec()
    }

    pub(crate) fn auth(&self) -> std::sync::MutexGuard<'_, Auth> {
        self.auth.lock().unwrap()
    }

    pub(crate) fn password_checks(&self) -> &tokio::sync::Semaphore {
        &self.password_checks
    }

    fn authorize(&self, headers: &HeaderMap, store: &Store) -> Option<(watch::Receiver<u64>, RemoteDevice)> {
        if !self.config.lock().unwrap().enabled {
            return None;
        }
        let device = self.auth().device(remote_auth::supplied_token(headers)?, store)?;
        Some((self.auth_changes(), device))
    }

    pub(crate) fn set_error(&self, error: String) {
        self.config.lock().unwrap().error = Some(error);
    }

    fn auth_changes(&self) -> watch::Receiver<u64> {
        self.auth_revision.subscribe()
    }

    pub(crate) fn invalidate_streams(&self) {
        let next = self.auth_revision.borrow().wrapping_add(1);
        let _ = self.auth_revision.send(next);
    }
}

/// Serves HTTPS; dropping the connection set on shutdown aborts every open connection.
async fn accept_loop(listener: TcpListener, router: Router, tls: Arc<Tls>, mut shutdown: watch::Receiver<bool>) {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            accepted = listener.accept() => {
                let Ok((stream, peer)) = accepted else { continue };
                connections.spawn(serve_connection(stream, peer, router.clone(), tls.clone()));
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

async fn serve_connection(stream: TcpStream, peer: SocketAddr, router: Router, tls: Arc<Tls>) {
    let mut first = [0u8; 1];
    let peeked = tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.peek(&mut first)).await;
    let Ok(local) = stream.local_addr() else { return };
    if !matches!(peeked, Ok(Ok(1))) || first[0] != TLS_HANDSHAKE_RECORD {
        return redirect_plain(stream, local).await;
    }
    let Ok(config) = tls.config_for(local.ip()) else { return };
    let accepted = tokio::time::timeout(HANDSHAKE_TIMEOUT, TlsAcceptor::from(config).accept(stream)).await;
    let Ok(Ok(stream)) = accepted else { return };
    let service = router.map_request(move |mut request: Request<hyper::body::Incoming>| {
        request.extensions_mut().insert(ConnectInfo(peer));
        request
    });
    let _ = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
        .serve_connection_with_upgrades(TokioIo::new(stream), TowerToHyperService::new(service))
        .await;
}

/// Plain-HTTP visitors (a typed address defaults to http://) are sent to the HTTPS origin.
async fn redirect_plain(mut stream: TcpStream, local: SocketAddr) {
    let mut head = vec![0u8; 8192];
    let Ok(Ok(read)) = tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.read(&mut head)).await else {
        return;
    };
    let response = plain_redirect(&String::from_utf8_lossy(&head[..read]), &local.to_string());
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn plain_redirect(head: &str, fallback_host: &str) -> String {
    let safe = |value: &str| !value.is_empty() && value.chars().all(|c| c.is_ascii_graphic());
    let target = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .filter(|target| target.starts_with('/') && safe(target))
        .unwrap_or("/");
    let host = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("host"))
        .map(|(_, value)| value.trim())
        .filter(|host| safe(host) && !host.contains(['/', '\\', '@']))
        .unwrap_or(fallback_host);
    format!(
        "HTTP/1.1 308 Permanent Redirect\r\nLocation: https://{host}{target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
}

fn status_for(config: &RemoteConfig, listening: bool) -> RemoteStatus {
    let ip = local_ipv4().filter(|ip| !ip.is_loopback());
    let urls = if config.enabled && listening {
        ip.map(|ip| vec![format!("https://{ip}:{HTTP_PORT}/companion")])
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    RemoteStatus {
        enabled: config.enabled,
        listening,
        port: HTTP_PORT,
        discovery_port: DISCOVERY_PORT,
        listening_address: (config.enabled && listening).then(|| format!("0.0.0.0:{HTTP_PORT}")),
        address_qr: urls.first().and_then(|url| address_qr(url)),
        urls,
        devices: Vec::new(),
        pending_links: Vec::new(),
        password_username: None,
        certificate_fingerprint: String::new(),
        error: config.error.clone(),
    }
}

/// The address carries no credential, so it is safe to show as a scannable code.
fn address_qr(url: &str) -> Option<String> {
    let code = qrcode::QrCode::new(url.as_bytes()).ok()?;
    Some(
        code.render::<qrcode::render::svg::Color<'_>>()
            .min_dimensions(168, 168)
            .build(),
    )
}

#[tauri::command]
pub(crate) async fn remote_access_status(
    access: tauri::State<'_, RemoteAccess>,
) -> Result<RemoteStatus, String> {
    Ok(access.status().await)
}

#[tauri::command]
pub(crate) async fn remote_access_enable(
    app: tauri::AppHandle,
    access: tauri::State<'_, RemoteAccess>,
    store: tauri::State<'_, Store>,
) -> Result<RemoteStatus, String> {
    let _transition = access.transition.lock().await;
    store.save_remote_access(true).map_err(|error| error.to_string())?;
    {
        let mut config = access.config.lock().unwrap();
        config.enabled = true;
        config.error = None;
    }
    if let Err(error) = access.start(app).await {
        {
            let mut config = access.config.lock().unwrap();
            config.enabled = false;
            config.error = Some(error.clone());
        }
        let _ = store.save_remote_access(false);
        return Err(error);
    }
    Ok(access.status().await)
}

#[tauri::command]
pub(crate) async fn remote_access_disable(
    access: tauri::State<'_, RemoteAccess>,
    store: tauri::State<'_, Store>,
) -> Result<RemoteStatus, String> {
    let _transition = access.transition.lock().await;
    store.save_remote_access(false).map_err(|error| error.to_string())?;
    {
        let mut config = access.config.lock().unwrap();
        config.enabled = false;
        config.error = None;
    }
    access.invalidate_streams();
    access.stop().await;
    Ok(access.status().await)
}

/// Approves the device showing `code`; returns that device's name.
#[tauri::command]
pub(crate) fn remote_access_link(access: tauri::State<'_, RemoteAccess>, code: String) -> Result<String, String> {
    access.auth().approve(&code)
}

/// Signs out one linked device, or all of them when `id` is omitted.
#[tauri::command]
pub(crate) async fn remote_access_revoke(
    access: tauri::State<'_, RemoteAccess>,
    store: tauri::State<'_, Store>,
    id: Option<String>,
) -> Result<RemoteStatus, String> {
    access.auth().revoke(id.as_deref(), &store)?;
    access.invalidate_streams();
    Ok(access.status().await)
}

/// Enables password sign-in with these credentials, or turns it off when both are omitted.
#[tauri::command]
pub(crate) async fn remote_access_set_password(
    access: tauri::State<'_, RemoteAccess>,
    store: tauri::State<'_, Store>,
    username: Option<String>,
    password: Option<String>,
) -> Result<RemoteStatus, String> {
    let credentials = match (username, password) {
        (Some(username), Some(password)) => {
            remote_auth::validate_credentials(&username, &password)?;
            let hash = tokio::task::spawn_blocking(move || remote_auth::new_password_hash(&password))
                .await
                .map_err(|error| error.to_string())?;
            Some((username.trim().to_string(), hash))
        }
        (None, None) => None,
        _ => return Err("Enter both a username and a password.".into()),
    };
    access.auth().set_password(credentials, &store)?;
    access.invalidate_streams();
    Ok(access.status().await)
}

fn router(app: tauri::AppHandle) -> Router {
    Router::new()
        .route("/", get(|| async { Redirect::temporary("/companion") }))
        .route("/companion", get(static_asset))
        .route("/auth/options", get(remote_auth::options))
        .route("/auth/certificate", get(remote_auth::certificate))
        .route("/auth/link", post(remote_auth::start_link))
        .route("/auth/link/{id}", get(remote_auth::poll_link))
        .route("/auth/login", post(remote_auth::login))
        .route("/auth/logout", post(remote_auth::logout))
        .route("/auth/me", get(remote_auth::me))
        .route("/engine", any(proxy_engine))
        .route("/engine/{*path}", any(proxy_engine))
        .route("/api/invoke", post(invoke_rpc))
        .route("/api/ui-state/events", get(ui_state_events))
        .fallback(static_asset)
        .layer(DefaultBodyLimit::max(MAX_RPC_BODY))
        .layer(middleware::from_fn_with_state(
            app.clone(),
            gateway_middleware,
        ))
        .with_state(app)
}

async fn ui_state_events(
    State(app): State<tauri::AppHandle>,
    Extension(auth): Extension<watch::Receiver<u64>>,
) -> Response {
    let authority = app.state::<ui_state::UiStateAuthority>();
    let Ok(initial) = authority.snapshot() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "desktop UI state has not been initialized",
        )
            .into_response();
    };
    let receiver = authority.subscribe();
    let stream = futures_util::stream::unfold(
        (Some(initial), receiver, auth),
        |(initial, mut receiver, mut auth)| async move {
            if let Some(snapshot) = initial {
                let event = Event::default().json_data(snapshot).ok()?;
                return Some((
                    Ok::<_, std::convert::Infallible>(event),
                    (None, receiver, auth),
                ));
            }
            loop {
                tokio::select! {
                    changed = auth.changed() => {
                        let _ = changed;
                        return None;
                    }
                    received = receiver.recv() => match received {
                        Ok(snapshot) => {
                            let event = Event::default().json_data(snapshot).ok()?;
                            return Some((Ok(event), (None, receiver, auth)));
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return None,
                    }
                }
            }
        },
    );
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn gateway_middleware(
    State(app): State<tauri::AppHandle>,
    mut request: Request,
    next: Next,
) -> Response {
    let access = app.state::<RemoteAccess>();
    let path = request.uri().path().to_string();
    let enabled = access.config.lock().unwrap().enabled;
    let response = if !valid_host_origin(request.headers()) {
        (StatusCode::FORBIDDEN, "invalid host or origin").into_response()
    } else if enabled && remote_auth::public_path(&path) {
        next.run(request).await
    } else if let Some((auth, device)) = access.authorize(request.headers(), &app.state::<Store>()) {
        request.extensions_mut().insert(auth);
        request.extensions_mut().insert(device);
        next.run(request).await
    } else if enabled && request.method() == axum::http::Method::GET && remote_auth::sign_in_path(&path) {
        remote_auth::sign_in_page()
    } else {
        (StatusCode::UNAUTHORIZED, "remote access authentication required").into_response()
    };
    secure(response)
}

fn valid_host_origin(headers: &HeaderMap) -> bool {
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    if host.is_empty() || host.contains(['/', '\\', '@']) {
        return false;
    }
    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        return true;
    };
    let Ok(origin) = url::Url::parse(origin) else {
        return false;
    };
    origin.scheme() == "https"
        && origin[url::Position::BeforeHost..url::Position::AfterPort] == *host
}

fn secure(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static("frame-ancestors 'self'"),
    );
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), geolocation=()"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn static_asset(uri: Uri) -> Response {
    let Some(path) = static_path(uri.path()) else {
        return (StatusCode::BAD_REQUEST, "invalid asset path").into_response();
    };
    let path = if path.is_empty() || path == "companion" {
        "index.html"
    } else {
        path
    };
    let content =
        dev_asset(path).or_else(|| FrontendAssets::get(path).map(|asset| asset.data.into_owned()));
    let Some(content) = content.or_else(|| {
        (!Path::new(path).extension().is_some())
            .then(|| {
                dev_asset("index.html").or_else(|| {
                    FrontendAssets::get("index.html").map(|asset| asset.data.into_owned())
                })
            })
            .flatten()
    }) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut response = Body::from(content).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref()).unwrap(),
    );
    response
}

fn dev_asset(path: &str) -> Option<Vec<u8>> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("dist");
    std::fs::read(root.join(path)).ok()
}

fn static_path(path: &str) -> Option<&str> {
    let raw = path.trim_start_matches('/');
    let lower = raw.to_ascii_lowercase();
    if raw.contains('\\') || lower.contains("%2e") || lower.contains("%2f") || lower.contains("%5c")
    {
        return None;
    }
    if Path::new(raw)
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
        && !raw.is_empty()
    {
        return None;
    }
    Some(raw)
}

async fn proxy_engine(
    State(app): State<tauri::AppHandle>,
    Extension(mut auth): Extension<watch::Receiver<u64>>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    if parts
        .headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_PROXY_BODY)
    {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            "engine request body is too large",
        )
            .into_response();
    }
    let (engine_url, password) = {
        let engine = app.state::<engine::Engine>();
        let url = engine.current_url();
        (url, engine.password.clone())
    };
    let Some(engine_url) = engine_url else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "embedded engine is starting",
        )
            .into_response();
    };
    let suffix = parts
        .uri
        .path()
        .strip_prefix("/engine")
        .unwrap_or(parts.uri.path());
    let mut target = format!("{engine_url}{suffix}");
    if let Some(query) = parts.uri.query() {
        target.push('?');
        target.push_str(query);
    }
    let method = match reqwest::Method::from_bytes(parts.method.as_str().as_bytes()) {
        Ok(method) => method,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let mut size = 0usize;
    let stream = body.into_data_stream().map(move |chunk| {
        let chunk = chunk.map_err(std::io::Error::other)?;
        size += chunk.len();
        if size > MAX_PROXY_BODY {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request body limit exceeded",
            ));
        }
        Ok(chunk)
    });
    let client = match proxy_client() {
        Ok(client) => client,
        Err(error) => return (StatusCode::BAD_GATEWAY, error).into_response(),
    };
    let mut outgoing = client
        .request(method, target)
        .body(reqwest::Body::wrap_stream(stream))
        .header(
            header::AUTHORIZATION.as_str(),
            format!(
                "Basic {}",
                engine::basic_authorization("opencode", &password)
            ),
        );
    for (name, value) in &parts.headers {
        if request_header_allowed(name, &parts.headers) {
            outgoing = outgoing.header(name.as_str(), value.as_bytes());
        }
    }
    let response = tokio::select! {
        changed = auth.changed() => {
            let _ = changed;
            return (StatusCode::UNAUTHORIZED, "remote access credentials changed").into_response();
        }
        response = outgoing.send() => match response {
            Ok(response) => response,
            Err(error) => return (StatusCode::BAD_GATEWAY, error.to_string()).into_response(),
        }
    };
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let response_headers = response.headers().clone();
    let stream = revoke_on_auth_change(response.bytes_stream(), auth)
        .map(|chunk| chunk.map(Bytes::from).map_err(std::io::Error::other));
    let mut proxied = Response::new(Body::from_stream(stream));
    *proxied.status_mut() = status;
    for (name, value) in &response_headers {
        if response_header_allowed(name, &response_headers) {
            proxied.headers_mut().append(name.clone(), value.clone());
        }
    }
    proxied.headers_mut().insert(
        HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
    proxied
}

static PROXY_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn proxy_client() -> Result<&'static reqwest::Client, String> {
    if let Some(client) = PROXY_CLIENT.get() {
        return Ok(client);
    }
    let client = reqwest::Client::builder()
        .redirect(Policy::none())
        .build()
        .map_err(|error| error.to_string())?;
    Ok(PROXY_CLIENT.get_or_init(|| client))
}

fn revoke_on_auth_change<S>(
    stream: S,
    mut auth: watch::Receiver<u64>,
) -> impl futures_util::Stream<Item = S::Item>
where
    S: futures_util::Stream,
{
    stream.take_until(async move {
        let _ = auth.changed().await;
    })
}

fn request_header_allowed(name: &HeaderName, headers: &HeaderMap) -> bool {
    !matches!(
        name.as_str(),
        "authorization"
            | "cookie"
            | "host"
            | "origin"
            | "referer"
            | "connection"
            | "proxy-connection"
            | "keep-alive"
            | "transfer-encoding"
            | "upgrade"
            | "te"
            | "trailer"
    ) && !connection_names(headers)
        .iter()
        .any(|item| item == name.as_str())
}

fn response_header_allowed(name: &HeaderName, headers: &HeaderMap) -> bool {
    !matches!(
        name.as_str(),
        "connection"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "keep-alive"
            | "transfer-encoding"
            | "upgrade"
            | "te"
            | "trailer"
            | "set-cookie"
            | "access-control-allow-origin"
    ) && !connection_names(headers)
        .iter()
        .any(|item| item == name.as_str())
}

fn connection_names(headers: &HeaderMap) -> Vec<String> {
    headers
        .get(header::CONNECTION)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(',')
                .map(|item| item.trim().to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct RpcRequest {
    command: String,
    #[serde(default)]
    args: Value,
}

macro_rules! remote_commands {
    (
        |$app:ident, $args:ident, $store:ident, $runtime:ident|;
        $($name:literal => $handler:expr),+ $(,)?
    ) => {
        fn rpc_allowed(command: &str) -> bool {
            matches!(command, $($name)|+)
        }

        async fn dispatch_rpc(
            $app: &tauri::AppHandle,
            command: &str,
            $args: &Value,
        ) -> Result<Value, String> {
            let $store = || $app.state::<Store>();
            let $runtime = || $app.state::<mcp::McpRuntime>();
            match command {
                $($name => $handler,)+
                _ => Err("command is not available remotely".into()),
            }
        }
    };
}

async fn invoke_rpc(
    State(app): State<tauri::AppHandle>,
    Extension(mut auth): Extension<watch::Receiver<u64>>,
    Json(request): Json<RpcRequest>,
) -> Response {
    if !rpc_allowed(&request.command) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "command is not available remotely" })),
        )
            .into_response();
    }
    let result = tokio::select! {
        changed = auth.changed() => {
            let _ = changed;
            return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "remote access credentials changed" }))).into_response();
        }
        result = dispatch_rpc(&app, &request.command, &request.args) => result,
    };
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response(),
    }
}

remote_commands! {
    |app, args, store, runtime|;
        "restart_engine" => value(engine::restart_engine(app.clone())?),
        "config_read" => value(config::config_read(app.state(), arg(args, "path")?)?),
        "pick_folder" => value(editor::pick_folder().await),
        "open_file" => value(editor::open_file(
            app.clone(),
            arg(args, "path")?,
            optional(args, "line")?,
            optional(args, "column")?,
        )?),
        "open_file_in_editor" => value(editor::open_file_in_editor(
            arg(args, "path")?,
            optional(args, "line")?,
            optional(args, "column")?,
        )?),
        "read_file_preview" => value(
            file_preview::read_file_preview(
                arg(args, "path")?,
                arg(args, "directory")?,
                arg(args, "maxBytes")?,
            )
            .await?,
        ),
        "store_workspaces" => value(commands::store_workspaces(store())?),
        "store_removed_workspaces" => {
            value(commands::store_removed_workspaces(store())?)
        },
        "store_add_workspace" => value(commands::store_add_workspace(
            store(),
            arg(args, "id")?,
            arg(args, "path")?,
            arg(args, "name")?,
            arg(args, "icon")?,
        )?),
        "store_save_workspace" => value(commands::store_save_workspace(
            store(),
            arg(args, "id")?,
            arg(args, "path")?,
            arg(args, "name")?,
            arg(args, "icon")?,
        )?),
        "store_touch_workspace" => {
            value(commands::store_touch_workspace(store(), arg(args, "id")?)?)
        },
        "store_remove_workspace" => {
            value(commands::store_remove_workspace(store(), arg(args, "id")?)?)
        },
        "store_expired_removed_workspaces" => value(
            commands::store_expired_removed_workspaces(store(), arg(args, "before")?)?,
        ),
        "store_forget_workspace" => {
            value(commands::store_forget_workspace(store(), arg(args, "id")?)?)
        },
        "store_archived" => value(commands::store_archived(store())?),
        "store_archive_session" => value(commands::store_archive_session(
            store(),
            arg(args, "sessionId")?,
            arg(args, "workspaceId")?,
        )?),
        "store_unarchive_session" => value(commands::store_unarchive_session(
            store(),
            arg(args, "sessionId")?,
        )?),
        "store_expired_archived" => value(commands::store_expired_archived(
            store(),
            arg(args, "before")?,
        )?),
        "mcp_snapshot" => value(commands::mcp_snapshot(
            runtime(),
            store(),
            arg(args, "directory")?,
        )?),
        "prompt_snapshot" => value(commands::prompt_snapshot(runtime(), store())?),
        "prompt_save" => value(commands::prompt_save(
            app.clone(),
            runtime(),
            store(),
            arg(args, "key")?,
            arg(args, "value")?,
            optional(args, "original")?,
        )?),
        "prompt_reset" => value(commands::prompt_reset(
            app.clone(),
            runtime(),
            store(),
            arg(args, "key")?,
        )?),
        "mcp_save" => value(commands::mcp_save(
            app.clone(),
            runtime(),
            store(),
            arg(args, "name")?,
            optional(args, "previousName")?,
            arg(args, "config")?,
            arg(args, "generation")?,
        )?),
        "mcp_remove" => value(commands::mcp_remove(
            app.clone(),
            runtime(),
            store(),
            arg(args, "name")?,
            arg(args, "generation")?,
        )?),
        "mcp_external_config" => value(commands::mcp_external_config(
            runtime(),
            store(),
            arg(args, "name")?,
            arg(args, "fingerprint")?,
            arg(args, "generation")?,
        )?),
        "mcp_external_save" => value(commands::mcp_external_save(
            runtime(),
            store(),
            arg(args, "name")?,
            arg(args, "previousName")?,
            arg(args, "fingerprint")?,
            arg(args, "config")?,
            arg(args, "generation")?,
        )?),
        "mcp_external_remove" => value(commands::mcp_external_remove(
            runtime(),
            store(),
            arg(args, "name")?,
            arg(args, "fingerprint")?,
            arg(args, "generation")?,
        )?),
        "mcp_approve" => value(commands::mcp_approve(
            app.clone(),
            runtime(),
            store(),
            arg(args, "directory")?,
            arg(args, "name")?,
            arg(args, "fingerprint")?,
            arg(args, "generation")?,
        )?),
        "mcp_reject" => value(commands::mcp_reject(
            app.clone(),
            runtime(),
            store(),
            arg(args, "directory")?,
            arg(args, "name")?,
            arg(args, "fingerprint")?,
            arg(args, "generation")?,
        )?),
        "mcp_revoke" => value(commands::mcp_revoke(
            app.clone(),
            runtime(),
            store(),
            arg(args, "directory")?,
            arg(args, "name")?,
            arg(args, "fingerprint")?,
            arg(args, "generation")?,
        )?),
        "storage_stats" => value(commands::storage_stats(store()).await?),
        "storage_analyze" => value(commands::storage_analyze(store()).await?),
        "storage_prune" => {
            value(commands::storage_prune(store(), arg(args, "rules")?).await?)
        },
        "storage_compact" => value(commands::storage_compact().await?),
        "voice_supported" => value(voice::voice_supported()),
        "voice_acceleration" => value(voice::voice_acceleration()),
        "voice_models" => value(voice::voice_models(app.clone())?),
        "voice_model_download" => {
            value(voice::voice_model_download(app.clone(), app.state(), arg(args, "id")?).await?)
        },
        "voice_model_remove" => {
            value(voice::voice_model_remove(app.clone(), arg(args, "id")?)?)
        },
        "voice_model_cancel" => {
            voice::voice_model_cancel(app.state());
            value(())
        },
        "voice_transcribe" => value(
            voice::voice_transcribe(
                app.clone(),
                arg(args, "id")?,
                arg(args, "audio")?,
                arg(args, "language")?,
                arg(args, "prompt")?,
            )
            .await?,
        ),
        "ui_state_snapshot" => value(ui_state::ui_state_snapshot(app.state())?),
        "ui_state_update" => value(ui_state::ui_state_update(
            app.clone(),
            app.state(),
            store(),
            arg(args, "mutation")?,
        )?),
        "shell_timeout_snapshot" => {
            value(ui_state::shell_timeout_snapshot(app.state())?)
        },
        "tool_routing_snapshot" => value(tool_routing::tool_routing_snapshot(store())?),
        "tool_routing_status" => value(tool_routing::tool_routing_status(app.state())),
        "tool_routing_update" => value(tool_routing::tool_routing_update(app.clone(), app.state(), store(), arg(args, "policy")?)?),
        "shell_timeout_update" => value(ui_state::shell_timeout_update(
            app.clone(),
            app.state(),
            store(),
            arg(args, "policy")?,
        )?),
}

fn arg<T: DeserializeOwned>(args: &Value, key: &str) -> Result<T, String> {
    let value = args
        .get(key)
        .cloned()
        .ok_or_else(|| format!("missing argument: {key}"))?;
    serde_json::from_value(value).map_err(|error| format!("invalid argument {key}: {error}"))
}

fn optional<T: DeserializeOwned>(args: &Value, key: &str) -> Result<Option<T>, String> {
    args.get(key)
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("invalid argument {key}: {error}"))
}

fn value<T: Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}

async fn discovery_loop(socket: tokio::net::UdpSocket, fingerprint: String, mut shutdown: watch::Receiver<bool>) {
    let mut buffer = [0u8; 256];
    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            received = socket.recv_from(&mut buffer) => {
                let Ok((size, peer)) = received else { continue };
                if &buffer[..size] != DISCOVERY_PROBE { continue; }
                let Some(ip) = local_ipv4_for(peer) else { continue };
                let descriptor = discovery_descriptor(ip, &fingerprint);
                if let Ok(payload) = serde_json::to_vec(&descriptor) {
                    let _ = socket.send_to(&payload, peer).await;
                }
            }
        }
    }
}

/// Version 2 moved to HTTPS; clients may pin `certificateSha256`, the gateway CA fingerprint.
fn discovery_descriptor(ip: Ipv4Addr, fingerprint: &str) -> DiscoveryDescriptor {
    DiscoveryDescriptor {
        kind: "drift-companion",
        name: "Drift",
        brand: "Drift",
        protocol: "drift-remote",
        version: 2,
        url: format!("https://{ip}:{HTTP_PORT}/companion"),
        host: ip.to_string(),
        port: HTTP_PORT,
        certificate_sha256: fingerprint.into(),
    }
}

fn local_ipv4_for(peer: SocketAddr) -> Option<Ipv4Addr> {
    let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(peer).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_unspecified() && !ip.is_loopback() => Some(ip),
        _ => None,
    }
}

fn local_ipv4() -> Option<Ipv4Addr> {
    local_ipv4_for(SocketAddr::from(([8, 8, 8, 8], 53)))
}

pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        difference |= left.get(index).copied().unwrap_or(0) as usize
            ^ right.get(index).copied().unwrap_or(0) as usize;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_status_has_no_listening_urls_or_code() {
        let status = status_for(&RemoteConfig { enabled: false, error: None }, false);
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
        assert!(rpc_allowed("store_workspaces"));
        assert!(rpc_allowed("store_expired_archived"));
        assert!(rpc_allowed("voice_transcribe"));
        assert!(rpc_allowed("ui_state_snapshot"));
        assert!(rpc_allowed("ui_state_update"));
        assert!(rpc_allowed("shell_timeout_snapshot"));
        assert!(rpc_allowed("shell_timeout_update"));
        assert!(rpc_allowed("pick_folder"));
        assert!(rpc_allowed("open_file"));
        assert!(rpc_allowed("open_file_in_editor"));
        assert!(rpc_allowed("read_file_preview"));
        assert!(!rpc_allowed("voice_dictation_set_enabled"));
        assert!(!rpc_allowed("remote_access_enable"));
        assert!(!rpc_allowed("ui_state_initialize"));
        assert!(!rpc_allowed("shell_timeout_initialize"));
        assert!(!rpc_allowed("plugin:shell|execute"));
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
            assert_eq!(error, format!("missing argument: {key}"));
        }
        for invalid in [Value::Null, json!(false), json!(1), json!([]), json!({})] {
            for key in ["path", "directory"] {
                let mut args = valid.clone();
                args[key] = invalid.clone();
                assert!(arg::<String>(&args, key)
                    .unwrap_err()
                    .contains("invalid argument"));
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
            assert!(arg::<u64>(&args, "maxBytes")
                .unwrap_err()
                .contains("invalid argument"));
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
        assert_eq!(
            mime_guess::from_path("font.woff2").first_raw(),
            Some("font/woff2")
        );
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
        let router = Router::new().route(
            "/peer",
            get(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move { peer.ip().to_string() }),
        );
        let (shutdown, receiver) = watch::channel(false);
        let server = tokio::spawn(accept_loop(listener, router, tls.clone(), receiver));

        let mut roots = rustls::RootCertStore::empty();
        roots.add(rustls::pki_types::CertificateDer::from(tls.ca_der().to_vec())).unwrap();
        let mut config = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let client = reqwest::Client::builder().use_preconfigured_tls(config).redirect(Policy::none()).build().unwrap();
        let secure = client.get(format!("https://127.0.0.1:{port}/peer")).send().await.unwrap();
        assert_eq!(secure.version(), reqwest::Version::HTTP_2);
        assert_eq!(secure.text().await.unwrap(), "127.0.0.1");

        let plain = client.get(format!("http://127.0.0.1:{port}/companion?a=1")).send().await.unwrap();
        assert_eq!(plain.status(), reqwest::StatusCode::PERMANENT_REDIRECT);
        assert_eq!(plain.headers()["location"], format!("https://127.0.0.1:{port}/companion?a=1"));

        let untrusted = reqwest::Client::builder()
            .use_preconfigured_tls(
                rustls::ClientConfig::builder()
                    .with_root_certificates(rustls::RootCertStore::empty())
                    .with_no_client_auth(),
            )
            .build()
            .unwrap();
        assert!(untrusted.get(format!("https://127.0.0.1:{port}/peer")).send().await.is_err());

        shutdown.send(true).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), server).await.unwrap().unwrap();
        assert!(client.get(format!("https://127.0.0.1:{port}/peer")).send().await.is_err());
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

    #[test]
    fn origins_must_match_the_https_host() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("192.168.1.20:41718"));
        assert!(valid_host_origin(&headers));
        headers.insert(header::ORIGIN, HeaderValue::from_static("https://192.168.1.20:41718"));
        assert!(valid_host_origin(&headers));
        headers.insert(header::ORIGIN, HeaderValue::from_static("http://192.168.1.20:41718"));
        assert!(!valid_host_origin(&headers));
        headers.insert(header::ORIGIN, HeaderValue::from_static("https://evil.example"));
        assert!(!valid_host_origin(&headers));
    }

    #[test]
    fn proxy_headers_strip_credentials_and_hop_by_hop_names() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONNECTION, HeaderValue::from_static("x-private"));
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer remote"),
        );
        headers.insert(
            HeaderName::from_static("x-private"),
            HeaderValue::from_static("no"),
        );
        headers.insert(
            HeaderName::from_static("x-next-cursor"),
            HeaderValue::from_static("yes"),
        );
        assert!(!request_header_allowed(&header::AUTHORIZATION, &headers));
        assert!(!request_header_allowed(
            &HeaderName::from_static("x-private"),
            &headers
        ));
        assert!(response_header_allowed(
            &HeaderName::from_static("x-next-cursor"),
            &headers
        ));
    }
}
