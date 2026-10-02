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
    /// Background workers' slots and their owners' stop scopes.
    pub workers: session::tasks::Workers,
    pub http: reqwest::Client,
    /// Sign-in flows waiting for their callback code, keyed by state.
    pub oauth: std::sync::Mutex<std::collections::HashMap<String, String>>,
    /// The user's per-agent model and prompt choices from Settings; the shell sets them.
    agent_overrides: RwLock<std::collections::HashMap<String, config::AgentOverride>>,
    /// The user's shell time limit from Settings, `Some(None)` for none; `None` until the shell says.
    shell_timeout: RwLock<Option<Option<std::time::Duration>>>,
    /// The runtime the engine serves on, for work started from outside it (a Settings change from the shell).
    runtime: std::sync::OnceLock<tokio::runtime::Handle>,
    /// What each local server last reported, kept across catalog refreshes.
    local_models: std::sync::Mutex<std::collections::BTreeMap<String, Vec<llm::catalog::Model>>>,
}

impl Engine {
    pub fn open(data_dir: &Path) -> Result<Arc<Self>, Error> {
        Self::open_with(data_dir, Options::default())
    }

    pub fn open_with(data_dir: &Path, options: Options) -> Result<Arc<Self>, Error> {
        let store = Arc::new(store::open(data_dir)?);
        store.abandon_streaming_messages()?;
        store.interrupt_unfinished_tasks()?;
        tool::stage::recover_leftovers(&store);
        let credentials = Credentials::open(data_dir, options.file_credentials);
        let catalog = with_user_providers(Catalog::load(data_dir), &credentials);
        Ok(Arc::new(Self {
            data_dir: data_dir.to_path_buf(),
            store,
            hub: Hub::new(options.event_history),
            token: random_hex(32),
            permissions: Permissions::new(Policy::default()),
            questions: question::Questions::default(),
            tools: Registry::builtin(),
            mcp: mcp::Servers::default(),
            credentials,
            catalog: RwLock::new(catalog),
            snapshots: Snapshots::new(data_dir),
            turns: Turns::default(),
            workers: Default::default(),
            http: llm::http::client(),
            oauth: Default::default(),
            agent_overrides: Default::default(),
            shell_timeout: Default::default(),
            runtime: Default::default(),
            local_models: Default::default(),
        }))
    }

    /// A changed agent model or prompt may be what an owed result was waiting for.
    pub fn set_agent_overrides(self: &Arc<Self>, overrides: std::collections::HashMap<String, config::AgentOverride>) {
        *self.agent_overrides.write().unwrap() = overrides;
        self.retry_deliveries(None);
    }

    /// `None` lets shell commands run as long as they need. Applies to calls that start afterwards.
    pub fn set_shell_timeout(&self, timeout: Option<std::time::Duration>) {
        *self.shell_timeout.write().unwrap() = Some(timeout);
    }

    /// How long a shell command may run when the model does not say.
    pub fn shell_timeout(&self) -> Option<std::time::Duration> {
        self.shell_timeout.read().unwrap().unwrap_or(Some(tool::bash::DEFAULT_TIMEOUT))
    }

    /// The workspace's agents, commands and skills with the user's Settings overrides applied.
    pub fn workspace_config(&self, workspace: &Path) -> config::Config {
        let mut config = config::Config::load(workspace);
        config.apply_overrides(&self.agent_overrides.read().unwrap());
        config
    }

    /// Housekeeping at startup and every [`MAINTENANCE_INTERVAL`] after, for as long as the engine
    /// lives: unreferenced snapshot content and old shell output logs go.
    pub async fn maintain(self: Arc<Self>) {
        let engine = Arc::downgrade(&self);
        drop(self);
        let mut every = tokio::time::interval(MAINTENANCE_INTERVAL);
        loop {
            every.tick().await;
            let Some(engine) = engine.upgrade() else { return };
            engine.prune_snapshots().await;
            engine.prune_tool_output(TOOL_OUTPUT_RETENTION);
        }
    }

    /// Deletes spooled shell output older than `age`; a call's result still says what it printed.
    pub fn prune_tool_output(&self, age: std::time::Duration) {
        let root = self.data_dir.join("tool-output");
        let Ok(sessions) = std::fs::read_dir(&root) else { return };
        for session in sessions.flatten() {
            for file in std::fs::read_dir(session.path()).into_iter().flatten().flatten() {
                let old = file.metadata().and_then(|m| m.modified()).is_ok_and(|at| at.elapsed().is_ok_and(|elapsed| elapsed > age));
                if old {
                    let _ = std::fs::remove_file(file.path());
                }
            }
            let _ = std::fs::remove_dir(session.path());
        }
    }

    /// Drops recorded file content no stored call refers to any more; content referenced by any stored
    /// call, archived ones included, is pinned. Failures only mean the store keeps more than it needs.
    pub async fn prune_snapshots(&self) {
        let (Ok(workspaces), Ok(mut blobs)) = (self.store.workspaces(), self.store.recorded_blobs()) else { return };
        for workspace in workspaces {
            let keep = blobs.remove(&workspace.id).unwrap_or_default();
            let path = tool::canonical(Path::new(&workspace.path));
            self.snapshots.bind(&workspace.id, &path);
            let _ = self.snapshots.prune(&path, &keep).await;
        }
    }

    /// Pulls a fresh catalog from models.dev when the cached one is stale; the bundled snapshot covers failure.
    pub async fn refresh_catalog(&self) {
        if Catalog::cache_is_fresh(&self.data_dir) {
            return;
        }
        if let Ok(catalog) = Catalog::refresh(&self.http, &self.data_dir).await {
            let mut catalog = with_user_providers(catalog, &self.credentials);
            for (id, models) in self.local_models.lock().unwrap().iter() {
                catalog.with_local(id, id, models);
            }
            *self.catalog.write().unwrap() = catalog;
            self.hub.publish(event::Event::CatalogUpdated {});
        }
    }

    /// Asks each local server (LM Studio, Ollama) what it has, every [`LOCAL_INTERVAL`] for as long as
    /// the engine lives; one that answers is connected with no key, one that stops is not.
    pub async fn watch_local(self: Arc<Self>) {
        let engine = Arc::downgrade(&self);
        drop(self);
        loop {
            let Some(engine) = engine.upgrade() else { return };
            engine.ask_local().await;
            drop(engine);
            tokio::time::sleep(LOCAL_INTERVAL).await;
        }
    }

    pub(crate) async fn ask_local(&self) {
        let mut changed = false;
        for (id, name, default) in llm::local::LOCAL {
            let base = self.catalog.read().unwrap().providers.get(id).and_then(|p| p.api.clone()).unwrap_or_else(|| default.into());
            let found = llm::local::discover(&self.http, id, &base).await;
            self.credentials.set_keyless(id, found.is_some());
            let mut known = self.local_models.lock().unwrap();
            let was_up = known.contains_key(id);
            match found {
                Some(models) if known.get(id) != Some(&models) => {
                    self.catalog.write().unwrap().with_local(id, name, &models);
                    known.insert(id.into(), models);
                    changed = true;
                }
                None if was_up => {
                    known.remove(id);
                    changed = true;
                }
                _ => {}
            }
        }
        if changed {
            self.hub.publish(event::Event::CatalogUpdated {});
        }
    }
}

/// How often local servers are asked what they have.
const LOCAL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

/// The catalog with the user's own providers laid over it; a new one without a key variable takes none.
fn with_user_providers(catalog: Catalog, credentials: &Credentials) -> Catalog {
    let user = config::user_providers();
    for (id, provider) in &user {
        if !catalog.providers.contains_key(id) && provider.api_key_env.is_none() {
            credentials.set_keyless(id, true);
        }
    }
    catalog.with_user(&user)
}

/// How often housekeeping runs while the engine is up.
pub const MAINTENANCE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);
/// Spooled shell output is kept this long.
const TOOL_OUTPUT_RETENTION: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);

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
    let _ = engine.runtime.set(tokio::runtime::Handle::current());
    let starting = engine.clone();
    tokio::spawn(engine.clone().watch_local());
    tokio::spawn(async move {
        starting.refresh_catalog().await;
        starting.connect_all_mcp().await;
        starting.recover_tasks().await;
        starting.maintain().await;
    });
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let addr = listener.local_addr()?;
    let router = api::router(engine);
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(Server { addr, task })
}
