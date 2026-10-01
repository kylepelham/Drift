//! MCP servers: configured in the store, approved by the user, connected with rmcp, tools offered to the model.

mod tool;

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::model::{CallToolRequestParams, ContentBlock};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::event::{Event, Hub};
use crate::store::Store;

pub use tool::McpTool;

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
    pub row: ServerRow,
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
/// How long a turn being planned waits for connects already under way, so its tools are not briefly missing.
pub const READY_WAIT: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct Servers {
    live: Mutex<HashMap<String, Arc<Live>>>,
    transient: Mutex<HashMap<String, (State, Option<String>)>>,
    /// Bumped by every save, disable, disconnect and remove; a connect that started under an older number is discarded.
    generation: Mutex<HashMap<String, u64>>,
    /// Signalled whenever a connect attempt ends, for turns waiting on the catalog.
    settled: tokio::sync::Notify,
}

impl Servers {
    pub fn statuses(&self, store: &Store) -> rusqlite::Result<Vec<ServerStatus>> {
        Ok(store.mcp_servers()?.into_iter().map(|row| self.status_of(row)).collect())
    }

    pub fn status_of(&self, row: ServerRow) -> ServerStatus {
        let (state, error) = if !row.enabled {
            (State::Disabled, None)
        } else if !row.is_approved() {
            (State::NeedsApproval, None)
        } else if let Some(live) = self.live.lock().unwrap().get(&row.name) {
            let _ = live;
            (State::Connected, None)
        } else {
            self.transient.lock().unwrap().get(&row.name).cloned().unwrap_or((State::Disconnected, None))
        };
        let tools = self
            .live
            .lock()
            .unwrap()
            .get(&row.name)
            .map(|live| live.tools.iter().map(tool_info).collect())
            .unwrap_or_default();
        ServerStatus { row, state, error, tools }
    }

    pub async fn connect(&self, row: ServerRow, hub: &Hub) -> Result<(), String> {
        if !row.enabled {
            return Err("server is disabled".into());
        }
        if !row.is_approved() {
            return Err("server needs approval".into());
        }
        let generation = self.current_generation(&row.name);
        self.set_transient(&row.name, State::Connecting, None);
        hub.publish(Event::McpUpdated { server: self.status_of(row.clone()) });
        let result = self.open(&row.config).await;
        let settled = self.settle_connect(row, hub, generation, result).await;
        self.settled.notify_waiters();
        settled
    }

    async fn settle_connect(&self, row: ServerRow, hub: &Hub, generation: u64, result: Result<Live, String>) -> Result<(), String> {
        if self.current_generation(&row.name) != generation {
            // The definition changed while we were connecting; whatever we opened belongs to a dead configuration.
            if let Ok(live) = result {
                let _ = live.service.cancel().await;
            }
            return Err("server definition changed during connect".into());
        }
        match result {
            Ok(live) => {
                self.live.lock().unwrap().insert(row.name.clone(), Arc::new(live));
                self.transient.lock().unwrap().remove(&row.name);
                hub.publish(Event::McpUpdated { server: self.status_of(row) });
                Ok(())
            }
            Err(error) => {
                self.set_transient(&row.name, State::Failed, Some(error.clone()));
                hub.publish(Event::McpUpdated { server: self.status_of(row) });
                Err(error)
            }
        }
    }

    async fn open(&self, config: &ServerConfig) -> Result<Live, String> {
        let service = match config {
            ServerConfig::Stdio { command, args, env } => {
                let mut cmd = tokio::process::Command::new(command);
                cmd.args(args).envs(env);
                #[cfg(windows)]
                cmd.creation_flags(0x0800_0000);
                let transport = TokioChildProcess::new(cmd).map_err(|e| format!("could not start {command}: {e}"))?;
                ().serve(transport).await.map_err(|e| e.to_string())?
            }
            ServerConfig::Http { url, headers } => {
                let mut config = StreamableHttpClientTransportConfig::with_uri(url.as_str());
                let mut custom = HashMap::new();
                for (name, value) in headers {
                    if name.eq_ignore_ascii_case("authorization") {
                        config = config.auth_header(value.trim_start_matches("Bearer ").to_string());
                        continue;
                    }
                    let (Ok(name), Ok(value)) = (name.parse::<http::HeaderName>(), value.parse::<http::HeaderValue>()) else { continue };
                    custom.insert(name, value);
                }
                config = config.custom_headers(custom);
                let transport = StreamableHttpClientTransport::with_client(crate::llm::http::client(), config);
                ().serve(transport).await.map_err(|e| e.to_string())?
            }
        };
        let tools = service.list_all_tools().await.map_err(|e| format!("tools/list failed: {e}"))?;
        Ok(Live { service, tools })
    }

    /// Also invalidates any connect still in flight for this server.
    pub fn invalidate(&self, name: &str) {
        *self.generation.lock().unwrap().entry(name.into()).or_default() += 1;
    }

    fn current_generation(&self, name: &str) -> u64 {
        self.generation.lock().unwrap().get(name).copied().unwrap_or(0)
    }

    /// Waits, at most `limit`, until no connect is in flight.
    pub async fn wait_ready(&self, limit: Duration) {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let settled = self.settled.notified();
            if !self.transient.lock().unwrap().values().any(|(state, _)| *state == State::Connecting) {
                return;
            }
            tokio::select! {
                () = settled => {}
                () = tokio::time::sleep_until(deadline) => return,
            }
        }
    }

    /// The server's connection ended without anyone asking: it is dropped and marked reconnecting, unless it changed meanwhile.
    fn drop_if_closed(&self, name: &str, generation: u64) -> bool {
        let mut live = self.live.lock().unwrap();
        let closed = live.get(name).is_some_and(|l| l.service.is_transport_closed() || l.service.is_closed());
        if !closed || self.current_generation(name) != generation {
            return false;
        }
        live.remove(name);
        drop(live);
        self.set_transient(name, State::Connecting, Some("the connection closed; reconnecting".into()));
        true
    }

    pub async fn disconnect(&self, name: &str, store: &Store, hub: &Hub) -> bool {
        self.invalidate(name);
        let removed = self.live.lock().unwrap().remove(name);
        self.transient.lock().unwrap().remove(name);
        let Some(live) = removed else { return false };
        if let Ok(live) = Arc::try_unwrap(live) {
            let _ = live.service.cancel().await;
        }
        if let Ok(Some(row)) = store.mcp_server(name) {
            hub.publish(Event::McpUpdated { server: self.status_of(row) });
        }
        true
    }

    fn set_transient(&self, name: &str, state: State, error: Option<String>) {
        self.transient.lock().unwrap().insert(name.into(), (state, error));
    }

    /// Every tool of every connected server, named `server_tool` so the model can tell them apart.
    pub fn tools(&self) -> Vec<Arc<dyn crate::tool::Tool>> {
        let live = self.live.lock().unwrap();
        live.iter()
            .flat_map(|(server, live)| {
                live.tools.iter().map(move |tool| Arc::new(McpTool::new(server, tool.clone(), live.clone())) as Arc<dyn crate::tool::Tool>)
            })
            .collect()
    }
}

impl crate::Engine {
    /// Connects a server, offers its tools to later turns, and watches it for as long as its definition stands.
    pub async fn connect_mcp(self: &Arc<Self>, row: ServerRow) -> Result<(), String> {
        let name = row.name.clone();
        let connected = self.mcp.connect(row, &self.hub).await;
        if connected.is_ok() {
            self.watch_mcp(name);
        }
        connected
    }

    /// Connects every enabled, approved server that is not already live.
    pub async fn connect_all_mcp(self: &Arc<Self>) {
        let Ok(rows) = self.store.mcp_servers() else { return };
        for row in rows.into_iter().filter(|r| r.enabled && r.is_approved()) {
            if !self.mcp.live.lock().unwrap().contains_key(&row.name) {
                let _ = self.connect_mcp(row).await;
            }
        }
    }

    /// Reconnects a connection that ends by itself; any change to the server's generation ends the watch.
    fn watch_mcp(self: &Arc<Self>, name: String) {
        let generation = self.mcp.current_generation(&name);
        let engine = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(WATCH_INTERVAL).await;
                let Some(engine) = engine.upgrade() else { return };
                if engine.mcp.current_generation(&name) != generation {
                    return;
                }
                if engine.mcp.drop_if_closed(&name, generation) {
                    engine.lost_mcp(&name);
                    return engine.reconnect_mcp(name, generation).await;
                }
            }
        });
    }

    fn lost_mcp(&self, name: &str) {
        if let Ok(Some(row)) = self.store.mcp_server(name) {
            self.hub.publish(Event::McpUpdated { server: self.mcp.status_of(row) });
        }
    }

    /// Tries again with growing waits until it connects or the server's definition changes.
    async fn reconnect_mcp(self: Arc<Self>, name: String, generation: u64) {
        let engine = Arc::downgrade(&self);
        drop(self);
        let mut wait = FIRST_RETRY;
        loop {
            tokio::time::sleep(wait).await;
            let Some(engine) = engine.upgrade() else { return };
            let current = engine.store.mcp_server(&name).ok().flatten().filter(|row| row.enabled && row.is_approved());
            let Some(row) = current.filter(|_| engine.mcp.current_generation(&name) == generation) else { return };
            if engine.connect_mcp(row).await.is_ok() {
                return;
            }
            wait = (wait * 2).min(MAX_RETRY);
        }
    }
}

impl Live {
    async fn call(&self, name: &str, arguments: serde_json::Value) -> Result<(String, bool), String> {
        let arguments = arguments.as_object().cloned();
        let mut params = CallToolRequestParams::new(name.to_string());
        params.arguments = arguments;
        let result = self.service.call_tool(params).await.map_err(|e| e.to_string())?;
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
