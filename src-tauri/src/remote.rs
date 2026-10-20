pub(crate) mod access_commands;
mod gateway;
mod interfaces;
mod transport;
mod tunnel;

use gateway::router;

use transport::{accept_loop, local_ipv4};

use crate::remote_auth::{self, Auth};
use crate::remote_tls::Tls;
use crate::store::{RemoteDevice, Store};
use crate::{commands, config, editor, file_preview, prompts, ui_state, voice};
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};
use tokio::sync::{Mutex as AsyncMutex, watch};
use tokio::task::JoinHandle;
use tunnel::TunnelStatus;

pub(crate) const HTTP_PORT: u16 = 41718;
const MAX_CONCURRENT_PASSWORD_CHECKS: usize = 2;
const BROWSER_KEY: &str = "remote_browser";
const TUNNEL_KEY: &str = "remote_tunnel";
const ROUTE_KEY: &str = "remote_tunnel_route";

#[derive(Debug, thiserror::Error)]
pub(crate) enum RemoteError {
    #[error("remote access is disabled")]
    Disabled,
    #[error("could not listen on port {HTTP_PORT}: {0}")]
    Listen(#[source] std::io::Error),
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    Auth(#[from] remote_auth::AuthError),
    #[error(transparent)]
    Tls(#[from] crate::remote_tls::TlsError),
}

#[derive(Clone)]
struct RemoteConfig {
    enabled: bool,
    /// The browser version: its sign-in page, password sign-in, and cookie sessions.
    browser: bool,
    /// Reaching this PC from anywhere through OpenTunnel.
    tunnel: bool,
    error: Option<String>,
}

struct Running {
    shutdown: watch::Sender<bool>,
    http: JoinHandle<()>,
    router: Router,
    tunnel: Option<tunnel::Running>,
}

pub(crate) struct RemoteAccess {
    app: tauri::AppHandle,
    config: Mutex<RemoteConfig>,
    auth: Mutex<Auth>,
    tls: Arc<Tls>,
    tunnel_dir: PathBuf,
    tunnel_status: Mutex<TunnelStatus>,
    /// Where phones reach this PC through the tunnel, while it is up.
    tunnel_endpoint: Mutex<Option<drift_pairing::Tunnel>>,
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
    /// What a browser on this network opens, while browser access is on.
    lan_url: Option<String>,
    browser: bool,
    tunnel_enabled: bool,
    tunnel: TunnelStatus,
    devices: Vec<RemoteDevice>,
    password_username: Option<String>,
    certificate_fingerprint: String,
    error: Option<String>,
}

/// The pairing code for the Drift app: what it holds, and as a scannable SVG.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemotePairing {
    uri: String,
    qr: String,
    expires_at: i64,
}

impl RemoteAccess {
    pub(crate) fn load(app: &tauri::AppHandle, store: &Store, data_dir: &Path) -> Result<Self, RemoteError> {
        let enabled = store.remote_access_enabled()?;
        let switched_on = |key| store.app_setting(key).map(|value| value.is_some());
        let (auth_revision, _) = watch::channel(0);

        Ok(Self {
            app: app.clone(),
            config: Mutex::new(RemoteConfig {
                enabled,
                browser: switched_on(BROWSER_KEY)?,
                tunnel: switched_on(TUNNEL_KEY)?,
                error: None,
            }),
            auth: Mutex::new(Auth::load(store)?),
            tls: Arc::new(Tls::load_or_create(&data_dir.join("remote-tls"))?),
            tunnel_dir: data_dir.join("opentunnel"),
            tunnel_status: Mutex::new(TunnelStatus::OFF),
            tunnel_endpoint: Mutex::new(None),
            password_checks: tokio::sync::Semaphore::new(MAX_CONCURRENT_PASSWORD_CHECKS),
            running: AsyncMutex::new(None),
            transition: AsyncMutex::new(()),
            auth_revision,
        })
    }

    pub(crate) fn should_start(&self) -> bool {
        self.config.lock().unwrap().enabled
    }

    pub(crate) fn browser_enabled(&self) -> bool {
        self.config.lock().unwrap().browser
    }

    pub(crate) async fn start(&self) -> Result<(), RemoteError> {
        let mut running = self.running.lock().await;
        let config = self.config.lock().unwrap().clone();
        if !config.enabled {
            return Err(RemoteError::Disabled);
        }
        if running.is_some() {
            return Ok(());
        }

        let http_listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, HTTP_PORT))
            .await
            .map_err(RemoteError::Listen)?;
        let router = router(self.app.clone());
        let (shutdown, http_shutdown) = watch::channel(false);
        let http = tokio::spawn(accept_loop(
            http_listener,
            router.clone(),
            self.tls.clone(),
            http_shutdown,
        ));
        let tunnel = config.tunnel.then(|| self.start_tunnel(&router)).transpose()?;

        *running = Some(Running {
            shutdown,
            http,
            router,
            tunnel,
        });
        self.config.lock().unwrap().error = None;

        Ok(())
    }

    pub(crate) async fn stop(&self) {
        let running = self.running.lock().await.take();
        let Some(mut running) = running else { return };

        let _ = running.shutdown.send(true);
        if let Some(tunnel) = running.tunnel.take() {
            tunnel.stop().await;
        }
        if tokio::time::timeout(std::time::Duration::from_millis(500), &mut running.http)
            .await
            .is_err()
        {
            running.http.abort();
        }
        self.tunnel_changed(TunnelStatus::OFF);
    }

    pub(crate) fn stop_on_exit(&self) {
        if let Ok(running) = self.running.try_lock()
            && let Some(running) = running.as_ref()
        {
            let _ = running.shutdown.send(true);
        }
    }

    /// Turns the tunnel on or off, at once while the gateway runs and otherwise when it next starts.
    async fn set_tunnel(&self, store: &Store, enabled: bool) -> Result<(), RemoteError> {
        save_switch(store, TUNNEL_KEY, enabled)?;
        self.config.lock().unwrap().tunnel = enabled;

        let mut running = self.running.lock().await;
        let Some(running) = running.as_mut() else {
            return Ok(());
        };
        match (enabled, running.tunnel.take()) {
            (true, Some(kept)) if !kept.is_finished() => running.tunnel = Some(kept),
            (true, _) => running.tunnel = Some(self.start_tunnel(&running.router)?),
            (false, Some(tunnel)) => {
                tunnel.stop().await;
                self.tunnel_changed(TunnelStatus::OFF);
            }
            (false, None) => {}
        }

        Ok(())
    }

    fn start_tunnel(&self, router: &Router) -> Result<tunnel::Running, RemoteError> {
        let store = self.app.state::<Store>();
        let route = match store.app_setting(ROUTE_KEY)? {
            Some(route) => route,
            None => {
                let route = tunnel::new_route();
                store.save_app_setting(ROUTE_KEY, &route)?;
                route
            }
        };

        Ok(tunnel::start(
            self.app.clone(),
            router.clone(),
            self.tunnel_dir.clone(),
            route,
        ))
    }

    /// Records how the tunnel is doing and tells the desktop's settings, without waking them for nothing.
    fn tunnel_changed(&self, status: TunnelStatus) {
        let mut current = self.tunnel_status.lock().unwrap();
        if *current == status {
            return;
        }
        *current = status;
        drop(current);

        let _ = self.app.emit("remote-access-changed", ());
    }

    fn set_tunnel_endpoint(&self, endpoint: Option<drift_pairing::Tunnel>) {
        *self.tunnel_endpoint.lock().unwrap() = endpoint;
    }

    fn set_browser(&self, store: &Store, enabled: bool) -> Result<(), RemoteError> {
        save_switch(store, BROWSER_KEY, enabled)?;
        self.config.lock().unwrap().browser = enabled;
        // A browser still open must lose its event stream now, not on its next request.
        if !enabled {
            self.invalidate_streams();
        }

        Ok(())
    }

    async fn status(&self) -> RemoteStatus {
        let config = self.config.lock().unwrap().clone();
        let listening = self.running.lock().await.is_some();
        let mut status = status_for(&config, listening);
        let auth = self.auth();
        status.devices = auth.devices();
        status.password_username = auth.password_username();
        drop(auth);
        status.tunnel = self.tunnel_status.lock().unwrap().clone();
        status.certificate_fingerprint = self.tls.fingerprint().to_string();

        status
    }

    /// Where a phone reaches this PC now: its network addresses, the gateway's port, and the tunnel while it is up.
    pub(crate) fn reach(&self) -> Value {
        let (addresses, _) = interfaces::reachable(local_ipv4());
        let tunnel = self.tunnel_endpoint.lock().unwrap().clone();

        json!({
            "addresses": addresses,
            "port": HTTP_PORT,
            "tunnel": tunnel.map(|tunnel| json!({ "host": tunnel.host, "key": drift_pairing::hex(&tunnel.key) })),
        })
    }

    /// A fresh one-time pairing code, replacing any shown before; `None` while the gateway is not listening.
    async fn pairing(&self) -> Option<RemotePairing> {
        if self.running.lock().await.is_none() {
            return None;
        }
        let fingerprint = drift_pairing::parse_fingerprint(self.tls.fingerprint())?;
        let (addresses, mac) = interfaces::reachable(local_ipv4());
        let tunnel = self.tunnel_endpoint.lock().unwrap().clone();
        if addresses.is_empty() && tunnel.is_none() {
            return None;
        }

        let pairing = drift_pairing::Pairing {
            name: computer_name(),
            fingerprint,
            addresses: addresses.into_iter().map(Into::into).collect(),
            port: HTTP_PORT,
            mac,
            tunnel,
            ticket: self.auth().new_ticket(),
        };
        let uri = pairing.to_uri();
        let expires_at = remote_auth::now_ms() + remote_auth::TICKET_TTL.as_millis() as i64;

        Some(RemotePairing {
            qr: qr_svg(&uri)?,
            uri,
            expires_at,
        })
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
        let config = self.config.lock().unwrap().clone();
        if !config.enabled {
            return None;
        }

        let token = remote_auth::supplied_token(headers, config.browser)?;
        let device = self.auth().device(token, store)?;
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

fn save_switch(store: &Store, key: &str, on: bool) -> rusqlite::Result<()> {
    if on {
        store.save_app_setting(key, "1")
    } else {
        store.delete_app_setting(key)
    }
}

fn status_for(config: &RemoteConfig, listening: bool) -> RemoteStatus {
    let serving = config.enabled && listening;
    let lan_url = (serving && config.browser)
        .then(local_ipv4)
        .flatten()
        .map(|ip| format!("https://{ip}:{HTTP_PORT}"));

    RemoteStatus {
        enabled: config.enabled,
        listening,
        lan_url,
        browser: config.browser,
        tunnel_enabled: config.tunnel,
        tunnel: TunnelStatus::OFF,
        devices: Vec::new(),
        password_username: None,
        certificate_fingerprint: String::new(),
        error: config.error.clone(),
    }
}

/// The pairing code as an SVG big enough to scan from across a desk.
fn qr_svg(text: &str) -> Option<String> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    Some(
        code.render::<qrcode::render::svg::Color<'_>>()
            .min_dimensions(240, 240)
            .build(),
    )
}

/// This computer's name, so the Drift app can label it among several.
pub(crate) fn computer_name() -> String {
    let named = ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|name| !name.trim().is_empty()));

    named.or_else(host_name).unwrap_or_else(|| "Drift".into())
}

#[cfg(unix)]
fn host_name() -> Option<String> {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer's length is passed, and gethostname writes at most that many bytes.
    let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    let end = buffer.iter().position(|byte| *byte == 0)?;

    (result == 0 && end > 0).then(|| String::from_utf8_lossy(&buffer[..end]).into_owned())
}

#[cfg(not(unix))]
fn host_name() -> Option<String> {
    None
}

#[derive(Deserialize)]
struct RpcRequest {
    command: String,
    #[serde(default)]
    args: Value,
}

#[derive(Debug, thiserror::Error)]
enum RpcError {
    #[error("command is not available remotely")]
    NotAllowed,
    #[error("missing argument: {0}")]
    MissingArgument(String),
    #[error("invalid argument {key}: {source}")]
    InvalidArgument { key: String, source: serde_json::Error },
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Command(String),
}

impl From<String> for RpcError {
    fn from(error: String) -> Self {
        Self::Command(error)
    }
}

macro_rules! remote_commands {
    (
        |$app:ident, $args:ident, $store:ident|;
        $($name:literal => $handler:expr),+ $(,)?
    ) => {
        fn rpc_allowed(command: &str) -> bool {
            matches!(command, $($name)|+)
        }

        async fn dispatch_rpc(
            $app: &tauri::AppHandle,
            command: &str,
            $args: &Value,
        ) -> Result<Value, RpcError> {
            let $store = || $app.state::<Store>();
            match command {
                $($name => $handler,)+
                _ => Err(RpcError::NotAllowed),
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
            let error = Json(json!({ "error": "remote access credentials changed" }));
            return (StatusCode::UNAUTHORIZED, error).into_response();
        }
        result = dispatch_rpc(&app, &request.command, &request.args) => result,
    };
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => (StatusCode::BAD_REQUEST, Json(json!({ "error": error.to_string() }))).into_response(),
    }
}

remote_commands! {
    |app, args, store|;
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
        "provider_usage" => value(crate::usage_limits::provider_usage(app.state(), arg(args, "provider")?).await?),
        "store_workspaces" => value(commands::store_workspaces(store())?),
        "store_removed_workspaces" => {
            value(commands::store_removed_workspaces(store())?)
        },
        "store_add_workspace" => value(commands::store_add_workspace(
            store(),
            app.state(),
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
            value(commands::store_remove_workspace(store(), app.state(), arg(args, "id")?)?)
        },
        "store_expired_removed_workspaces" => value(
            commands::store_expired_removed_workspaces(store(), arg(args, "before")?)?,
        ),
        "store_forget_workspace" => {
            value(commands::store_forget_workspace(store(), app.state(), arg(args, "id")?)?)
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
        "prompt_snapshot" => value(prompts::prompt_snapshot(store())?),
        "prompt_save" => value(prompts::prompt_save(
            app.clone(),
            store(),
            arg(args, "key")?,
            arg(args, "value")?,
            optional(args, "original")?,
        )?),
        "prompt_reset" => value(prompts::prompt_reset(app.clone(), store(), arg(args, "key")?)?),
        "storage_stats" => value(commands::storage_stats(store(), app.state()).await?),
        "storage_prune" => value(commands::storage_prune(app.state()).await?),
        "storage_compact" => value(commands::storage_compact(app.state()).await?),
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
            value(ui_state::timeout::shell_timeout_snapshot(app.state())?)
        },
        "shell_timeout_update" => value(ui_state::timeout::shell_timeout_update(
            app.clone(),
            app.state(),
            store(),
            arg(args, "policy")?,
        )?),
}

fn arg<T: DeserializeOwned>(args: &Value, key: &str) -> Result<T, RpcError> {
    let value = args
        .get(key)
        .cloned()
        .ok_or_else(|| RpcError::MissingArgument(key.to_string()))?;

    serde_json::from_value(value).map_err(|source| RpcError::InvalidArgument {
        key: key.to_string(),
        source,
    })
}

fn optional<T: DeserializeOwned>(args: &Value, key: &str) -> Result<Option<T>, RpcError> {
    args.get(key)
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|source| RpcError::InvalidArgument {
            key: key.to_string(),
            source,
        })
}

fn value<T: Serialize>(value: T) -> Result<Value, RpcError> {
    Ok(serde_json::to_value(value)?)
}

pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();

    for index in 0..left.len().max(right.len()) {
        difference |= left.get(index).copied().unwrap_or(0) as usize ^ right.get(index).copied().unwrap_or(0) as usize;
    }

    difference == 0
}

#[cfg(test)]
#[path = "remote/tests.rs"]
mod tests;
