//! MCP servers: configured in the store, connected with rmcp, tools offered to the model.

mod oauth;
mod resources;
mod sse;
mod tool;
mod view;

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use rmcp::model::{CallToolRequestParams, ClientCapabilities, ClientConfig, ContentBlock, Implementation, ProtocolVersion};
use rmcp::service::{ClientLifecycleMode, ClientServiceExt, RunningService, ServiceError};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::RoleClient;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;

use crate::event::{Event, Hub};
use crate::platform::process::Tree;
use crate::store::Store;
use crate::tool::image::Image;
pub(crate) use tool::{wire_names, Given};

pub use oauth::{forget as forget_sign_in, forget_if_moved, move_sign_in};
pub use tool::McpTool;
pub use view::{ServerConfigInput, ServerConfigView, ServerView};

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
        let (Self::Stdio { timeout_seconds, .. } | Self::Http { timeout_seconds, .. } | Self::Sse { timeout_seconds, .. }) = self;
        timeout_seconds.filter(|s| *s > 0).map(Duration::from_secs)
    }
}

/// A server whose saved definition does not parse: shown empty and failed, so the editor can save a new one over it.
fn unreadable(name: String) -> ServerStatus {
    let config = view::ServerConfigView::Stdio { command: String::new(), args: Vec::new(), env: Vec::new(), cwd: None, timeout_seconds: None };
    ServerStatus {
        server: ServerView { name, config, enabled: false, read_only_trusted: false, updated_at: 0 },
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
        [Self::Stateless, Self::Legacy].into_iter().find(|era| era.as_str() == text)
    }

    fn other(self) -> Self {
        match self {
            Self::Stateless => Self::Legacy,
            Self::Legacy => Self::Stateless,
        }
    }
}

/// How a connect begins: a probe when the server's era is unknown, else straight to the one it speaks.
fn lifecycle(known: Option<Era>) -> ClientLifecycleMode {
    match known {
        None => ClientLifecycleMode::Auto { preferred_versions: vec![ProtocolVersion::V_2026_07_28], legacy_version: Some(ProtocolVersion::V_2025_11_25) },
        Some(Era::Stateless) => ClientLifecycleMode::Discover { preferred_versions: vec![ProtocolVersion::V_2026_07_28] },
        Some(Era::Legacy) => ClientLifecycleMode::Initialize,
    }
}

/// Drift as an MCP client: no sampling or elicitation, so a server asking for them is declined; a
/// connection for a workspace offers roots and names that workspace, as opencode does; the
/// handshake offers 2025-11-25.
#[allow(deprecated)]
fn client_info(roots: bool) -> ClientConfig {
    let mut capabilities = ClientCapabilities::default();
    if roots {
        capabilities.roots = Some(rmcp::model::RootsCapabilities::default());
    }
    ClientConfig::new(capabilities, Implementation::new("Drift", env!("CARGO_PKG_VERSION"))).with_protocol_version(ProtocolVersion::V_2025_11_25)
}

/// The client side of one connection: the workspace it was opened for, as a `file:` URI, is its only root.
#[derive(Clone)]
struct DriftClient {
    root: Option<String>,
}

impl DriftClient {
    fn rooted(workspace: Option<&Path>) -> Self {
        Self { root: workspace.map(|path| reqwest::Url::from_directory_path(path).map_or_else(|()| path.to_string_lossy().into_owned(), String::from)) }
    }
}

#[allow(deprecated)]
impl rmcp::ClientHandler for DriftClient {
    fn get_info(&self) -> ClientConfig {
        client_info(self.root.is_some())
    }

    async fn list_roots(&self, _context: rmcp::service::RequestContext<RoleClient>) -> Result<rmcp::model::ListRootsResult, rmcp::ErrorData> {
        Ok(rmcp::model::ListRootsResult::new(self.root.iter().map(rmcp::model::Root::new).collect()))
    }
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
        Self { tools, stale_at: ttl.map(|ttl| Instant::now() + ttl) }
    }
}

/// Why a connect failed, and whether the server asked for a sign-in (a 401 or 403), as rmcp's typed error says.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Failure {
    message: String,
    needs_sign_in: bool,
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self { message, needs_sign_in: false }
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        Self::from(message.to_string())
    }
}

impl From<rmcp::service::ClientInitializeError> for Failure {
    fn from(error: rmcp::service::ClientInitializeError) -> Self {
        Self { needs_sign_in: error.is_authorization_required(), message: error.to_string() }
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
        Self { state, error, needs_sign_in: false }
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

impl Slot {
    fn current(&self) -> Option<Arc<Live>> {
        self.current.lock().unwrap().clone()
    }

    fn touch(&self) {
        *self.used.lock().unwrap() = Some(Instant::now());
    }

    fn idle_for(&self, limit: Duration) -> bool {
        self.used.lock().unwrap().is_none_or(|used| used.elapsed() >= limit)
    }

    fn publish(&self, live: Arc<Live>) {
        let mut served = self.served.lock().unwrap();
        served.retain(|client| client.strong_count() > 0);
        served.push(Arc::downgrade(&live));
        drop(served);
        *self.current.lock().unwrap() = Some(live);
        self.touch();
        self.published.notify_waiters();
    }

    fn take(&self) -> Option<Arc<Live>> {
        self.current.lock().unwrap().take()
    }

    fn close(&self) {
        self.closed.cancel();
        self.take();
        let served: Vec<_> = self.served.lock().unwrap().drain(..).collect();
        for live in served.iter().filter_map(Weak::upgrade) {
            live.close();
        }
    }

    fn is_closed(&self) -> bool {
        self.closed.is_cancelled()
    }

    async fn closing(&self) {
        self.closed.cancelled().await;
    }

    /// The client a tool opened as `pinned` calls now: the current one when it serves the same definition.
    fn client_for(&self, pinned: &Arc<Live>) -> Arc<Live> {
        self.touch();
        self.current().filter(|current| current.hash == pinned.hash && current.is_open()).unwrap_or_else(|| pinned.clone())
    }

    /// Waits, at most `limit`, for a client of the same definition to take over from `lost`.
    async fn replacement(&self, lost: &Arc<Live>, limit: Duration) -> Option<Arc<Live>> {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let published = self.published.notified();
            if let Some(next) = self.current().filter(|next| next.hash == lost.hash && !Arc::ptr_eq(next, lost) && next.is_open()) {
                return Some(next);
            }
            tokio::select! {
                () = published => {}
                () = self.closed.cancelled() => return None,
                () = tokio::time::sleep_until(deadline) => return None,
            }
        }
    }
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
#[cfg(test)]
const STEP_LIMIT: Duration = Duration::from_millis(1500);
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
        Self { server: server.into(), workspace: None }
    }

    fn of(server: &str, workspace: Option<&Path>) -> Self {
        Self { server: server.into(), workspace: workspace.map(Path::to_path_buf) }
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

impl Slots {
    fn generation_of(&self, key: &Key) -> u64 {
        self.generation.get(key).copied().unwrap_or(0)
    }

    fn is_current(&self, key: &Key, attempt: &Attempt) -> bool {
        self.attempts.get(key).is_some_and(|a| a.id == attempt.id) && self.generation_of(key) == attempt.generation
    }

    fn live(&self, key: &Key) -> Option<Arc<Live>> {
        self.servers.get(key).and_then(|slot| slot.current())
    }

    /// Every connection of `server` this registry holds: live, kept for a reconnect, connecting or failed.
    fn keys_of(&self, server: &str) -> Vec<Key> {
        let mut keys: Vec<Key> = self.servers.keys().chain(self.attempts.keys()).chain(self.transient.keys()).filter(|key| key.server == server).cloned().collect();
        keys.sort();
        keys.dedup();
        keys
    }

    /// The server's live connection for `workspace`: its own there, else a remote server's shared one;
    /// a stdio server is never used from anywhere but the workspace it runs in. Finding it counts as a use.
    fn live_for(&self, server: &str, workspace: Option<&Path>) -> Option<(Key, Arc<Live>)> {
        let own = workspace.map(|workspace| Key::of(server, Some(workspace)));
        own.into_iter().chain([Key::shared(server)]).find_map(|key| {
            let slot = self.servers.get(&key)?;
            let live = slot.current().filter(|live| key.workspace.is_some() || live.transport != Transport::Stdio)?;
            slot.touch();
            Some((key, live))
        })
    }

    /// Each server's live connection for `workspace`, by name.
    fn lives_for(&self, workspace: Option<&Path>) -> Vec<(Key, Arc<Slot>, Arc<Live>)> {
        let mut names: Vec<&String> = self.servers.keys().map(|key| &key.server).collect();
        names.sort();
        names.dedup();
        names.into_iter().filter_map(|name| {
            let (key, live) = self.live_for(name, workspace)?;
            Some((key.clone(), self.servers.get(&key)?.clone(), live))
        }).collect()
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
}

enum Watch {
    Holding,
    Gone,
    Lost { generation: u64, lived: Duration },
}

impl Servers {
    pub fn new(sign_ins: Arc<crate::llm::credentials::Credentials>) -> Self {
        Self { sign_ins: Some(sign_ins), ..Self::default() }
    }

    fn lock(&self) -> MutexGuard<'_, Slots> {
        self.slots.lock().unwrap()
    }

    /// Every saved server, those this build cannot read included, as failed rows to save again or remove.
    pub fn statuses(&self, store: &Store) -> rusqlite::Result<Vec<ServerStatus>> {
        let mut statuses: Vec<ServerStatus> = store.mcp_servers()?.into_iter().map(|row| self.status_of(row)).collect();
        statuses.extend(store.unreadable_mcp_servers()?.into_iter().map(unreadable));
        statuses.sort_by(|a, b| a.server.name.cmp(&b.server.name));
        Ok(statuses)
    }

    /// The server as one row, over all its connections: connected if any is, else connecting, else its last failure.
    pub fn status_of(&self, row: ServerRow) -> ServerStatus {
        let slots = self.lock();
        let keys = slots.keys_of(&row.name);
        let live = keys.iter().find_map(|key| slots.live(key));
        let Transient { state, error, needs_sign_in } = if !row.enabled {
            Transient::new(State::Disabled, None)
        } else if live.is_some() {
            Transient::new(State::Connected, None)
        } else {
            let states: Vec<&Transient> = keys.iter().filter_map(|key| slots.transient.get(key)).collect();
            states.iter().find(|transient| transient.state == State::Connecting).or(states.first()).map(|transient| (*transient).clone()).unwrap_or(Transient::new(State::Disconnected, None))
        };
        let protocol = live.as_ref().and_then(|live| live.service.peer_info()).map(|info| info.protocol_version.to_string());
        let era = live.as_ref().map(|live| live.era);
        let tools = live.map(|live| live.tools().iter().map(tool_info).collect()).unwrap_or_default();
        let needs_sign_in = needs_sign_in && state == State::Failed;
        let signed_in = row.config.is_remote() && self.sign_ins.as_ref().is_some_and(|store| oauth::has_sign_in(store, &row.name));
        ServerStatus { transport: Transport::of(&row.config), protocol, era, needs_sign_in, signed_in, server: ServerView::of(&row), state, error, tools, unreadable: false }
    }

    /// A sign-in that did not finish: the server, unless it connected meanwhile, shows why and still asks to sign in.
    pub(super) fn sign_in_failed(&self, name: &str, store: &Store, hub: &Hub, why: &str) {
        let key = Key::shared(name);
        let mut slots = self.lock();
        if slots.live(&key).is_some() || slots.attempts.contains_key(&key) {
            return;
        }
        slots.transient.insert(key, Transient { state: State::Failed, error: Some(format!("Sign-in did not finish: {why}")), needs_sign_in: true });
        drop(slots);
        if let Ok(Some(row)) = store.mcp_server(name) {
            hub.publish(Event::McpUpdated { server: self.status_of(row) });
        }
    }

    /// Connects `key` as its server's row stands now.
    async fn connect(&self, key: &Key, store: &Store, hub: &Hub, start: Start) -> Result<Arc<Live>, String> {
        let (row, attempt) = self.begin(key, store, start)?;
        self.complete(row, key.clone(), attempt, store, hub).await
    }

    /// The rest of a connect `begin` started: opens the server and publishes what it opened.
    async fn complete(&self, row: ServerRow, key: Key, attempt: Attempt, store: &Store, hub: &Hub) -> Result<Arc<Live>, String> {
        let _settle = Settle { servers: self, hub, row: &row, key: &key, id: attempt.id };
        hub.publish(Event::McpUpdated { server: self.status_of(row.clone()) });
        let sign_in = SignIn { server: &row.name, credentials: self.sign_ins.as_ref() };
        let opened = tokio::select! {
            opened = open(&row.config, row.hash.clone(), sign_in, row.era, key.workspace.as_deref()) => opened,
            () = attempt.cancel.cancelled() => Err(Failure::from("server definition changed during connect".to_string())),
        };
        let finished = self.finish(&row, &key, hub, &attempt, opened);
        remember_era(store, &row, finished.as_ref().ok().map(|live| live.era));
        finished
    }

    /// Reads the row and its generation together, so a save cannot slip between them. The engine's
    /// own connects never retry one that failed; the user's connect does.
    fn begin(&self, key: &Key, store: &Store, start: Start) -> Result<(ServerRow, Attempt), String> {
        let mut slots = self.lock();
        let row = store.mcp_server(&key.server).map_err(|e| e.to_string())?.ok_or("no such server")?;
        if row.config.is_remote() != key.workspace.is_none() {
            return Err(if key.workspace.is_none() { NEEDS_WORKSPACE } else { "a remote server has one shared connection" }.into());
        }
        let generation = slots.generation_of(key);
        if matches!(start, Start::Reconnect(expected) if expected != generation) {
            return Err("server definition changed".into());
        }
        if start != Start::User && (slots.attempts.contains_key(key) || slots.live(key).is_some()) {
            return Err("already connected or connecting".into());
        }
        if start == Start::Startup && slots.transient.get(key).is_some_and(|transient| transient.state == State::Failed) {
            return Err("it failed to connect; connect it again in Settings".into());
        }
        match start {
            Start::User => drop(slots.held.remove(&key.server)),
            _ if slots.held.contains(&key.server) => return Err("the user disconnected it".into()),
            _ => {}
        }
        if !row.enabled {
            return Err("server is disabled".into());
        }
        let attempt = Attempt { id: self.next_attempt.fetch_add(1, Ordering::Relaxed), generation, cancel: CancellationToken::new() };
        if let Some(earlier) = slots.attempts.insert(key.clone(), attempt.clone()) {
            earlier.cancel.cancel();
        }
        slots.transient.insert(key.clone(), Transient::new(State::Connecting, None));
        Ok((row, attempt))
    }

    /// Publishes what the attempt opened, unless it was overtaken; then what it opened is dropped, killing it.
    fn finish(&self, row: &ServerRow, key: &Key, hub: &Hub, attempt: &Attempt, opened: Result<Live, Failure>) -> Result<Arc<Live>, String> {
        let mut slots = self.lock();
        if !slots.is_current(key, attempt) {
            return Err("server definition changed during connect".into());
        }
        slots.attempts.remove(key);
        let result = match opened {
            Ok(live) => {
                let live = Arc::new(live);
                slots.servers.entry(key.clone()).or_default().publish(live.clone());
                slots.transient.remove(key);
                Ok(live)
            }
            Err(Failure { message, needs_sign_in }) => {
                slots.transient.insert(key.clone(), Transient { state: State::Failed, error: Some(message.clone()), needs_sign_in });
                Err(message)
            }
        };
        drop(slots);
        hub.publish(Event::McpUpdated { server: self.status_of(row.clone()) });
        result
    }

    /// Writes the server's row and, in the same step, ends every connection of it and any connect in flight. A write that refuses changes nothing.
    fn detach<R, E>(&self, name: &str, store: &Store, ending: Ending, write: impl FnOnce(&Store) -> Result<R, E>) -> Result<(Vec<Arc<Live>>, R), E> {
        let mut slots = self.lock();
        let written = write(store)?;
        let mut lives = Vec::new();
        for key in slots.keys_of(name) {
            *slots.generation.entry(key.clone()).or_default() += 1;
            if let Some(attempt) = slots.attempts.remove(&key) {
                attempt.cancel.cancel();
            }
            slots.transient.remove(&key);
            let slot = if ending == Ending::Close { slots.servers.remove(&key) } else { slots.servers.get(&key).cloned() };
            lives.extend(slot.as_ref().and_then(|slot| slot.take()));
            if ending == Ending::Close {
                slot.inspect(|slot| slot.close());
            }
        }
        Ok((lives, written))
    }

    /// A save: the write and the end of the old connections are one step; running turns keep their client.
    pub async fn change<R, E>(&self, name: &str, store: &Store, hub: &Hub, write: impl FnOnce(&Store) -> Result<R, E>) -> Result<R, E> {
        let (lives, written) = self.detach(name, store, Ending::Keep, write)?;
        self.retire(name, store, hub, lives).await;
        Ok(written)
    }

    /// A disable, remove or rename: as [`Self::change`], and every client the server served is closed, running turns' too.
    pub async fn close<R, E>(&self, name: &str, store: &Store, hub: &Hub, write: impl FnOnce(&Store) -> Result<R, E>) -> Result<R, E> {
        let (lives, written) = self.detach(name, store, Ending::Close, write)?;
        self.retire(name, store, hub, lives).await;
        Ok(written)
    }

    /// The user's disconnect: every connection of the server ends, and none starts again by itself until the user connects it.
    pub async fn disconnect(&self, name: &str, store: &Store, hub: &Hub) -> bool {
        self.lock().held.insert(name.into());
        let Ok((lives, ())) = self.detach(name, store, Ending::Keep, |_| Ok::<_, rusqlite::Error>(())) else { return false };
        let was_live = !lives.is_empty();
        self.retire(name, store, hub, lives).await;
        was_live
    }

    /// Ends one connection (a workspace removed or left idle); the next turn there starts it again. A
    /// running turn keeps the client it holds until it lets go, and a connect under way is left alone.
    fn stop(&self, key: &Key, store: &Store, hub: &Hub) {
        let mut slots = self.lock();
        if slots.attempts.contains_key(key) {
            return;
        }
        *slots.generation.entry(key.clone()).or_default() += 1;
        slots.transient.remove(key);
        let live = slots.servers.remove(key).and_then(|slot| slot.take());
        drop(slots);
        drop(live);
        if let Ok(Some(row)) = store.mcp_server(&key.server) {
            hub.publish(Event::McpUpdated { server: self.status_of(row) });
        }
    }

    /// Stops every connection running in `workspace`.
    pub fn stop_workspace(&self, workspace: &Path, store: &Store, hub: &Hub) {
        let keys: Vec<Key> = self.lock().servers.keys().filter(|key| key.workspace.as_deref() == Some(workspace)).cloned().collect();
        for key in keys {
            self.stop(&key, store, hub);
        }
    }

    /// What workspace a client's socket has open now, or none; it is forgotten when the socket closes.
    pub fn set_open(&self, socket: u64, workspace: Option<PathBuf>) {
        let mut open = self.open.lock().unwrap();
        match workspace {
            Some(workspace) => open.insert(socket, workspace),
            None => open.remove(&socket),
        };
    }

    pub(crate) fn is_open(&self, workspace: &Path) -> bool {
        self.open.lock().unwrap().values().any(|open| open == workspace)
    }

    /// Stops each workspace's connection that no client has open and nothing has used for `limit`,
    /// as opencode keeps a project's servers while the project is open; a remote server's shared one stays.
    pub fn stop_idle(&self, limit: Duration, store: &Store, hub: &Hub) {
        let idle: Vec<Key> = self.lock().servers.iter().filter(|(key, slot)| key.workspace.as_deref().is_some_and(|workspace| !self.is_open(workspace)) && slot.idle_for(limit)).map(|(key, _)| key.clone()).collect();
        for key in idle {
            self.stop(&key, store, hub);
        }
    }

    /// Closes connections nothing else holds; one a running turn still holds closes when that turn lets go.
    async fn retire(&self, name: &str, store: &Store, hub: &Hub, lives: Vec<Arc<Live>>) {
        for live in lives.into_iter().filter_map(|live| Arc::try_unwrap(live).ok()) {
            let _ = live.service.cancel().await;
        }
        self.settled.notify_waiters();
        if let Ok(Some(row)) = store.mcp_server(name) {
            hub.publish(Event::McpUpdated { server: self.status_of(row) });
        }
    }

    fn generation_of(&self, key: &Key) -> u64 {
        self.lock().generation_of(key)
    }

    fn is_live(&self, key: &Key) -> bool {
        self.lock().live(key).is_some()
    }

    /// Whether any connect is in flight.
    pub fn connecting(&self) -> bool {
        !self.lock().attempts.is_empty()
    }

    /// Waits, at most `limit`, until no connect is in flight.
    pub async fn wait_ready(&self, limit: Duration) {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let settled = self.settled.notified();
            if !self.connecting() {
                return;
            }
            tokio::select! {
                () = settled => {}
                () = tokio::time::sleep_until(deadline) => return,
            }
        }
    }

    /// Whether `live` is still the connection at `key`; one whose transport closed by itself is dropped here.
    fn check(&self, key: &Key, live: &Weak<Live>) -> Watch {
        let mut slots = self.lock();
        let Some(slot) = slots.servers.get(key).cloned() else { return Watch::Gone };
        let Some(current) = slot.current().filter(|current| Arc::downgrade(current).ptr_eq(live)) else { return Watch::Gone };
        if current.is_open() {
            return Watch::Holding;
        }
        slot.take();
        slots.transient.insert(key.clone(), Transient::new(State::Connecting, Some("the connection closed; reconnecting".into())));
        Watch::Lost { generation: slots.generation_of(key), lived: current.since.elapsed() }
    }

    /// Every tool of every server connected for `workspace` (its own stdio connection, else the
    /// shared one), named `server_tool` so the model can tell them apart; the names are kept in
    /// `store`, so a tool keeps its name for good ([`wire_names`]).
    pub fn tools(&self, store: &Store, workspace: Option<&Path>) -> Vec<Arc<dyn crate::tool::Tool>> {
        let lives = self.lock().lives_for(workspace);
        let resources = lives.iter().any(|(_, _, live)| live.resources);
        let listed: Vec<(Key, rmcp::model::Tool, Arc<Live>, Arc<Slot>)> =
            lives.into_iter().flat_map(|(key, slot, live)| live.tools().into_iter().map(move |tool| (key.clone(), tool, live.clone(), slot.clone()))).collect();
        let pairs: Vec<(&str, &str)> = listed.iter().map(|(key, tool, ..)| (key.server.as_str(), tool.name.as_ref())).collect();
        let names = store.name_mcp_tools(&pairs).unwrap_or_else(|_| wire_names(&Given::new(), &pairs));
        let mut tools: Vec<Arc<dyn crate::tool::Tool>> =
            listed.iter().zip(names).map(|((key, tool, live, slot), name)| Arc::new(McpTool::new(key, tool.clone(), live.clone(), slot.clone(), name)) as Arc<dyn crate::tool::Tool>).collect();
        if resources {
            tools.push(Arc::new(resources::ListResources));
            tools.push(Arc::new(resources::ReadResource));
        }
        tools
    }

    /// Lists again the tools of each connection whose last list has gone stale by its `ttlMs`, one short request each, so a turn sees what changed.
    pub async fn refresh_stale(&self, store: &Store, hub: &Hub) {
        let due: Vec<(String, Arc<Live>)> = self.lock().servers.iter().filter_map(|(key, slot)| Some((key.server.clone(), slot.current()?))).filter(|(_, live)| live.listing_due()).collect();
        let relist = |name: String, live: Arc<Live>| async move {
            match tokio::time::timeout(READY_WAIT, list_tools(&live.service)).await {
                Ok(Ok((tools, ttl))) => live.relisted(tools, ttl).then_some(name),
                _ => {
                    live.relist_failed();
                    None
                }
            }
        };
        let changed = futures_util::future::join_all(due.into_iter().map(|(name, live)| relist(name, live))).await;
        for name in changed.into_iter().flatten() {
            if let Ok(Some(row)) = store.mcp_server(&name) {
                hub.publish(Event::McpUpdated { server: self.status_of(row) });
            }
        }
    }

    /// The own instructions of each server connected for `workspace`, by server name.
    pub fn instructions(&self, workspace: Option<&Path>) -> Vec<(String, String)> {
        self.lock().lives_for(workspace).into_iter().filter_map(|(key, _, live)| Some((key.server, live.instructions.clone()?))).collect()
    }

    fn live(&self, server: &str, workspace: Option<&Path>) -> Result<Arc<Live>, String> {
        self.lock().live_for(server, workspace).map(|(_, live)| live).ok_or_else(|| format!("the {server} MCP server is not connected"))
    }

    /// Servers connected for `workspace` that serve resources, by name.
    pub fn with_resources(&self, workspace: Option<&Path>) -> Vec<String> {
        self.lock().lives_for(workspace).into_iter().filter(|(_, _, live)| live.resources).map(|(key, _, _)| key.server).collect()
    }

    /// The prompts of every server connected for `workspace`, as `(server, prompt)`.
    pub fn prompts(&self, workspace: Option<&Path>) -> Vec<(String, rmcp::model::Prompt)> {
        let mut all: Vec<(String, rmcp::model::Prompt)> = self.lock().lives_for(workspace).into_iter().flat_map(|(key, _, live)| live.prompts.iter().map(|prompt| (key.server.clone(), prompt.clone())).collect::<Vec<_>>()).collect();
        all.sort_by(|a, b| (&a.0, &a.1.name).cmp(&(&b.0, &b.1.name)));
        all
    }

    pub async fn list_resources(&self, server: &str, workspace: Option<&Path>) -> Result<Vec<rmcp::model::Resource>, String> {
        let live = self.live(server, workspace)?;
        within("list its resources", async { live.service.list_all_resources().await.map_err(|e| e.to_string()) }).await
    }

    /// A resource's contents: text inline, images and PDFs as files, other binaries named.
    pub(crate) async fn read_resource(&self, server: &str, workspace: Option<&Path>, uri: &str) -> Result<Answer, String> {
        let live = self.live(server, workspace)?;
        let read = within("read the resource", async { live.service.read_resource(rmcp::model::ReadResourceRequestParams::new(uri)).await.map_err(|e| e.to_string()) }).await?;
        let mut answer = Answer { text: String::new(), is_error: false, images: Vec::new() };
        let mut lines = Vec::new();
        for content in &read.contents {
            take_resource(content, &mut answer.images, &mut lines);
        }
        answer.text = lines.join("\n");
        Ok(answer)
    }

    /// The prompts of every server connected for `workspace` as slash commands named `server:prompt`.
    pub fn prompt_commands(&self, workspace: Option<&Path>) -> Vec<crate::config::Command> {
        self.prompts(workspace)
            .into_iter()
            .map(|(server, prompt)| {
                let description = prompt.description.clone().unwrap_or_else(|| format!("A prompt from the {server} MCP server"));
                let mut command = crate::config::Command::new(format!("{server}:{}", prompt.name), description, String::new());
                command.arguments = prompt.arguments.iter().flatten().map(|argument| argument.name.clone()).collect();
                command.server = Some(server);
                command
            })
            .collect()
    }
    /// A prompt filled with `arguments`, as the text of its messages.
    pub async fn get_prompt(&self, server: &str, workspace: Option<&Path>, name: &str, arguments: serde_json::Map<String, serde_json::Value>) -> Result<String, String> {
        let live = self.live(server, workspace)?;
        let mut params = rmcp::model::GetPromptRequestParams::new(name);
        params.arguments = Some(arguments);
        let got = within("fill the prompt", async { live.service.get_prompt(params).await.map_err(|e| e.to_string()) }).await?;
        let texts: Vec<String> = got
            .messages
            .iter()
            .map(|message| match &message.content {
                ContentBlock::Text(text) => text.text.clone(),
                ContentBlock::Resource(resource) => resource_text(&resource.resource),
                other => serde_json::to_string(other).unwrap_or_default(),
            })
            .collect();
        Ok(texts.join("\n\n"))
    }
}

/// Keeps the era a connect found; one that failed forgets the remembered era, so the next connect probes.
fn remember_era(store: &Store, row: &ServerRow, found: Option<Era>) {
    if found != row.era {
        let _ = store.set_mcp_era(&row.name, &row.config, found);
    }
}

/// However a connect ends, even dropped mid-flight, its record goes and waiters hear.
struct Settle<'a> {
    servers: &'a Servers,
    hub: &'a Hub,
    row: &'a ServerRow,
    key: &'a Key,
    id: u64,
}

impl Drop for Settle<'_> {
    fn drop(&mut self) {
        let mut slots = self.servers.lock();
        let abandoned = slots.attempts.get(self.key).is_some_and(|a| a.id == self.id);
        if abandoned {
            slots.attempts.remove(self.key);
            slots.transient.remove(self.key);
        }
        drop(slots);
        if abandoned {
            self.hub.publish(Event::McpUpdated { server: self.servers.status_of(self.row.clone()) });
        }
        self.servers.settled.notify_waiters();
    }
}

/// The signed-in server a connect is for, when it has a sign-in to use.
#[derive(Clone, Copy)]
struct SignIn<'a> {
    server: &'a str,
    credentials: Option<&'a Arc<crate::llm::credentials::Credentials>>,
}

async fn open(config: &ServerConfig, hash: String, sign_in: SignIn<'_>, known: Option<Era>, workspace: Option<&Path>) -> Result<Live, Failure> {
    let (service, tree) = begin(config, sign_in, known, workspace).await?;
    let (tools, ttl) = within("list its tools", list_tools(&service)).await?;
    let info = service.peer_info();
    let era = info.as_ref().map_or(Era::Legacy, |info| Era::of(&info.protocol_version));
    let instructions = info.as_ref().and_then(|info| info.instructions.clone()).map(|text| text.trim().to_string()).filter(|text| !text.is_empty());
    let resources = info.as_ref().is_some_and(|info| info.capabilities.resources.is_some());
    // A server whose prompts cannot be listed still serves its tools; it simply offers no commands.
    let prompts = match info.as_ref().is_some_and(|info| info.capabilities.prompts.is_some()) {
        true => within("list its prompts", async { service.list_all_prompts().await.map_err(|e| e.to_string()) }).await.unwrap_or_default(),
        false => Vec::new(),
    };
    Ok(Live { service, era, transport: Transport::of(config), listing: Mutex::new(Listing::new(tools, ttl)), instructions, prompts, resources, timeout: config.timeout(), hash, since: Instant::now(), tree })
}

/// Every page of the server's tools, and the shortest freshness any page gave.
async fn list_tools(service: &Client) -> Result<(Vec<rmcp::model::Tool>, Option<Duration>), String> {
    let (mut tools, mut ttl, mut cursor) = (Vec::new(), None::<u64>, None);
    loop {
        let page = service.list_tools(Some(rmcp::model::PaginatedRequestParams::default().with_cursor(cursor))).await.map_err(|e| format!("tools/list failed: {e}"))?;
        ttl = match (ttl, page.ttl_ms) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        tools.extend(page.tools);
        cursor = page.next_cursor;
        if cursor.is_none() {
            return Ok((tools, ttl.map(Duration::from_millis)));
        }
    }
}

/// The eras a connect tries in turn (`None` is rmcp's probe-then-handshake), and whether a try that timed out moves on.
fn attempts(config: &ServerConfig, known: Option<Era>) -> (Vec<Option<Era>>, bool) {
    match (config, known) {
        (ServerConfig::Sse { .. }, _) => (vec![Some(Era::Legacy)], false),
        // rmcp's own fallback gives up on the probe after 10 s and then talks over a slow starter's late answer, so stdio probes alone and starts afresh for the handshake.
        (ServerConfig::Stdio { .. }, None) => (vec![Some(Era::Stateless), Some(Era::Legacy)], true),
        (ServerConfig::Stdio { .. }, Some(era)) => (vec![Some(era), Some(era.other())], false),
        (ServerConfig::Http { .. }, None) => (vec![None], false),
        (ServerConfig::Http { .. }, Some(era)) => (vec![Some(era), None], false),
    }
}

/// Starts in each era `attempts` gives until one answers; the last failure is the one reported.
async fn begin(config: &ServerConfig, sign_in: SignIn<'_>, known: Option<Era>, workspace: Option<&Path>) -> Result<(Client, Option<Tree>), Failure> {
    let (tries, past_timeouts) = attempts(config, known);
    let mut failed = Failure::from(String::new());
    for era in tries {
        let limit = if era.is_none() { STEP_LIMIT + PROBE_WAIT } else { STEP_LIMIT };
        match tokio::time::timeout(limit, start(config, sign_in, era, workspace)).await {
            Ok(Ok(started)) => return Ok(started),
            Ok(Err(error)) => failed = error,
            Err(_) => {
                failed = Failure::from(format!("the server did not start within {limit:?}"));
                if !past_timeouts {
                    break;
                }
            }
        }
    }
    Err(failed)
}

async fn within<T>(what: &str, step: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    within_for(STEP_LIMIT, what, step).await
}

async fn within_for<T>(limit: Duration, what: &str, step: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    tokio::time::timeout(limit, step).await.unwrap_or_else(|_| Err(format!("the server did not {what} within {limit:?}")))
}

/// Opens the transport and begins the session in the server's era, probing for it when `known` is `None`; HTTP+SSE predates the probe.
/// A stdio server for a workspace runs there (its own `cwd` wins) and is told it as its root.
async fn start(config: &ServerConfig, sign_in: SignIn<'_>, known: Option<Era>, workspace: Option<&Path>) -> Result<(Client, Option<Tree>), Failure> {
    match config {
        ServerConfig::Stdio { command, args, env, cwd, .. } => {
            // Found on the PATH as it is now, so a program installed while Drift runs is found without a restart.
            let program = crate::platform::process::which(command).ok_or_else(|| format!("{command} was not found on PATH; install it, or give its full path"))?;
            let mut cmd = tokio::process::Command::new(program);
            crate::platform::process::use_current_path(&mut cmd, env);
            cmd.args(args).envs(env);
            if let Some(dir) = cwd.as_deref().filter(|cwd| !cwd.trim().is_empty()).map(PathBuf::from).or_else(|| workspace.map(Path::to_path_buf)) {
                cmd.current_dir(dir);
            }
            crate::platform::process::prepare(&mut cmd);
            #[cfg(windows)]
            cmd.creation_flags(0x0800_0000);
            let transport = TokioChildProcess::new(cmd).map_err(|e| format!("could not start {command}: {e}"))?;
            // Adopted before it answers, so a start cut short takes the server's children with it.
            let tree = transport.id().and_then(|pid| Tree::adopt(pid).ok());
            let service = DriftClient::rooted(workspace).serve_with_lifecycle(transport, lifecycle(known)).await?;
            Ok((service, tree))
        }
        ServerConfig::Http { url, headers, oauth: app, .. } => {
            let config = http_config(url, headers);
            // A server signed in to goes through rmcp's authorized client, which refreshes the token itself.
            let signed_in = match sign_in.credentials {
                Some(credentials) => oauth::signed_in_client(credentials, sign_in.server, url, app.as_ref()).await,
                None => None,
            };
            let service = match signed_in {
                Some(client) => DriftClient::rooted(None).serve_with_lifecycle(StreamableHttpClientTransport::with_client(client, config), lifecycle(known)).await,
                None => DriftClient::rooted(None).serve_with_lifecycle(StreamableHttpClientTransport::with_client(crate::llm::http::client(), config), lifecycle(known)).await,
            };
            Ok((service?, None))
        }
        ServerConfig::Sse { url, headers, oauth: app, .. } => {
            let mut headers = header_map(headers);
            // rmcp's authorized client speaks only streamable HTTP, so a signed-in SSE server gets its token, refreshed when due, as a header.
            if let Some(token) = match sign_in.credentials {
                Some(credentials) => oauth::signed_in_token(credentials, sign_in.server, url, app.as_ref()).await,
                None => None,
            } {
                if let Ok(value) = format!("Bearer {token}").parse() {
                    headers.insert(http::header::AUTHORIZATION, value);
                }
            }
            let transport = sse::SseTransport::connect(crate::llm::http::client(), url, headers).await?;
            Ok((DriftClient::rooted(None).serve_with_lifecycle(transport, ClientLifecycleMode::Initialize).await?, None))
        }
    }
}

/// Headers as given, any that are not valid HTTP left out.
fn header_map(headers: &BTreeMap<String, String>) -> http::HeaderMap {
    headers.iter().filter_map(|(name, value)| Some((name.parse::<http::HeaderName>().ok()?, value.parse::<http::HeaderValue>().ok()?))).collect()
}

fn http_config(url: &str, headers: &BTreeMap<String, String>) -> StreamableHttpClientTransportConfig {
    let mut config = StreamableHttpClientTransportConfig::with_uri(url);
    let mut custom = HashMap::new();
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("authorization") {
            config = config.auth_header(value.trim_start_matches("Bearer ").to_string());
            continue;
        }
        let (Ok(name), Ok(value)) = (name.parse::<http::HeaderName>(), value.parse::<http::HeaderValue>()) else { continue };
        custom.insert(name, value);
    }
    config.custom_headers(custom)
}

/// The wait before reconnecting: the first again after a connection that held, else the one carried over.
fn after_loss(lived: Duration, carried: Duration) -> Duration {
    if lived >= STABLE {
        FIRST_RETRY
    } else {
        carried
    }
}

impl crate::Engine {
    /// [`Self::connect_mcp_in`] with no workspace: a remote server's connection, or a stdio server's wherever it runs.
    pub async fn connect_mcp(self: &Arc<Self>, name: &str) -> Result<(), String> {
        self.connect_mcp_in(name, None).await
    }

    /// The user's connect (Connect, a save, an enable): a remote server's shared connection; a stdio
    /// server's for `workspace` (the active one) and for every other workspace it runs in, all at once.
    /// A stdio server running nowhere and given no workspace is refused with [`NEEDS_WORKSPACE`].
    pub async fn connect_mcp_in(self: &Arc<Self>, name: &str, workspace: Option<&Path>) -> Result<(), String> {
        let stdio = self.store.mcp_server(name).ok().flatten().is_some_and(|row| !row.config.is_remote());
        if !stdio {
            return self.connect_mcp_at(&Key::shared(name), Start::User, FIRST_RETRY).await;
        }
        let mut keys = self.mcp.lock().keys_of(name);
        keys.extend(workspace.map(|workspace| Key::of(name, Some(workspace))));
        keys.retain(|key| key.workspace.is_some());
        keys.sort();
        keys.dedup();
        if keys.is_empty() {
            return Err(NEEDS_WORKSPACE.into());
        }
        let connected = futures_util::future::join_all(keys.iter().map(|key| self.connect_mcp_at(key, Start::User, FIRST_RETRY))).await;
        connected.into_iter().find(Result::is_err).unwrap_or(Ok(()))
    }

    /// `backoff` is the wait before reconnecting if this connection drops before it proves stable.
    async fn connect_mcp_at(self: &Arc<Self>, key: &Key, start: Start, backoff: Duration) -> Result<(), String> {
        let live = self.mcp.connect(key, &self.store, &self.hub, start).await?;
        self.watch_if_open(key, &live, backoff);
        Ok(())
    }

    fn watch_if_open(self: &Arc<Self>, key: &Key, live: &Arc<Live>, backoff: Duration) {
        if !live.holds_nothing_open() {
            self.watch_mcp(key.clone(), Arc::downgrade(live), backoff);
        }
    }

    /// What the watch does for a server with nothing to watch, asked by a call that failed: a client that has ended is replaced.
    fn recheck_mcp(self: &Arc<Self>, key: &Key, live: &Arc<Live>) {
        if let Watch::Lost { generation, lived } = self.mcp.check(key, &Arc::downgrade(live)) {
            self.lost_mcp(&key.server);
            tokio::spawn(self.clone().reconnect_mcp(key.clone(), generation, after_loss(lived, FIRST_RETRY)));
        }
    }

    /// Begins connecting every enabled remote server not live or connecting, each on its own so none
    /// waits on another; `wait_ready` sees them at once. A stdio server starts for a workspace when a
    /// turn there is planned ([`Self::start_workspace_mcp`]).
    pub fn connect_all_mcp(self: &Arc<Self>) {
        let Ok(rows) = self.store.mcp_servers() else { return };
        for row in rows.into_iter().filter(|row| row.enabled && row.config.is_remote()) {
            self.begin_mcp(Key::shared(&row.name));
        }
    }

    /// Starts each enabled stdio server's connection for `workspace` that is not live, connecting or failed.
    pub(crate) fn start_workspace_mcp(self: &Arc<Self>, workspace: &Path) {
        let Ok(rows) = self.store.mcp_servers() else { return };
        for row in rows.into_iter().filter(|row| row.enabled && !row.config.is_remote()) {
            self.begin_mcp(Key::of(&row.name, Some(workspace)));
        }
    }

    /// Stops the stdio servers running in a workspace the user removed; using it again starts them again.
    pub fn stop_workspace_mcp(&self, workspace_id: &str) {
        if let Ok(Some(workspace)) = self.store.workspace(workspace_id) {
            self.mcp.stop_workspace(&crate::tool::canonical(Path::new(&workspace.path)), &self.store, &self.hub);
        }
    }

    /// Every `IDLE_SWEEP`, stops workspace connections left unused for `IDLE`, until the engine is gone.
    pub async fn stop_idle_mcp(self: Arc<Self>) {
        let engine = Arc::downgrade(&self);
        drop(self);
        let mut every = tokio::time::interval(IDLE_SWEEP);
        loop {
            every.tick().await;
            let Some(engine) = engine.upgrade() else { return };
            engine.mcp.stop_idle(IDLE, &engine.store, &engine.hub);
        }
    }

    fn begin_mcp(self: &Arc<Self>, key: Key) {
        let Ok((row, attempt)) = self.mcp.begin(&key, &self.store, Start::Startup) else { return };
        let engine = self.clone();
        tokio::spawn(async move {
            if let Ok(live) = engine.mcp.complete(row, key.clone(), attempt, &engine.store, &engine.hub).await {
                engine.watch_if_open(&key, &live, FIRST_RETRY);
            }
        });
    }

    /// Reconnects a connection that ends by itself; once it is no longer the connection at `key`, the watch ends.
    fn watch_mcp(self: &Arc<Self>, key: Key, live: Weak<Live>, backoff: Duration) {
        let engine = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(WATCH_INTERVAL).await;
                let Some(engine) = engine.upgrade() else { return };
                match engine.mcp.check(&key, &live) {
                    Watch::Holding => {}
                    Watch::Gone => return,
                    Watch::Lost { generation, lived } => {
                        engine.lost_mcp(&key.server);
                        return engine.reconnect_mcp(key, generation, after_loss(lived, backoff)).await;
                    }
                }
            }
        });
    }

    fn lost_mcp(&self, name: &str) {
        if let Ok(Some(row)) = self.store.mcp_server(name) {
            self.hub.publish(Event::McpUpdated { server: self.mcp.status_of(row) });
        }
    }

    /// Tries again with growing waits until it connects, someone else connects it, or its generation changes.
    async fn reconnect_mcp(self: Arc<Self>, key: Key, generation: u64, mut wait: Duration) {
        let engine = Arc::downgrade(&self);
        drop(self);
        loop {
            tokio::time::sleep(wait).await;
            let Some(engine) = engine.upgrade() else { return };
            if engine.mcp.generation_of(&key) != generation || engine.mcp.is_live(&key) {
                return;
            }
            wait = (wait * 2).min(MAX_RETRY);
            if engine.connect_mcp_at(&key, Start::Reconnect(generation), wait).await.is_ok() {
                return;
            }
        }
    }
}
impl Live {
    fn is_open(&self) -> bool {
        !self.service.is_transport_closed() && !self.service.is_closed()
    }

    fn tools(&self) -> Vec<rmcp::model::Tool> {
        self.listing.lock().unwrap().tools.clone()
    }

    fn listing_due(&self) -> bool {
        self.listing.lock().unwrap().stale_at.is_some_and(|at| at <= Instant::now())
    }

    /// Takes a fresh list; whether the tools changed.
    fn relisted(&self, tools: Vec<rmcp::model::Tool>, ttl: Option<Duration>) -> bool {
        let mut listing = self.listing.lock().unwrap();
        let changed = listing.tools != tools;
        *listing = Listing::new(tools, ttl);
        changed
    }

    fn relist_failed(&self) {
        self.listing.lock().unwrap().stale_at = Some(Instant::now() + RELIST_BACKOFF);
    }

    /// A stateless server over HTTP: one POST per request and no session, so between calls there is no connection to lose.
    fn holds_nothing_open(&self) -> bool {
        self.era == Era::Stateless && self.transport == Transport::StreamableHttp
    }

    /// Ends the connection and kills its process tree even while others still hold it.
    fn close(&self) {
        self.service.cancellation_token().cancel();
        if let Some(tree) = &self.tree {
            tree.kill();
        }
    }

    async fn call(&self, name: &str, arguments: serde_json::Value) -> Result<Answer, CallError> {
        let arguments = arguments.as_object().cloned();
        let mut params = CallToolRequestParams::new(name.to_string());
        params.arguments = arguments;
        let result = self.service.call_tool(params).await.map_err(|error| match error {
            ServiceError::TransportClosed | ServiceError::TransportSend(_) | ServiceError::Cancelled { .. } => CallError::Lost,
            ServiceError::InputRequiredRoundsExceeded { .. } => {
                CallError::Failed("the server kept asking for input Drift does not give (a person's answer, a model's reply or the roots), so the call did not finish".into())
            }
            other => CallError::Failed(other.to_string()),
        })?;
        let mut answer = Answer { text: String::new(), is_error: result.is_error.unwrap_or(false), images: Vec::new() };
        let mut lines = Vec::new();
        for block in &result.content {
            match block {
                ContentBlock::Text(text) => lines.push(text.text.clone()),
                ContentBlock::Image(image) => match sendable(&image.mime_type, &image.data) {
                    Ok(()) => answer.images.push(Image { mime: image.mime_type.clone(), base64: image.data.clone() }),
                    Err(why) => lines.push(format!("[an image ({}) not shown: {why}]", image.mime_type)),
                },
                ContentBlock::Resource(resource) => take_resource(&resource.resource, &mut answer.images, &mut lines),
                other => lines.push(serde_json::to_string(other).unwrap_or_default()),
            }
        }
        answer.text = lines.join("\n");
        Ok(answer)
    }
}

/// What a call returned: its text, whether the server called it an error, and any images for the model.
pub(super) struct Answer {
    pub text: String,
    pub is_error: bool,
    pub images: Vec<Image>,
}

#[cfg(test)]
#[test]
fn only_png_jpeg_gif_and_webp_within_the_limit_are_sent() {
    assert!(sendable("image/png", "AAAA").is_ok() && sendable("image/webp", "AAAA").is_ok());
    assert!(sendable("image/svg+xml", "AAAA").unwrap_err().contains("only PNG"));
    assert!(sendable("image/bmp", "AAAA").is_err());
    assert!(sendable("image/png", &"A".repeat(8 * 1024 * 1024)).is_ok(), "scaled down when kept");
    assert!(sendable("image/png", &"A".repeat(44 * 1024 * 1024)).unwrap_err().contains("32 MB"));
}

#[cfg(test)]
#[test]
fn an_embedded_image_or_pdf_is_attached_and_anything_else_is_text() {
    let resource = |json: serde_json::Value| serde_json::from_value::<rmcp::model::ResourceContents>(json).unwrap();
    let (mut images, mut lines) = (Vec::new(), Vec::new());
    take_resource(&resource(serde_json::json!({ "uri": "shot://1", "mimeType": "image/png", "blob": "AAAA" })), &mut images, &mut lines);
    take_resource(&resource(serde_json::json!({ "uri": "doc://1", "mimeType": "application/pdf", "blob": "JVBE" })), &mut images, &mut lines);
    take_resource(&resource(serde_json::json!({ "uri": "zip://1", "mimeType": "application/zip", "blob": "UEsD" })), &mut images, &mut lines);
    take_resource(&resource(serde_json::json!({ "uri": "note://1", "text": "hello" })), &mut images, &mut lines);
    assert_eq!(images.iter().map(|image| image.mime.as_str()).collect::<Vec<_>>(), ["image/png", "application/pdf"]);
    assert!(lines[0].starts_with("[binary resource zip://1") && lines[1].contains("hello"), "{lines:?}");
}

/// Whether an MCP image can go to a model: a format every provider takes, within the size limit.
fn sendable(mime: &str, base64: &str) -> Result<(), &'static str> {
    if !crate::tool::image::SENDABLE.contains(&mime) {
        return Err("only PNG, JPEG, GIF and WebP reach the model");
    }
    if base64.len() > crate::tool::image::MAX_SOURCE_BYTES * 4 / 3 + 4 {
        return Err("larger than 32 MB");
    }
    Ok(())
}

/// A resource, read or embedded in a call's result, as opencode passes it on: an image or PDF the
/// model can take is attached, anything else becomes text.
fn take_resource(resource: &rmcp::model::ResourceContents, images: &mut Vec<Image>, lines: &mut Vec<String>) {
    match resource {
        rmcp::model::ResourceContents::BlobResourceContents { mime_type: Some(mime), blob, .. } if sendable(mime, blob).is_ok() || mime == crate::tool::image::PDF => {
            images.push(Image { mime: mime.clone(), base64: blob.clone() });
        }
        other => lines.push(resource_text(other)),
    }
}

/// An embedded resource as the model reads it: its text, or a line naming a binary one.
fn resource_text(resource: &rmcp::model::ResourceContents) -> String {
    match resource {
        rmcp::model::ResourceContents::TextResourceContents { uri, text, .. } => format!("<resource uri=\"{uri}\">\n{text}\n</resource>"),
        rmcp::model::ResourceContents::BlobResourceContents { uri, mime_type, blob, .. } => {
            format!("[binary resource {uri} ({}, {} bytes), not shown]", mime_type.as_deref().unwrap_or("unknown type"), blob.len() * 3 / 4)
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn tool_info(tool: &rmcp::model::Tool) -> ToolInfo {
    ToolInfo {
        name: tool.name.to_string(),
        description: tool.description.clone().unwrap_or_default().to_string(),
        read_only: tool.annotations.as_ref().and_then(|a| a.read_only_hint).unwrap_or(false),
    }
}

#[cfg(test)]
mod tests;
