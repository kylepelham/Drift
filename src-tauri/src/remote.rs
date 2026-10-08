pub(crate) mod access_commands;
mod gateway;
mod transport;

use gateway::router;

use transport::{accept_loop, discovery_loop, local_ipv4};

use crate::remote_auth::{self, Auth, PendingLink};
use crate::remote_tls::Tls;
use crate::store::{RemoteDevice, Store};
use crate::{commands, config, editor, file_preview, prompts, ui_state, voice};
use axum::Json;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::Manager;
use tokio::sync::{Mutex as AsyncMutex, watch};
use tokio::task::JoinHandle;

pub(crate) const HTTP_PORT: u16 = 41718;
pub(crate) const DISCOVERY_PORT: u16 = 41717;
const MAX_CONCURRENT_PASSWORD_CHECKS: usize = 2;

#[derive(Debug, thiserror::Error)]
pub(crate) enum RemoteError {
    #[error("remote access is disabled")]
    Disabled,
    #[error("could not listen on port {HTTP_PORT}: {0}")]
    Listen(#[source] std::io::Error),
    #[error("could not listen for LAN discovery: {0}")]
    Discovery(#[source] std::io::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
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

impl RemoteAccess {
    pub(crate) fn load(store: &Store, data_dir: &Path) -> Result<Self, RemoteError> {
        let enabled = store.remote_access_enabled()?;
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

    pub(crate) async fn start(&self, app: tauri::AppHandle) -> Result<(), RemoteError> {
        let mut running = self.running.lock().await;
        if !self.config.lock().unwrap().enabled {
            return Err(RemoteError::Disabled);
        }
        if running.is_some() {
            return Ok(());
        }

        let http_listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, HTTP_PORT))
            .await
            .map_err(RemoteError::Listen)?;
        let discovery_socket = tokio::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT))
            .await
            .map_err(RemoteError::Discovery)?;
        discovery_socket.set_broadcast(true)?;

        let (shutdown, http_shutdown) = watch::channel(false);
        let discovery_shutdown = shutdown.subscribe();
        let http = tokio::spawn(accept_loop(
            http_listener,
            router(app.clone()),
            self.tls.clone(),
            http_shutdown,
        ));
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
            if tokio::time::timeout(std::time::Duration::from_millis(500), &mut running.discovery)
                .await
                .is_err()
            {
                running.discovery.abort();
            }
        }
    }

    pub(crate) fn stop_on_exit(&self) {
        if let Ok(running) = self.running.try_lock()
            && let Some(running) = running.as_ref()
        {
            let _ = running.shutdown.send(true);
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
            return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "remote access credentials changed" }))).into_response();
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
