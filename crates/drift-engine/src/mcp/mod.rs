//! MCP servers: configured in the store, approved by the user, connected with rmcp, tools offered to the model.

mod tool;
mod view;

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use rmcp::model::{CallToolRequestParams, ContentBlock};
use rmcp::service::{RunningService, ServiceError};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;

use crate::event::{Event, Hub};
use crate::platform::process::Tree;
use crate::store::Store;

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
    },
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerRow {
    pub name: String,
    pub config: ServerConfig,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_hash: Option<String>,
    pub updated_at: i64,
}

impl ServerRow {
    /// Identity of the config as approved; a different command or URL is a different thing to approve.
    pub fn hash(&self) -> String {
        use sha2::Digest;
        let json = serde_json::to_string(&self.config).unwrap();
        sha2::Sha256::digest(json.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
    }

    pub fn is_approved(&self) -> bool {
        self.approved_hash.as_deref() == Some(self.hash().as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Disabled,
    NeedsApproval,
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub read_only: bool,
}

struct Live {
    service: RunningService<RoleClient, ()>,
    tools: Vec<rmcp::model::Tool>,
    /// The definition it was opened from; a client of another definition never stands in for this one.
    hash: String,
    since: Instant,
    /// A stdio server's process tree; it dies with the last handle to this connection.
    tree: Option<Tree>,
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
}

impl Slot {
    fn current(&self) -> Option<Arc<Live>> {
        self.current.lock().unwrap().clone()
    }

    fn publish(&self, live: Arc<Live>) {
        let mut served = self.served.lock().unwrap();
        served.retain(|client| client.strong_count() > 0);
        served.push(Arc::downgrade(&live));
        drop(served);
        *self.current.lock().unwrap() = Some(live);
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
/// How long a turn being planned waits for connects already under way, so its tools are not briefly missing.
pub const READY_WAIT: Duration = Duration::from_secs(2);
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

#[derive(Default)]
struct Slots {
    servers: HashMap<String, Arc<Slot>>,
    transient: HashMap<String, (State, Option<String>)>,
    /// Bumped by every save, disable, disconnect and remove; a connect from an older one publishes nothing.
    generation: HashMap<String, u64>,
    attempts: HashMap<String, Attempt>,
}

impl Slots {
    fn generation_of(&self, name: &str) -> u64 {
        self.generation.get(name).copied().unwrap_or(0)
    }

    fn is_current(&self, name: &str, attempt: &Attempt) -> bool {
        self.attempts.get(name).is_some_and(|a| a.id == attempt.id) && self.generation_of(name) == attempt.generation
    }

    fn live(&self, name: &str) -> Option<Arc<Live>> {
        self.servers.get(name).and_then(|slot| slot.current())
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
}

enum Watch {
    Holding,
    Gone,
    Lost { generation: u64, lived: Duration },
}

impl Servers {
    fn lock(&self) -> MutexGuard<'_, Slots> {
        self.slots.lock().unwrap()
    }

    pub fn statuses(&self, store: &Store) -> rusqlite::Result<Vec<ServerStatus>> {
        Ok(store.mcp_servers()?.into_iter().map(|row| self.status_of(row)).collect())
    }

    pub fn status_of(&self, row: ServerRow) -> ServerStatus {
        let slots = self.lock();
        let live = slots.live(&row.name);
        let (state, error) = if !row.enabled {
            (State::Disabled, None)
        } else if !row.is_approved() {
            (State::NeedsApproval, None)
        } else if live.is_some() {
            (State::Connected, None)
        } else {
            slots.transient.get(&row.name).cloned().unwrap_or((State::Disconnected, None))
        };
        let tools = live.map(|live| live.tools.iter().map(tool_info).collect()).unwrap_or_default();
        ServerStatus { server: ServerView::of(&row), state, error, tools }
    }

    /// Connects `name` as its row stands now.
    async fn connect(&self, name: &str, store: &Store, hub: &Hub, start: Start) -> Result<Arc<Live>, String> {
        let (row, attempt) = self.begin(name, store, start)?;
        let _settle = Settle { servers: self, hub, row: &row, id: attempt.id };
        hub.publish(Event::McpUpdated { server: self.status_of(row.clone()) });
        let opened = tokio::select! {
            opened = open(&row.config, row.hash()) => opened,
            () = attempt.cancel.cancelled() => Err("server definition changed during connect".into()),
        };
        self.finish(&row, hub, &attempt, opened)
    }

    /// Reads the row and its generation together, so a save cannot slip between them.
    fn begin(&self, name: &str, store: &Store, start: Start) -> Result<(ServerRow, Attempt), String> {
        let mut slots = self.lock();
        let row = store.mcp_server(name).map_err(|e| e.to_string())?.ok_or("no such server")?;
        let generation = slots.generation_of(name);
        if matches!(start, Start::Reconnect(expected) if expected != generation) {
            return Err("server definition changed".into());
        }
        if start != Start::User && (slots.attempts.contains_key(name) || slots.live(name).is_some()) {
            return Err("already connected or connecting".into());
        }
        if !row.enabled {
            return Err("server is disabled".into());
        }
        if !row.is_approved() {
            return Err("server needs approval".into());
        }
        let attempt = Attempt { id: self.next_attempt.fetch_add(1, Ordering::Relaxed), generation, cancel: CancellationToken::new() };
        if let Some(earlier) = slots.attempts.insert(name.into(), attempt.clone()) {
            earlier.cancel.cancel();
        }
        slots.transient.insert(name.into(), (State::Connecting, None));
        Ok((row, attempt))
    }

    /// Publishes what the attempt opened, unless it was overtaken; then what it opened is dropped, killing it.
    fn finish(&self, row: &ServerRow, hub: &Hub, attempt: &Attempt, opened: Result<Live, String>) -> Result<Arc<Live>, String> {
        let mut slots = self.lock();
        if !slots.is_current(&row.name, attempt) {
            return Err("server definition changed during connect".into());
        }
        slots.attempts.remove(&row.name);
        let result = match opened {
            Ok(live) => {
                let live = Arc::new(live);
                slots.servers.entry(row.name.clone()).or_default().publish(live.clone());
                slots.transient.remove(&row.name);
                Ok(live)
            }
            Err(error) => {
                slots.transient.insert(row.name.clone(), (State::Failed, Some(error.clone())));
                Err(error)
            }
        };
        drop(slots);
        hub.publish(Event::McpUpdated { server: self.status_of(row.clone()) });
        result
    }

    /// Writes the server's row and, in the same step, ends its connection and any connect in flight. A write that refuses changes nothing.
    fn detach<R, E>(&self, name: &str, store: &Store, ending: Ending, write: impl FnOnce(&Store) -> Result<R, E>) -> Result<(Option<Arc<Live>>, R), E> {
        let mut slots = self.lock();
        let written = write(store)?;
        *slots.generation.entry(name.into()).or_default() += 1;
        if let Some(attempt) = slots.attempts.remove(name) {
            attempt.cancel.cancel();
        }
        slots.transient.remove(name);
        let slot = if ending == Ending::Close { slots.servers.remove(name) } else { slots.servers.get(name).cloned() };
        let live = slot.as_ref().and_then(|slot| slot.take());
        if ending == Ending::Close {
            slot.inspect(|slot| slot.close());
        }
        Ok((live, written))
    }

    /// A save: the write and the end of the old connection are one step; running turns keep their client.
    pub async fn change<R, E>(&self, name: &str, store: &Store, hub: &Hub, write: impl FnOnce(&Store) -> Result<R, E>) -> Result<R, E> {
        let (live, written) = self.detach(name, store, Ending::Keep, write)?;
        self.retire(name, store, hub, live).await;
        Ok(written)
    }

    /// A disable, remove or rename: as [`Self::change`], and every client the server served is closed, running turns' too.
    pub async fn close<R, E>(&self, name: &str, store: &Store, hub: &Hub, write: impl FnOnce(&Store) -> Result<R, E>) -> Result<R, E> {
        let (live, written) = self.detach(name, store, Ending::Close, write)?;
        self.retire(name, store, hub, live).await;
        Ok(written)
    }

    pub async fn disconnect(&self, name: &str, store: &Store, hub: &Hub) -> bool {
        let Ok((live, ())) = self.detach(name, store, Ending::Keep, |_| Ok::<_, rusqlite::Error>(())) else { return false };
        let was_live = live.is_some();
        self.retire(name, store, hub, live).await;
        was_live
    }

    /// Closes a connection nothing else holds; one a running turn still holds closes when that turn lets go.
    async fn retire(&self, name: &str, store: &Store, hub: &Hub, live: Option<Arc<Live>>) {
        if let Some(Ok(live)) = live.map(Arc::try_unwrap) {
            let _ = live.service.cancel().await;
        }
        self.settled.notify_waiters();
        if let Ok(Some(row)) = store.mcp_server(name) {
            hub.publish(Event::McpUpdated { server: self.status_of(row) });
        }
    }

    fn generation_of(&self, name: &str) -> u64 {
        self.lock().generation_of(name)
    }

    fn is_live(&self, name: &str) -> bool {
        self.lock().live(name).is_some()
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

    /// Whether `live` is still the server's connection; one whose transport closed by itself is dropped here.
    fn check(&self, name: &str, live: &Weak<Live>) -> Watch {
        let mut slots = self.lock();
        let Some(slot) = slots.servers.get(name).cloned() else { return Watch::Gone };
        let Some(current) = slot.current().filter(|current| Arc::downgrade(current).ptr_eq(live)) else { return Watch::Gone };
        if current.is_open() {
            return Watch::Holding;
        }
        slot.take();
        slots.transient.insert(name.into(), (State::Connecting, Some("the connection closed; reconnecting".into())));
        Watch::Lost { generation: slots.generation_of(name), lived: current.since.elapsed() }
    }

    /// Every tool of every connected server, named `server_tool` so the model can tell them apart.
    pub fn tools(&self) -> Vec<Arc<dyn crate::tool::Tool>> {
        let slots = self.lock();
        let mut tools: Vec<Arc<dyn crate::tool::Tool>> = Vec::new();
        for (server, slot) in &slots.servers {
            let Some(live) = slot.current() else { continue };
            tools.extend(live.tools.iter().map(|tool| Arc::new(McpTool::new(server, tool.clone(), live.clone(), slot.clone())) as Arc<dyn crate::tool::Tool>));
        }
        tools
    }
}

/// However a connect ends, even dropped mid-flight, its record goes and waiters hear.
struct Settle<'a> {
    servers: &'a Servers,
    hub: &'a Hub,
    row: &'a ServerRow,
    id: u64,
}

impl Drop for Settle<'_> {
    fn drop(&mut self) {
        let mut slots = self.servers.lock();
        let abandoned = slots.attempts.get(&self.row.name).is_some_and(|a| a.id == self.id);
        if abandoned {
            slots.attempts.remove(&self.row.name);
            slots.transient.remove(&self.row.name);
        }
        drop(slots);
        if abandoned {
            self.hub.publish(Event::McpUpdated { server: self.servers.status_of(self.row.clone()) });
        }
        self.servers.settled.notify_waiters();
    }
}

async fn open(config: &ServerConfig, hash: String) -> Result<Live, String> {
    let (service, tree) = within("start", start(config)).await?;
    let tools = within("list its tools", async { service.list_all_tools().await.map_err(|e| format!("tools/list failed: {e}")) }).await?;
    Ok(Live { service, tools, hash, since: Instant::now(), tree })
}

async fn within<T>(what: &str, step: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    tokio::time::timeout(STEP_LIMIT, step).await.unwrap_or_else(|_| Err(format!("the server did not {what} within {STEP_LIMIT:?}")))
}

async fn start(config: &ServerConfig) -> Result<(RunningService<RoleClient, ()>, Option<Tree>), String> {
    match config {
        ServerConfig::Stdio { command, args, env } => {
            let mut cmd = tokio::process::Command::new(command);
            cmd.args(args).envs(env);
            crate::platform::process::prepare(&mut cmd);
            #[cfg(windows)]
            cmd.creation_flags(0x0800_0000);
            let transport = TokioChildProcess::new(cmd).map_err(|e| format!("could not start {command}: {e}"))?;
            // Adopted before it answers, so a start cut short takes the server's children with it.
            let tree = transport.id().and_then(|pid| Tree::adopt(pid).ok());
            let service = ().serve(transport).await.map_err(|e| e.to_string())?;
            Ok((service, tree))
        }
        ServerConfig::Http { url, headers } => Ok((().serve(http_transport(url, headers)).await.map_err(|e| e.to_string())?, None)),
    }
}

fn http_transport(url: &str, headers: &BTreeMap<String, String>) -> StreamableHttpClientTransport<reqwest::Client> {
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
    StreamableHttpClientTransport::with_client(crate::llm::http::client(), config.custom_headers(custom))
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
    /// Connects a server, offers its tools to later turns, and watches it for as long as its definition stands.
    pub async fn connect_mcp(self: &Arc<Self>, name: &str) -> Result<(), String> {
        self.connect_mcp_at(name, Start::User, FIRST_RETRY).await
    }

    /// `backoff` is the wait before reconnecting if this connection drops before it proves stable.
    async fn connect_mcp_at(self: &Arc<Self>, name: &str, start: Start, backoff: Duration) -> Result<(), String> {
        let live = self.mcp.connect(name, &self.store, &self.hub, start).await?;
        self.watch_mcp(name.into(), Arc::downgrade(&live), backoff);
        Ok(())
    }

    /// Connects every enabled, approved server that is neither live nor already connecting.
    pub async fn connect_all_mcp(self: &Arc<Self>) {
        let Ok(rows) = self.store.mcp_servers() else { return };
        for row in rows.into_iter().filter(|r| r.enabled && r.is_approved()) {
            let _ = self.connect_mcp_at(&row.name, Start::Startup, FIRST_RETRY).await;
        }
    }

    /// Reconnects a connection that ends by itself; once it is no longer the server's connection, the watch ends.
    fn watch_mcp(self: &Arc<Self>, name: String, live: Weak<Live>, backoff: Duration) {
        let engine = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(WATCH_INTERVAL).await;
                let Some(engine) = engine.upgrade() else { return };
                match engine.mcp.check(&name, &live) {
                    Watch::Holding => {}
                    Watch::Gone => return,
                    Watch::Lost { generation, lived } => {
                        engine.lost_mcp(&name);
                        return engine.reconnect_mcp(name, generation, after_loss(lived, backoff)).await;
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
    async fn reconnect_mcp(self: Arc<Self>, name: String, generation: u64, mut wait: Duration) {
        let engine = Arc::downgrade(&self);
        drop(self);
        loop {
            tokio::time::sleep(wait).await;
            let Some(engine) = engine.upgrade() else { return };
            if engine.mcp.generation_of(&name) != generation || engine.mcp.is_live(&name) {
                return;
            }
            wait = (wait * 2).min(MAX_RETRY);
            if engine.connect_mcp_at(&name, Start::Reconnect(generation), wait).await.is_ok() {
                return;
            }
        }
    }
}

impl Live {
    fn is_open(&self) -> bool {
        !self.service.is_transport_closed() && !self.service.is_closed()
    }

    /// Ends the connection and kills its process tree even while others still hold it.
    fn close(&self) {
        self.service.cancellation_token().cancel();
        if let Some(tree) = &self.tree {
            tree.kill();
        }
    }

    async fn call(&self, name: &str, arguments: serde_json::Value) -> Result<(String, bool), CallError> {
        let arguments = arguments.as_object().cloned();
        let mut params = CallToolRequestParams::new(name.to_string());
        params.arguments = arguments;
        let result = self.service.call_tool(params).await.map_err(|error| match error {
            ServiceError::TransportClosed | ServiceError::TransportSend(_) | ServiceError::Cancelled { .. } => CallError::Lost,
            other => CallError::Failed(other.to_string()),
        })?;
        let text: Vec<String> = result
            .content
            .iter()
            .map(|block| match block {
                ContentBlock::Text(text) => text.text.clone(),
                ContentBlock::Image(image) => format!("[image {}]", image.mime_type),
                ContentBlock::Resource(resource) => format!("[resource {:?}]", resource.resource),
                other => format!("[{other:?}]"),
            })
            .collect();
        Ok((text.join("\n"), result.is_error.unwrap_or(false)))
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
