//! The Drift engine: sessions, providers, tools and the API that serves them.

pub mod api;
pub mod config;
pub mod edit;
pub mod event;
pub mod id;
pub mod llm;
pub mod mcp;
pub mod permission;
pub mod platform;
pub mod question;
pub mod session;
pub mod store;
pub mod tool;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use event::Hub;
use llm::catalog::Catalog;
use llm::credentials::Credentials;
use permission::{Permissions, Policy};
use session::snapshot::Snapshots;
use session::turn::Turns;
use store::Store;
use tool::Registry;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
pub struct Options {
    /// Events kept for cursor replay before a reconnecting client must hydrate instead.
    pub event_history: usize,
    /// Keep secrets in a file under the data dir instead of the OS keychain; tests and CI want this.
    pub file_credentials: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { event_history: 4096, file_credentials: false }
    }
}

#[derive(Debug)]
pub enum Error {
    Store(rusqlite::Error),
    Io(std::io::Error),
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(error) => write!(f, "store: {error}"),
            Self::Io(error) => write!(f, "io: {error}"),
        }
    }
}

impl std::error::Error for Error {}

pub struct Engine {
    pub data_dir: PathBuf,
    pub store: Arc<Store>,
    pub hub: Hub,
    /// Every request must present this; the shell hands it to the UI, remote clients get it via the gateway.
    pub token: String,
    pub permissions: Permissions,
    pub questions: question::Questions,
    pub tools: Registry,
    pub mcp: mcp::Servers,
    pub credentials: Credentials,
    pub catalog: RwLock<Catalog>,
    pub snapshots: Snapshots,
    pub turns: Turns,
    pub http: reqwest::Client,
    /// Sign-in flows waiting for their callback code, keyed by state.
    pub oauth: std::sync::Mutex<std::collections::HashMap<String, String>>,
    /// The user's per-agent model and prompt choices from Settings; the shell sets them.
    agent_overrides: RwLock<std::collections::HashMap<String, config::AgentOverride>>,
}

impl Engine {
    pub fn open(data_dir: &Path) -> Result<Arc<Self>, Error> {
        Self::open_with(data_dir, Options::default())
    }

    pub fn open_with(data_dir: &Path, options: Options) -> Result<Arc<Self>, Error> {
        let store = Arc::new(store::open(data_dir)?);
        store.abandon_streaming_messages()?;
        Ok(Arc::new(Self {
            data_dir: data_dir.to_path_buf(),
            store,
            hub: Hub::new(options.event_history),
            token: random_hex(32),
            permissions: Permissions::new(Policy::default()),
            questions: question::Questions::default(),
            tools: Registry::builtin(),
            mcp: mcp::Servers::default(),
            credentials: Credentials::open(data_dir, options.file_credentials),
            catalog: RwLock::new(Catalog::load(data_dir)),
            snapshots: Snapshots::new(data_dir),
            turns: Turns::default(),
            http: reqwest::Client::new(),
            oauth: Default::default(),
            agent_overrides: Default::default(),
        }))
    }

    pub fn set_agent_overrides(&self, overrides: std::collections::HashMap<String, config::AgentOverride>) {
        *self.agent_overrides.write().unwrap() = overrides;
    }

    /// The workspace's agents, commands and skills with the user's Settings overrides applied.
    pub fn workspace_config(&self, workspace: &Path) -> config::Config {
        let mut config = config::Config::load(workspace);
        config.apply_overrides(&self.agent_overrides.read().unwrap());
        config
    }

    /// Pulls a fresh catalog from models.dev when the cached one is stale; the bundled snapshot covers failure.
    pub async fn refresh_catalog(&self) {
        if Catalog::cache_is_fresh(&self.data_dir) {
            return;
        }
        if let Ok(catalog) = Catalog::refresh(&self.http, &self.data_dir).await {
            *self.catalog.write().unwrap() = catalog;
            self.hub.publish(event::Event::CatalogUpdated {});
        }
    }
}

pub(crate) fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer).expect("system random source unavailable");
    buffer.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub struct Server {
    pub addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn stop(&self) {
        self.task.abort();
    }
}

pub async fn listen(engine: Arc<Engine>, addr: SocketAddr) -> Result<Server, Error> {
    let starting = engine.clone();
    tokio::spawn(async move {
        starting.refresh_catalog().await;
        starting.mcp.connect_all(&starting.store, &starting.hub).await;
        starting.tools.set_dynamic(starting.mcp.tools());
    });
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let addr = listener.local_addr()?;
    let router = api::router(engine);
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(Server { addr, task })
}
