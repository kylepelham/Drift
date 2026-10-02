//! MCP servers: configured in the store, connected with rmcp, tools offered to the model.

mod oauth;
mod resources;
mod sse;
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
use crate::tool::image::Image;

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
        timeout_seconds: Option<u64>,
    },
    /// The older HTTP+SSE transport some servers still speak.
    Sse {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_seconds: Option<u64>,
    },
}

impl ServerConfig {
    /// How long one tool call may take before it fails; unset, it runs until done or stopped.
    pub fn timeout(&self) -> Option<Duration> {
        let (Self::Stdio { timeout_seconds, .. } | Self::Http { timeout_seconds, .. } | Self::Sse { timeout_seconds, .. }) = self;
        timeout_seconds.filter(|s| *s > 0).map(Duration::from_secs)
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
    /// The MCP protocol version the server agreed to at initialize; absent until connected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// The server refused to connect until the user signs in (`POST /mcp/{name}/signin`).
    pub needs_sign_in: bool,
    /// A sign-in is kept for it (`DELETE /mcp/{name}/signin` forgets it).
    pub signed_in: bool,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub read_only: bool,
}

struct Live {
    service: RunningService<RoleClient, ()>,
    tools: Vec<rmcp::model::Tool>,
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
    /// Where remote servers' sign-ins are kept; none in a bare test registry.
    sign_ins: Option<Arc<crate::llm::credentials::Credentials>>,
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

    pub fn statuses(&self, store: &Store) -> rusqlite::Result<Vec<ServerStatus>> {
        Ok(store.mcp_servers()?.into_iter().map(|row| self.status_of(row)).collect())
    }

    pub fn status_of(&self, row: ServerRow) -> ServerStatus {
        let slots = self.lock();
        let live = slots.live(&row.name);
        let (state, error) = if !row.enabled {
            (State::Disabled, None)
        } else if live.is_some() {
            (State::Connected, None)
        } else {
            slots.transient.get(&row.name).cloned().unwrap_or((State::Disconnected, None))
        };
        let protocol = live.as_ref().and_then(|live| live.service.peer_info()).map(|info| info.protocol_version.to_string());
        let tools = live.map(|live| live.tools.iter().map(tool_info).collect()).unwrap_or_default();
        let remote = matches!(row.config, ServerConfig::Http { .. });
        let needs_sign_in = remote && state == State::Failed && error.as_deref().is_some_and(oauth::wants_sign_in);
        let signed_in = remote && self.sign_ins.as_ref().is_some_and(|store| oauth::has_sign_in(store, &row.name));
        ServerStatus { transport: Transport::of(&row.config), protocol, needs_sign_in, signed_in, server: ServerView::of(&row), state, error, tools }
    }

    /// Connects `name` as its row stands now.
    async fn connect(&self, name: &str, store: &Store, hub: &Hub, start: Start) -> Result<Arc<Live>, String> {
        let (row, attempt) = self.begin(name, store, start)?;
        let _settle = Settle { servers: self, hub, row: &row, id: attempt.id };
        hub.publish(Event::McpUpdated { server: self.status_of(row.clone()) });
        let opened = tokio::select! {
            opened = open(&row.config, row.hash.clone(), SignIn { server: &row.name, credentials: self.sign_ins.as_ref() }) => opened,
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
        if slots.servers.values().any(|slot| slot.current().is_some_and(|live| live.resources)) {
            tools.push(Arc::new(resources::ListResources));
            tools.push(Arc::new(resources::ReadResource));
        }
        tools
    }

    /// Each connected server's own instructions, by server name.
    pub fn instructions(&self) -> Vec<(String, String)> {
        let slots = self.lock();
        slots.servers.iter().filter_map(|(server, slot)| Some((server.clone(), slot.current()?.instructions.clone()?))).collect()
    }

    fn live(&self, server: &str) -> Result<Arc<Live>, String> {
        self.lock().servers.get(server).and_then(|slot| slot.current()).ok_or_else(|| format!("the {server} MCP server is not connected"))
    }

    /// Connected servers that serve resources, by name.
    pub fn with_resources(&self) -> Vec<String> {
        let slots = self.lock();
        let mut names: Vec<String> = slots.servers.iter().filter(|(_, slot)| slot.current().is_some_and(|live| live.resources)).map(|(name, _)| name.clone()).collect();
        names.sort();
        names
    }

    /// Every connected server's prompts, as `(server, prompt)`.
    pub fn prompts(&self) -> Vec<(String, rmcp::model::Prompt)> {
        let slots = self.lock();
        let mut all: Vec<(String, rmcp::model::Prompt)> =
            slots.servers.iter().filter_map(|(name, slot)| slot.current().map(|live| (name.clone(), live))).flat_map(|(name, live)| live.prompts.iter().map(move |p| (name.clone(), p.clone())).collect::<Vec<_>>()).collect();
        all.sort_by(|a, b| (&a.0, &a.1.name).cmp(&(&b.0, &b.1.name)));
        all
    }

    pub async fn list_resources(&self, server: &str) -> Result<Vec<rmcp::model::Resource>, String> {
        let live = self.live(server)?;
        within("list its resources", async { live.service.list_all_resources().await.map_err(|e| e.to_string()) }).await
    }

    /// A resource's contents: text inline, images and PDFs as files, other binaries named.
    pub(crate) async fn read_resource(&self, server: &str, uri: &str) -> Result<Answer, String> {
        let live = self.live(server)?;
        let read = within("read the resource", async { live.service.read_resource(rmcp::model::ReadResourceRequestParams::new(uri)).await.map_err(|e| e.to_string()) }).await?;
        let mut answer = Answer { text: String::new(), is_error: false, images: Vec::new() };
        let mut lines = Vec::new();
        for content in &read.contents {
            match content {
                rmcp::model::ResourceContents::BlobResourceContents { mime_type: Some(mime), blob, .. } if sendable(mime, blob).is_ok() || mime == crate::tool::image::PDF => {
                    answer.images.push(Image { mime: mime.clone(), base64: blob.clone() });
                }
                other => lines.push(resource_text(other)),
            }
        }
        answer.text = lines.join("\n");
        Ok(answer)
    }

    /// Every connected server's prompts as slash commands named `server:prompt`.
    pub fn prompt_commands(&self) -> Vec<crate::config::Command> {
        self.prompts()
            .into_iter()
            .map(|(server, prompt)| crate::config::Command {
                name: format!("{server}:{}", prompt.name),
                description: prompt.description.clone().unwrap_or_else(|| format!("A prompt from the {server} MCP server")),
                template: String::new(),
                arguments: prompt.arguments.iter().flatten().map(|argument| argument.name.clone()).collect(),
                server: Some(server),
            })
            .collect()
    }

    /// A prompt filled with `arguments`, as the text of its messages.
    pub async fn get_prompt(&self, server: &str, name: &str, arguments: serde_json::Map<String, serde_json::Value>) -> Result<String, String> {
        let live = self.live(server)?;
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

/// The signed-in server a connect is for, when it has a sign-in to use.
struct SignIn<'a> {
    server: &'a str,
    credentials: Option<&'a Arc<crate::llm::credentials::Credentials>>,
}

async fn open(config: &ServerConfig, hash: String, sign_in: SignIn<'_>) -> Result<Live, String> {
    let (service, tree) = within("start", start(config, sign_in)).await?;
    let tools = within("list its tools", async { service.list_all_tools().await.map_err(|e| format!("tools/list failed: {e}")) }).await?;
    let info = service.peer_info();
    let instructions = info.as_ref().and_then(|info| info.instructions.clone()).map(|text| text.trim().to_string()).filter(|text| !text.is_empty());
    let resources = info.as_ref().is_some_and(|info| info.capabilities.resources.is_some());
    // A server whose prompts cannot be listed still serves its tools; it simply offers no commands.
    let prompts = match info.as_ref().is_some_and(|info| info.capabilities.prompts.is_some()) {
        true => within("list its prompts", async { service.list_all_prompts().await.map_err(|e| e.to_string()) }).await.unwrap_or_default(),
        false => Vec::new(),
    };
    Ok(Live { service, tools, instructions, prompts, resources, timeout: config.timeout(), hash, since: Instant::now(), tree })
}

async fn within<T>(what: &str, step: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    tokio::time::timeout(STEP_LIMIT, step).await.unwrap_or_else(|_| Err(format!("the server did not {what} within {STEP_LIMIT:?}")))
}

async fn start(config: &ServerConfig, sign_in: SignIn<'_>) -> Result<(RunningService<RoleClient, ()>, Option<Tree>), String> {
    match config {
        ServerConfig::Stdio { command, args, env, cwd, .. } => {
            let mut cmd = tokio::process::Command::new(command);
            cmd.args(args).envs(env);
            if let Some(cwd) = cwd.as_deref().filter(|cwd| !cwd.trim().is_empty()) {
                cmd.current_dir(cwd);
            }
            crate::platform::process::prepare(&mut cmd);
            #[cfg(windows)]
            cmd.creation_flags(0x0800_0000);
            let transport = TokioChildProcess::new(cmd).map_err(|e| format!("could not start {command}: {e}"))?;
            // Adopted before it answers, so a start cut short takes the server's children with it.
            let tree = transport.id().and_then(|pid| Tree::adopt(pid).ok());
            let service = ().serve(transport).await.map_err(|e| e.to_string())?;
            Ok((service, tree))
        }
        ServerConfig::Http { url, headers, .. } => {
            let config = http_config(url, headers);
            // A server signed in to goes through rmcp's authorized client, which refreshes the token itself.
            let signed_in = match sign_in.credentials {
                Some(credentials) => oauth::signed_in_client(credentials, sign_in.server, url).await,
                None => None,
            };
            let service = match signed_in {
                Some(client) => ().serve(StreamableHttpClientTransport::with_client(client, config)).await,
                None => ().serve(StreamableHttpClientTransport::with_client(crate::llm::http::client(), config)).await,
            };
            Ok((service.map_err(|e| e.to_string())?, None))
        }
        ServerConfig::Sse { url, headers, .. } => {
            let transport = sse::SseTransport::connect(crate::llm::http::client(), url, header_map(headers)).await?;
            Ok((().serve(transport).await.map_err(|e| e.to_string())?, None))
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

    /// Connects every enabled server that is neither live nor already connecting.
    pub async fn connect_all_mcp(self: &Arc<Self>) {
        let Ok(rows) = self.store.mcp_servers() else { return };
        for row in rows.into_iter().filter(|r| r.enabled) {
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

    async fn call(&self, name: &str, arguments: serde_json::Value) -> Result<Answer, CallError> {
        let arguments = arguments.as_object().cloned();
        let mut params = CallToolRequestParams::new(name.to_string());
        params.arguments = arguments;
        let result = self.service.call_tool(params).await.map_err(|error| match error {
            ServiceError::TransportClosed | ServiceError::TransportSend(_) | ServiceError::Cancelled { .. } => CallError::Lost,
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
                ContentBlock::Resource(resource) => lines.push(resource_text(&resource.resource)),
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
    assert!(sendable("image/png", &"A".repeat(8 * 1024 * 1024)).unwrap_err().contains("5 MB"));
}

/// Whether an MCP image can go to a model: a format every provider takes, within the size limit.
fn sendable(mime: &str, base64: &str) -> Result<(), &'static str> {
    if !crate::tool::image::SENDABLE.contains(&mime) {
        return Err("only PNG, JPEG, GIF and WebP reach the model");
    }
    if base64.len() > crate::tool::image::MAX_IMAGE_BYTES * 4 / 3 + 4 {
        return Err("larger than 5 MB");
    }
    Ok(())
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
