use crate::{commands, config, editor, engine, mcp, store::Store, voice};
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use reqwest::redirect::Policy;
use rust_embed::RustEmbed;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Component, Path};
use std::sync::Mutex;
use tauri::Manager;
use tokio::sync::{watch, Mutex as AsyncMutex};
use tokio::task::JoinHandle;

pub(crate) const HTTP_PORT: u16 = 41718;
pub(crate) const DISCOVERY_PORT: u16 = 41717;
const DISCOVERY_PROBE: &[u8] = b"OPENCODE_COMPANION_DISCOVERY";
const COOKIE_NAME: &str = "drift_remote";
const MAX_PROXY_BODY: usize = 32 * 1024 * 1024;
const MAX_RPC_BODY: usize = 10 * 1024 * 1024;

#[derive(RustEmbed)]
#[folder = "../dist"]
struct FrontendAssets;

#[derive(Clone)]
struct RemoteConfig {
    enabled: bool,
    token: String,
    error: Option<String>,
}

struct Running {
    shutdown: watch::Sender<bool>,
    http: JoinHandle<()>,
    discovery: JoinHandle<()>,
}

pub(crate) struct RemoteAccess {
    config: Mutex<RemoteConfig>,
    running: AsyncMutex<Option<Running>>,
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
    connection_urls: Vec<String>,
    error: Option<String>,
}

#[derive(Serialize)]
struct DiscoveryDescriptor {
    kind: &'static str,
    name: &'static str,
    brand: &'static str,
    protocol: &'static str,
    version: u8,
    url: String,
    host: String,
    port: u16,
}

impl RemoteAccess {
    pub(crate) fn load(store: &Store) -> Result<Self, String> {
        let saved = store.remote_access().map_err(|error| error.to_string())?;
        let (enabled, token) = saved.unwrap_or_else(|| (false, random_token()));
        Ok(Self {
            config: Mutex::new(RemoteConfig {
                enabled,
                token,
                error: None,
            }),
            running: AsyncMutex::new(None),
        })
    }

    pub(crate) fn should_start(&self) -> bool {
        self.config.lock().unwrap().enabled
    }

    pub(crate) async fn start(&self, app: tauri::AppHandle) -> Result<(), String> {
        if !self.config.lock().unwrap().enabled {
            return Err("remote access is disabled".into());
        }
        let mut running = self.running.lock().await;
        if running.is_some() {
            return Ok(());
        }
        let http_listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, HTTP_PORT))
            .await
            .map_err(|error| format!("could not listen on port {HTTP_PORT}: {error}"))?;
        let discovery_socket = tokio::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT))
            .await
            .map_err(|error| format!("could not listen for Deck discovery: {error}"))?;
        discovery_socket
            .set_broadcast(true)
            .map_err(|error| error.to_string())?;
        let (shutdown, http_shutdown) = watch::channel(false);
        let discovery_shutdown = shutdown.subscribe();
        let http_app = router(app.clone());
        let http = tokio::spawn(async move {
            let result = axum::serve(http_listener, http_app)
                .with_graceful_shutdown(wait_for_shutdown(http_shutdown))
                .await;
            if let Err(error) = result {
                eprintln!("remote access HTTP listener stopped: {error}");
            }
        });
        let discovery = tokio::spawn(discovery_loop(app, discovery_socket, discovery_shutdown));
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
        status_for(&config, listening)
    }

    fn token_if_enabled(&self) -> Option<String> {
        let config = self.config.lock().unwrap();
        config.enabled.then(|| config.token.clone())
    }

    pub(crate) fn set_error(&self, error: String) {
        self.config.lock().unwrap().error = Some(error);
    }
}

async fn wait_for_shutdown(mut shutdown: watch::Receiver<bool>) {
    if !*shutdown.borrow() {
        let _ = shutdown.changed().await;
    }
}

fn status_for(config: &RemoteConfig, listening: bool) -> RemoteStatus {
    let ip = local_ipv4().filter(|ip| !ip.is_loopback());
    let urls = if config.enabled && listening {
        ip.map(|ip| vec![format!("http://{ip}:{HTTP_PORT}/companion")])
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let connection_urls = urls
        .iter()
        .map(|url| format!("{url}?token={}", config.token))
        .collect();
    RemoteStatus {
        enabled: config.enabled,
        listening,
        port: HTTP_PORT,
        discovery_port: DISCOVERY_PORT,
        listening_address: (config.enabled && listening).then(|| format!("0.0.0.0:{HTTP_PORT}")),
        urls,
        connection_urls,
        error: config.error.clone(),
    }
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
    let token = {
        let mut config = access.config.lock().unwrap();
        if config.token.is_empty() {
            config.token = random_token();
        }
        config.enabled = true;
        config.error = None;
        config.token.clone()
    };
    store
        .save_remote_access(true, &token)
        .map_err(|error| error.to_string())?;
    if let Err(error) = access.start(app).await {
        {
            let mut config = access.config.lock().unwrap();
            config.enabled = false;
            config.error = Some(error.clone());
        }
        let _ = store.save_remote_access(false, &token);
        return Err(error);
    }
    Ok(access.status().await)
}

#[tauri::command]
pub(crate) async fn remote_access_disable(
    access: tauri::State<'_, RemoteAccess>,
    store: tauri::State<'_, Store>,
) -> Result<RemoteStatus, String> {
    let token = {
        let mut config = access.config.lock().unwrap();
        config.enabled = false;
        config.error = None;
        config.token.clone()
    };
    store
        .save_remote_access(false, &token)
        .map_err(|error| error.to_string())?;
    access.stop().await;
    Ok(access.status().await)
}

#[tauri::command]
pub(crate) async fn remote_access_rotate_token(
    access: tauri::State<'_, RemoteAccess>,
    store: tauri::State<'_, Store>,
) -> Result<RemoteStatus, String> {
    let (enabled, token) = {
        let mut config = access.config.lock().unwrap();
        config.token = random_token();
        config.error = None;
        (config.enabled, config.token.clone())
    };
    store
        .save_remote_access(enabled, &token)
        .map_err(|error| error.to_string())?;
    Ok(access.status().await)
}

#[tauri::command]
pub(crate) async fn remote_access_urls(
    access: tauri::State<'_, RemoteAccess>,
) -> Result<Vec<String>, String> {
    Ok(access.status().await.connection_urls)
}

fn router(app: tauri::AppHandle) -> Router {
    Router::new()
        .route("/", get(|| async { Redirect::temporary("/companion") }))
        .route("/companion", get(static_asset))
        .route("/engine", any(proxy_engine))
        .route("/engine/{*path}", any(proxy_engine))
        .route("/api/invoke", post(invoke_rpc))
        .fallback(static_asset)
        .layer(DefaultBodyLimit::max(MAX_RPC_BODY))
        .layer(middleware::from_fn_with_state(
            app.clone(),
            gateway_middleware,
        ))
        .with_state(app)
}

async fn gateway_middleware(
    State(app): State<tauri::AppHandle>,
    request: Request,
    next: Next,
) -> Response {
    let response = if !valid_host_origin(request.headers()) {
        (StatusCode::FORBIDDEN, "invalid host or origin").into_response()
    } else if let Some(response) = cookie_exchange(&app, request.uri()) {
        response
    } else if !authorized(&app, request.headers()) {
        (
            StatusCode::UNAUTHORIZED,
            "remote access authentication required",
        )
            .into_response()
    } else {
        next.run(request).await
    };
    secure(response)
}

fn cookie_exchange(app: &tauri::AppHandle, uri: &Uri) -> Option<Response> {
    let token = app.state::<RemoteAccess>().token_if_enabled();
    cookie_exchange_with_token(uri, token.as_deref())
}

fn cookie_exchange_with_token(uri: &Uri, token: Option<&str>) -> Option<Response> {
    if uri.path() != "/companion" {
        return None;
    }
    let supplied = uri.query().and_then(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .find_map(|(key, value)| (key == "token").then(|| value.into_owned()))
    })?;
    let token = token?;
    if !constant_time_eq(supplied.as_bytes(), token.as_bytes()) {
        return Some((StatusCode::UNAUTHORIZED, "invalid access key").into_response());
    }
    let mut response = Redirect::to("/companion").into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "{COOKIE_NAME}={token}; HttpOnly; SameSite=Strict; Path=/"
        ))
        .unwrap(),
    );
    Some(response)
}

fn authorized(app: &tauri::AppHandle, headers: &HeaderMap) -> bool {
    let Some(token) = app.state::<RemoteAccess>().token_if_enabled() else {
        return false;
    };
    supplied_bearer(headers)
        .or_else(|| supplied_cookie(headers))
        .is_some_and(|supplied| constant_time_eq(supplied.as_bytes(), token.as_bytes()))
}

fn supplied_bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn supplied_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|value| value.strip_prefix(&format!("{COOKIE_NAME}=")))
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
    origin.scheme() == "http"
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

async fn proxy_engine(State(app): State<tauri::AppHandle>, request: Request) -> Response {
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
    let client = match reqwest::Client::builder().redirect(Policy::none()).build() {
        Ok(client) => client,
        Err(error) => return (StatusCode::BAD_GATEWAY, error.to_string()).into_response(),
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
    let response = match outgoing.send().await {
        Ok(response) => response,
        Err(error) => return (StatusCode::BAD_GATEWAY, error.to_string()).into_response(),
    };
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let response_headers = response.headers().clone();
    let stream = response
        .bytes_stream()
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

async fn invoke_rpc(
    State(app): State<tauri::AppHandle>,
    Json(request): Json<RpcRequest>,
) -> Response {
    if !rpc_allowed(&request.command) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "command is not available remotely" })),
        )
            .into_response();
    }
    match dispatch_rpc(&app, &request.command, &request.args).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response(),
    }
}

fn rpc_allowed(command: &str) -> bool {
    matches!(
        command,
        "restart_engine"
            | "config_read"
            | "pick_folder"
            | "open_file"
            | "store_workspaces"
            | "store_removed_workspaces"
            | "store_add_workspace"
            | "store_save_workspace"
            | "store_touch_workspace"
            | "store_remove_workspace"
            | "store_expired_removed_workspaces"
            | "store_forget_workspace"
            | "store_archived"
            | "store_archive_session"
            | "store_unarchive_session"
            | "store_purge_archived"
            | "mcp_snapshot"
            | "prompt_snapshot"
            | "prompt_save"
            | "prompt_reset"
            | "mcp_save"
            | "mcp_remove"
            | "mcp_approve"
            | "mcp_reject"
            | "mcp_revoke"
            | "storage_stats"
            | "storage_analyze"
            | "storage_prune"
            | "storage_compact"
            | "voice_supported"
            | "voice_acceleration"
            | "voice_models"
            | "voice_model_download"
            | "voice_model_remove"
            | "voice_model_cancel"
            | "voice_transcribe"
    )
}

async fn dispatch_rpc(
    app: &tauri::AppHandle,
    command: &str,
    args: &Value,
) -> Result<Value, String> {
    let store = || app.state::<Store>();
    let runtime = || app.state::<mcp::McpRuntime>();
    match command {
        "restart_engine" => value(engine::restart_engine(app.clone())?),
        "config_read" => value(config::config_read(app.state(), arg(args, "path")?)?),
        "pick_folder" => value(editor::pick_folder().await),
        "open_file" => value(editor::open_file(
            app.clone(),
            arg(args, "path")?,
            optional(args, "line")?,
            optional(args, "column")?,
        )?),
        "store_workspaces" => value(commands::store_workspaces(store())?),
        "store_removed_workspaces" => value(commands::store_removed_workspaces(store())?),
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
        }
        "store_remove_workspace" => {
            value(commands::store_remove_workspace(store(), arg(args, "id")?)?)
        }
        "store_expired_removed_workspaces" => value(commands::store_expired_removed_workspaces(
            store(),
            arg(args, "before")?,
        )?),
        "store_forget_workspace" => {
            value(commands::store_forget_workspace(store(), arg(args, "id")?)?)
        }
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
        "store_purge_archived" => value(commands::store_purge_archived(
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
            runtime(),
            store(),
            arg(args, "key")?,
            arg(args, "value")?,
            optional(args, "original")?,
        )?),
        "prompt_reset" => value(commands::prompt_reset(
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
        "storage_prune" => value(commands::storage_prune(store(), arg(args, "rules")?).await?),
        "storage_compact" => value(commands::storage_compact().await?),
        "voice_supported" => value(voice::voice_supported()),
        "voice_acceleration" => value(voice::voice_acceleration()),
        "voice_models" => value(voice::voice_models(app.clone())?),
        "voice_model_download" => {
            value(voice::voice_model_download(app.clone(), app.state(), arg(args, "id")?).await?)
        }
        "voice_model_remove" => value(voice::voice_model_remove(app.clone(), arg(args, "id")?)?),
        "voice_model_cancel" => {
            voice::voice_model_cancel(app.state());
            value(())
        }
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
        _ => Err("command is not available remotely".into()),
    }
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

async fn discovery_loop(
    app: tauri::AppHandle,
    socket: tokio::net::UdpSocket,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut buffer = [0u8; 256];
    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            received = socket.recv_from(&mut buffer) => {
                let Ok((size, peer)) = received else { continue };
                if &buffer[..size] != DISCOVERY_PROBE { continue; }
                let Some(token) = app.state::<RemoteAccess>().token_if_enabled() else { continue };
                let Some(ip) = local_ipv4_for(peer) else { continue };
                let descriptor = discovery_descriptor(ip, &token);
                if let Ok(payload) = serde_json::to_vec(&descriptor) {
                    let _ = socket.send_to(&payload, peer).await;
                }
            }
        }
    }
}

fn discovery_descriptor(ip: Ipv4Addr, token: &str) -> DiscoveryDescriptor {
    DiscoveryDescriptor {
        kind: "opencode-companion",
        name: "Drift",
        brand: "Drift",
        protocol: "drift-remote",
        version: 1,
        url: format!("http://{ip}:{HTTP_PORT}/companion?token={token}"),
        host: ip.to_string(),
        port: HTTP_PORT,
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

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("failed to generate remote access key");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
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
    fn bearer_auth_is_parsed_without_accepting_basic() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer secret"),
        );
        assert_eq!(supplied_bearer(&headers), Some("secret"));
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Basic secret"),
        );
        assert_eq!(supplied_bearer(&headers), None);
    }

    #[test]
    fn cookie_exchange_sets_a_strict_http_only_cookie_and_redirects() {
        let companion: Uri = "/companion?token=secret".parse().unwrap();
        let engine: Uri = "/engine/global/event?token=secret".parse().unwrap();
        let response = cookie_exchange_with_token(&companion, Some("secret")).unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/companion");
        assert_eq!(
            response.headers()[header::SET_COOKIE],
            "drift_remote=secret; HttpOnly; SameSite=Strict; Path=/"
        );
        assert!(cookie_exchange_with_token(&engine, Some("secret")).is_none());
        assert_eq!(
            cookie_exchange_with_token(&companion, Some("wrong"))
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn disabled_status_has_no_listening_urls() {
        let status = status_for(
            &RemoteConfig {
                enabled: false,
                token: "secret".into(),
                error: None,
            },
            false,
        );
        assert!(!status.enabled);
        assert!(!status.listening);
        assert!(status.urls.is_empty());
        assert!(status.connection_urls.is_empty());
    }

    #[test]
    fn token_rotation_source_is_random() {
        let first = random_token();
        let second = random_token();
        assert_eq!(first.len(), 64);
        assert_ne!(first, second);
    }

    #[test]
    fn rpc_has_a_finite_allowlist() {
        assert!(rpc_allowed("store_workspaces"));
        assert!(rpc_allowed("voice_transcribe"));
        assert!(!rpc_allowed("remote_access_enable"));
        assert!(!rpc_allowed("plugin:shell|execute"));
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
    fn discovery_is_legacy_compatible_and_branded() {
        let descriptor = discovery_descriptor(Ipv4Addr::new(192, 168, 1, 20), "secret");
        let value = serde_json::to_value(descriptor).unwrap();
        assert_eq!(value["kind"], "opencode-companion");
        assert_eq!(value["brand"], "Drift");
        assert_eq!(value["version"], 1);
        assert_eq!(
            value["url"],
            "http://192.168.1.20:41718/companion?token=secret"
        );
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
