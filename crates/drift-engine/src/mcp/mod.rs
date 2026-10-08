//! MCP servers: configured in the store, connected with rmcp, tools offered to the model.

mod catalog;
mod client;
mod connect;
mod error;
mod oauth;
mod registry;
mod resources;
mod response;
mod sse;
mod state;
mod tool;
mod view;
mod watch;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use rmcp::RoleClient;
use rmcp::model::ProtocolVersion;
use rmcp::service::RunningService;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;

use crate::platform::process::Tree;
use crate::store::Store;
use crate::tool::image::Image;
use client::lifecycle;
#[cfg(test)]
use connect::attempts;
pub(crate) use tool::{Given, wire_names};

pub use error::Error;
pub use oauth::{SignInError, forget as forget_sign_in, forget_if_moved, move_sign_in};
pub use tool::McpTool;
pub use view::{ServerConfigInput, ServerConfigView, ServerView};
#[cfg(test)]
use watch::after_loss;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        /// Where the server runs; Drift's own directory when unset.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_seconds: Option<u64>,
    },
    /// Streamable HTTP, the current transport.
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oauth: Option<OAuthClient>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_seconds: Option<u64>,
    },
    /// The older HTTP+SSE transport some servers still speak.
    Sse {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oauth: Option<OAuthClient>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_seconds: Option<u64>,
    },
}

/// An app registered with the server's authorization server beforehand, for servers that do not let Drift register itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OAuthClient {
    pub client_id: String,
    /// A confidential app's secret; never sent to clients.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// Scopes to ask for; none lets the server's own metadata decide.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
}

impl ServerConfig {
    /// Spoken to over HTTP, so a sign-in can apply.
    pub fn is_remote(&self) -> bool {
        !matches!(self, Self::Stdio { .. })
    }

    /// Where a remote server is, and the app it signs in as when it has one; `None` for stdio.
    pub fn remote(&self) -> Option<(&str, Option<&OAuthClient>)> {
        match self {
            Self::Http { url, oauth, .. } | Self::Sse { url, oauth, .. } => Some((url, oauth.as_ref())),
            Self::Stdio { .. } => None,
        }
    }

    /// How long one tool call may take before it fails; unset, it runs until done or stopped.
    pub fn timeout(&self) -> Option<Duration> {
        let (Self::Stdio { timeout_seconds, .. }
        | Self::Http { timeout_seconds, .. }
        | Self::Sse { timeout_seconds, .. }) = self;
        timeout_seconds.filter(|s| *s > 0).map(Duration::from_secs)
    }
}

/// A server whose saved definition does not parse: shown empty and failed, so the editor can save a new one over it.
fn unreadable(name: String) -> ServerStatus {
    let config = view::ServerConfigView::Stdio {
        command: String::new(),
        args: Vec::new(),
        env: Vec::new(),
        cwd: None,
        timeout_seconds: None,
    };
    ServerStatus {
        server: ServerView { name, config, enabled: false, read_only_trusted: false, updated_at: 0, workspaces: Vec::new() },
        state: State::Failed,
        error: Some("Its saved definition could not be read, probably because a newer Drift wrote it. Edit and save it again, or remove it.".into()),
        tools: Vec::new(),
        transport: Transport::Stdio,
        protocol: None,
        era: None,
        needs_sign_in: false,
        signed_in: false,
        unreadable: true,
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerRow {
    pub name: String,
    pub config: ServerConfig,
    pub enabled: bool,
    /// The config's identity, secrets included, so a connection knows which definition it serves; never sent to clients.
    #[serde(skip)]
    pub hash: String,
    pub updated_at: i64,
    /// The era it last answered in, so a reconnect skips the probe; a save forgets it.
    #[serde(skip)]
    pub era: Option<Era>,
    /// The user lets read-only agents (plan, explore) use the tools it marks read-only. A save that
    /// changes its definition, env and headers included, takes this back.
    pub read_only_trusted: bool,
    /// Workspaces that chose otherwise than `enabled`, which is the choice everywhere else.
    pub workspaces: Vec<WorkspaceChoice>,
}

/// A workspace's own choice for a server: on there though off elsewhere, or the reverse.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChoice {
    pub workspace_id: String,
    /// The workspace's folder as stored; matched against a connection's folder, never sent to clients.
    #[serde(skip)]
    pub path: String,
    pub enabled: bool,
}

impl ServerRow {
    /// Whether turns in `workspace` (a canonical folder) are offered this server.
    pub fn on_in(&self, workspace: &Path) -> bool {
        let chosen = self
            .workspaces
            .iter()
            .find(|choice| crate::tool::canonical(Path::new(&choice.path)) == workspace);
        chosen.map_or(self.enabled, |choice| choice.enabled)
    }

    /// On somewhere: by its switch, or in a workspace that turned it on.
    pub fn on_anywhere(&self) -> bool {
        self.enabled || self.workspaces.iter().any(|choice| choice.enabled)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Disabled,
    Disconnected,
    Connecting,
    Connected,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    #[serde(flatten)]
    pub server: ServerView,
    pub state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub tools: Vec<ToolInfo>,
    /// How the engine talks to it.
    pub transport: Transport,
    /// The MCP protocol version the server agreed to; absent until connected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// Whether that version is stateless (2026-07-28 on) or the legacy handshake; absent until connected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub era: Option<Era>,
    /// The server refused to connect until the user signs in (`POST /mcp/{name}/signin`).
    pub needs_sign_in: bool,
    /// A sign-in is kept for it (`DELETE /mcp/{name}/signin` forgets it).
    pub signed_in: bool,
    /// Its saved definition does not parse in this build: it can only be saved again or removed.
    pub unreadable: bool,
}

/// The wire a server is spoken to over; a stateless transport will join these.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Stdio,
    StreamableHttp,
    Sse,
}

impl Transport {
    fn of(config: &ServerConfig) -> Self {
        match config {
            ServerConfig::Stdio { .. } => Self::Stdio,
            ServerConfig::Http { .. } => Self::StreamableHttp,
            ServerConfig::Sse { .. } => Self::Sse,
        }
    }
}

/// Which generation of MCP a server speaks: 2026-07-28 and later, with no handshake and no session, or the `initialize` handshake before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Era {
    Stateless,
    Legacy,
}

impl Era {
    fn of(version: &ProtocolVersion) -> Self {
        if version.as_str() >= ProtocolVersion::V_2026_07_28.as_str() {
            Self::Stateless
        } else {
            Self::Legacy
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stateless => "stateless",
            Self::Legacy => "legacy",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Self::Stateless, Self::Legacy]
            .into_iter()
            .find(|era| era.as_str() == text)
    }

    fn other(self) -> Self {
        match self {
            Self::Stateless => Self::Legacy,
            Self::Legacy => Self::Stateless,
        }
    }
}

/// The client side of one connection: the workspace it was opened for, as a `file:` URI, is its only root.
#[derive(Clone)]
struct DriftClient {
    root: Option<String>,
}

type Client = RunningService<RoleClient, DriftClient>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub read_only: bool,
}

struct Live {
    service: Client,
    /// The era the server answered in.
    era: Era,
    transport: Transport,
    listing: Mutex<Listing>,
    /// What the server said at initialize about using it; goes in the system prompt beside its tools.
    instructions: Option<String>,
    /// Its prompts, offered to the user as `server:prompt` slash commands.
    prompts: Vec<rmcp::model::Prompt>,
    /// Whether it serves resources, which the `mcp_resources` tools list and read.
    resources: bool,
    /// How long one tool call may take, from its config.
    timeout: Option<Duration>,
    /// The definition it was opened from; a client of another definition never stands in for this one.
    hash: String,
    since: Instant,
    /// A stdio server's process tree; it dies with the last handle to this connection.
    tree: Option<Tree>,
}

/// A server's tools as last listed, and when that list goes stale by its `ttlMs`; a server that gives none is listed once.
struct Listing {
    tools: Vec<rmcp::model::Tool>,
    stale_at: Option<Instant>,
}

impl Listing {
    fn new(tools: Vec<rmcp::model::Tool>, ttl: Option<Duration>) -> Self {
        Self {
            tools,
            stale_at: ttl.map(|ttl| Instant::now() + ttl),
        }
    }
}

/// Why a connect failed, and whether the server asked for a sign-in (a 401 or 403), as rmcp's typed error says.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("{message}")]
pub(super) struct Failure {
    message: String,
    needs_sign_in: bool,
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self {
            message,
            needs_sign_in: false,
        }
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        Self::from(message.to_string())
    }
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self::from(error.to_string())
    }
}

impl From<rmcp::service::ClientInitializeError> for Failure {
    fn from(error: rmcp::service::ClientInitializeError) -> Self {
        Self {
            needs_sign_in: error.is_authorization_required(),
            message: error.to_string(),
        }
    }
}

/// What a server not connected is doing, as its status shows it.
#[derive(Clone, Debug)]
struct Transient {
    state: State,
    error: Option<String>,
    needs_sign_in: bool,
}

impl Transient {
    fn new(state: State, error: Option<String>) -> Self {
        Self {
            state,
            error,
            needs_sign_in: false,
        }
    }
}

/// A call that never got an answer because the connection ended, as opposed to one the server refused.
enum CallError {
    Lost,
    Failed(String),
}

/// One server's connections from enable to disable or remove, shared by every tool made from it.
#[derive(Default)]
struct Slot {
    current: Mutex<Option<Arc<Live>>>,
    /// Every client it has served, so a disable reaches the ones running turns still hold.
    served: Mutex<Vec<Weak<Live>>>,
    /// Cancelled by disable and remove: tools made from it refuse, and calls under way end.
    closed: CancellationToken,
    published: tokio::sync::Notify,
    /// When a turn, a call or the workspace last used it; a workspace's connection idle for `IDLE` stops.
    used: Mutex<Option<Instant>>,
}

/// How often a connected server's transport is checked for having closed by itself.
#[cfg(not(test))]
const WATCH_INTERVAL: Duration = Duration::from_secs(1);
#[cfg(test)]
const WATCH_INTERVAL: Duration = Duration::from_millis(50);
/// Reconnect backoff: the first wait, doubled each failure up to the last.
#[cfg(not(test))]
const FIRST_RETRY: Duration = Duration::from_millis(500);
#[cfg(test)]
const FIRST_RETRY: Duration = Duration::from_millis(20);
const MAX_RETRY: Duration = Duration::from_secs(30);
/// How long a server gets to answer `initialize`, and then again to answer `tools/list`.
#[cfg(not(test))]
const STEP_LIMIT: Duration = Duration::from_secs(30);
// Room for a cold `node` on a fresh CI runner, still short enough for the timeout tests.
#[cfg(test)]
const STEP_LIMIT: Duration = Duration::from_secs(5);
/// A connection that held this long starts its reconnects from the first wait again.
#[cfg(not(test))]
const STABLE: Duration = Duration::from_secs(60);
#[cfg(test)]
const STABLE: Duration = Duration::from_millis(500);
/// After a re-list fails, how long before a turn tries again, so a server that stopped answering does not hold up every turn.
const RELIST_BACKOFF: Duration = Duration::from_secs(30);
/// How long rmcp waits for an HTTP server to answer the `server/discover` probe before falling back to `initialize`; it is fixed there.
const PROBE_WAIT: Duration = Duration::from_secs(10);
/// How long a turn being planned waits for connects already under way, so its tools are not briefly missing.
pub const READY_WAIT: Duration = Duration::from_secs(2);
/// Why a stdio server cannot connect with no workspace to run in.
pub const NEEDS_WORKSPACE: &str = "a stdio server runs in a workspace; open one and connect it there";
/// How long a stdio server of a workspace no client has open may go unused before it stops; the next use starts it again.
pub const IDLE: Duration = Duration::from_secs(5 * 60);
/// How often idle servers are looked for.
const IDLE_SWEEP: Duration = Duration::from_secs(60);
/// How long a read-only call cut off by a lost connection waits for the reconnect before giving up.
#[cfg(not(test))]
const REPLACEMENT_WAIT: Duration = Duration::from_secs(10);
#[cfg(test)]
const REPLACEMENT_WAIT: Duration = Duration::from_secs(3);

/// A connect in flight. A newer connect, or any change to the server, cancels it.
#[derive(Clone)]
struct Attempt {
    id: u64,
    generation: u64,
    cancel: CancellationToken,
}

struct Connecting {
    key: Key,
    attempt: Attempt,
}

struct EndConnections<'a> {
    name: &'a str,
    ending: Ending,
}

pub struct WorkspaceServer<'a> {
    pub name: &'a str,
    pub workspace: &'a Path,
}

/// One connection of a server: a remote server has one, shared (`workspace` none); a stdio server
/// has one per workspace that used it, run in that workspace and naming it as the server's root, as
/// opencode runs one per project, and never a shared one.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Key {
    pub server: String,
    pub workspace: Option<PathBuf>,
}

impl Key {
    pub(crate) fn shared(server: &str) -> Self {
        Self {
            server: server.into(),
            workspace: None,
        }
    }

    fn of(server: &str, workspace: Option<&Path>) -> Self {
        Self {
            server: server.into(),
            workspace: workspace.map(Path::to_path_buf),
        }
    }
}

#[derive(Default)]
struct Slots {
    servers: HashMap<Key, Arc<Slot>>,
    transient: HashMap<Key, Transient>,
    /// Bumped by every save, disable, disconnect and remove; a connect from an older one publishes nothing.
    generation: HashMap<Key, u64>,
    attempts: HashMap<Key, Attempt>,
    /// Servers the user disconnected: the engine's own starts leave them alone until the user connects one again.
    held: std::collections::HashSet<String>,
}

/// The servers turns in a workspace are offered; with no store (a bare registry) or no workspace, every one on anywhere.
struct Shown(Option<std::collections::HashSet<String>>);

impl Shown {
    fn of(store: Option<&Store>, workspace: Option<&Path>) -> Self {
        let Some(rows) = store.and_then(|store| store.mcp_servers().ok()) else {
            return Self(None);
        };
        Self(Some(
            rows.into_iter()
                .filter(|row| workspace.map_or(row.on_anywhere(), |workspace| row.on_in(workspace)))
                .map(|row| row.name)
                .collect(),
        ))
    }

    fn allows(&self, server: &str) -> bool {
        self.0.as_ref().is_none_or(|names| names.contains(server))
    }
}

/// Who asked for a connect. Only the user's replaces one in flight; the engine's own never cancel anything.
#[derive(Clone, Copy, PartialEq)]
enum Start {
    User,
    Startup,
    /// After a lost connection, and only while the server is still at this generation.
    Reconnect(u64),
}

/// What a change does to the server's slot: a save or disconnect keeps it, a disable or remove ends it.
#[derive(Clone, Copy, PartialEq)]
enum Ending {
    Keep,
    Close,
}

#[derive(Default)]
pub struct Servers {
    /// The lifecycle lock: row writes, generation bumps, connect starts and connect results all go through it.
    slots: Mutex<Slots>,
    next_attempt: AtomicU64,
    /// Signalled whenever a connect attempt ends, for turns waiting on the catalog.
    settled: tokio::sync::Notify,
    /// Where remote servers' sign-ins are kept; none in a bare test registry.
    sign_ins: Option<Arc<crate::llm::credentials::Credentials>>,
    /// The workspace each connected client (window or device) has open, by socket; their servers never stop for idleness.
    open: Mutex<HashMap<u64, PathBuf>>,
    /// Where each workspace's choices are read; none in a bare test registry, which offers every server.
    store: Option<Arc<Store>>,
}

enum Watch {
    Holding,
    Gone,
    Lost { generation: u64, lived: Duration },
}

/// What a call returned: its text, whether the server called it an error, and any images for the model.
pub(super) struct Answer {
    pub text: String,
    pub is_error: bool,
    pub images: Vec<Image>,
}

#[cfg(test)]
mod tests;
