//! The Drift engine: sessions, providers, tools and the API that serves them.

#![expect(
    clippy::too_many_arguments,
    reason = "parameter structs replace these in the lint pass; remove with it"
)]

pub mod api;
pub mod config;
pub mod edit;
pub mod event;
pub mod hook;
pub mod id;
pub mod llm;
pub mod lsp;
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
        Self {
            event_history: 4096,
            file_credentials: false,
        }
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
    pub credentials: Arc<Credentials>,
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
    /// Ollama models' own windows, asked once per installed build.
    local_shown: llm::local::Shown,
    /// Language servers per project root, started when a file one handles is read or written.
    pub lsp: lsp::Servers,
    /// The user's plugins, loaded at start and on request.
    pub hooks: hook::Hooks,
    /// The engine itself, for work started from a call that only borrows it (a workspace's MCP servers, from planning).
    me: std::sync::Weak<Engine>,
}

/// Where a workspace's "always" permission grants are kept.
fn grants_key(workspace_id: &str) -> String {
    format!("permissionGrants:{workspace_id}")
}

/// The rules the user keeps in Settings, for every workspace, checked after drift.json's.
const PERMISSION_RULES_KEY: &str = "permissionRules";
const AUTO_ACCEPT_ALL_KEY: &str = "autoAcceptAll";
/// Plugins the user switched off in Settings, by their drift.json entry.
const DISABLED_PLUGINS_KEY: &str = "disabledPlugins";
/// Skill folders the user switched off in Settings; the files themselves are never touched.
const DISABLED_SKILLS_KEY: &str = "disabledSkills";
const REGISTRY_SOURCES_KEY: &str = "registrySources";

impl Engine {
    pub fn open(data_dir: &Path) -> Result<Arc<Self>, Error> {
        Self::open_with(data_dir, Options::default())
    }

    pub fn open_with(data_dir: &Path, options: Options) -> Result<Arc<Self>, Error> {
        let store = Arc::new(store::open(data_dir)?);
        store.abandon_streaming_messages()?;
        store.interrupt_unfinished_tasks()?;
        tool::stage::recover_leftovers(&store);
        let _ = std::fs::create_dir_all(std::env::temp_dir().join("Drift"));
        let credentials = Arc::new(Credentials::open(data_dir, options.file_credentials));
        let catalog = with_user_providers(Catalog::load(data_dir), &credentials);
        let permissions = Permissions::new(Policy {
            rules: store.setting(PERMISSION_RULES_KEY)?.unwrap_or_default(),
        });
        let saving = store.clone();
        permissions.save_grants_with(Box::new(move |workspace, grants| {
            let _ = saving.set_setting(&grants_key(workspace), &grants);
        }));
        if store.setting::<bool>(AUTO_ACCEPT_ALL_KEY)?.unwrap_or(false) {
            permissions.set_auto_accept(&Hub::new(0), None, true);
        }
        let mcp = mcp::Servers::new(credentials.clone(), store.clone());
        let background_limit = session::tasks::stored_background_limit(&store);
        Ok(Arc::new_cyclic(|me| Self {
            me: me.clone(),
            data_dir: data_dir.to_path_buf(),
            store,
            hub: Hub::new(options.event_history),
            token: random_hex(32),
            permissions,
            questions: question::Questions::default(),
            tools: Registry::builtin(),
            mcp,
            credentials,
            catalog: RwLock::new(catalog),
            snapshots: Snapshots::new(data_dir),
            turns: Turns::default(),
            workers: session::tasks::Workers::new(background_limit),
            http: llm::http::client(),
            oauth: Default::default(),
            agent_overrides: Default::default(),
            shell_timeout: Default::default(),
            runtime: Default::default(),
            local_models: Default::default(),
            local_shown: Default::default(),
            lsp: Default::default(),
            hooks: Default::default(),
        }))
    }

    /// Reads drift.json again and loads every plugin that is not switched off.
    pub async fn reload_plugins(&self) -> Vec<hook::PluginInfo> {
        let disabled: Vec<String> = self
            .store
            .setting(DISABLED_PLUGINS_KEY)
            .ok()
            .flatten()
            .unwrap_or_default();
        self.hooks
            .load(
                &self.data_dir.join("plugin-cache"),
                config::user_plugins(),
                &disabled,
                self.me.clone(),
            )
            .await
    }

    pub fn registry_sources(&self) -> Vec<config::sources::RegistrySource> {
        let mut sources: Vec<config::sources::RegistrySource> = self
            .store
            .setting(REGISTRY_SOURCES_KEY)
            .ok()
            .flatten()
            .unwrap_or_default();
        for source in &mut sources {
            source.has_token = self
                .credentials
                .secret(&config::sources::token_key(&source.id))
                .is_some();
        }
        sources
    }

    pub fn registry_source(&self, id: &str) -> Option<config::sources::RegistrySource> {
        self.registry_sources().into_iter().find(|source| source.id == id)
    }

    /// Stores the sources and each one's token; a source dropped from the list loses its token too.
    pub fn set_registry_sources(&self, inputs: Vec<config::sources::SourceInput>) -> Result<(), String> {
        let before = self.registry_sources();
        let mut sources = Vec::new();
        for input in inputs {
            let mut source = input.source;
            if source.name.trim().is_empty() || source.url.trim().is_empty() {
                return Err("a registry source needs a name and a location".into());
            }
            if source.id.trim().is_empty() {
                source.id = random_hex(8);
            }
            let is_url = matches!(source.source, config::sources::SourceKind::Url);
            if is_url
                && !source.url.starts_with("https://")
                && !(source.allow_http && source.url.starts_with("http://"))
            {
                return Err(format!(
                    "a URL source needs https (or http allowed for it): {}",
                    source.url
                ));
            }
            match input.token.as_deref().map(str::trim) {
                Some("") => self
                    .credentials
                    .remove_secret(&config::sources::token_key(&source.id))
                    .map_err(|error| error.to_string())?,
                Some(token) => self
                    .credentials
                    .set_secret(&config::sources::token_key(&source.id), token)
                    .map_err(|error| error.to_string())?,
                None => {}
            }
            source.has_token = false;
            sources.push(source);
        }
        for gone in before.iter().filter(|old| !sources.iter().any(|new| new.id == old.id)) {
            let _ = self.credentials.remove_secret(&config::sources::token_key(&gone.id));
        }
        self.store
            .set_setting(REGISTRY_SOURCES_KEY, &sources)
            .map_err(|error| error.to_string())
    }

    pub fn fetcher(&self) -> config::sources::Fetcher {
        config::sources::Fetcher::new(self.http.clone(), self.credentials.clone())
    }

    /// Fetches a registry plugin, checks its hash, lists it in drift.json with its config, and reloads.
    pub async fn install_plugin(&self, install: config::plugins::Install) -> Result<Vec<hook::PluginInfo>, String> {
        let source = install.registry.as_deref().and_then(|id| self.registry_source(id));
        let path = config::plugins::fetch_component(&self.fetcher(), source.as_ref(), &install).await?;
        let dir = config::plugins::config_dir()?;
        config::plugins::edit_plugins(&dir, |plugins| {
            config::plugins::set_entry(plugins, &path, install.config)
        })?;
        Ok(self.reload_plugins().await)
    }

    /// Removes a plugin's drift.json entry and its component, and reloads.
    pub async fn remove_plugin(&self, path: &str) -> Result<Vec<hook::PluginInfo>, String> {
        config::plugins::remove(&config::plugins::config_dir()?, path)?;
        Ok(self.reload_plugins().await)
    }

    /// Replaces a plugin's config in drift.json and reloads, so it reads the new values.
    pub async fn configure_plugin(
        &self,
        path: &str,
        config: serde_json::Value,
    ) -> Result<Vec<hook::PluginInfo>, String> {
        let dir = config::plugins::config_dir()?;
        config::plugins::edit_plugins(&dir, |plugins| config::plugins::set_entry(plugins, path, config))?;
        Ok(self.reload_plugins().await)
    }

    /// Switches a plugin on or off by its drift.json entry and reloads.
    pub async fn set_plugin_enabled(&self, path: &str, enabled: bool) -> rusqlite::Result<Vec<hook::PluginInfo>> {
        let mut disabled: Vec<String> = self.store.setting(DISABLED_PLUGINS_KEY)?.unwrap_or_default();
        disabled.retain(|entry| entry != path);
        if !enabled {
            disabled.push(path.to_owned());
        }
        self.store.set_setting(DISABLED_PLUGINS_KEY, &disabled)?;
        Ok(self.reload_plugins().await)
    }

    /// The skill folders switched off, as the engine compares folders.
    pub fn disabled_skills(&self) -> Vec<PathBuf> {
        self.store
            .setting::<Vec<String>>(DISABLED_SKILLS_KEY)
            .ok()
            .flatten()
            .unwrap_or_default()
            .into_iter()
            .map(PathBuf::from)
            .collect()
    }

    /// Turns a skill on or off for every workspace and session from the next turn; its files stay as they are.
    pub fn set_skill_enabled(&self, folder: &Path, enabled: bool) -> rusqlite::Result<()> {
        let key = folder.to_string_lossy().into_owned();
        let mut disabled: Vec<String> = self.store.setting(DISABLED_SKILLS_KEY)?.unwrap_or_default();
        disabled.retain(|entry| *entry != key);
        if !enabled {
            disabled.push(key);
        }
        self.store.set_setting(DISABLED_SKILLS_KEY, &disabled)
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
        self.shell_timeout
            .read()
            .unwrap()
            .unwrap_or(Some(tool::bash::DEFAULT_TIMEOUT))
    }

    /// Ties a session's permission checks to its workspace's "always" grants, loading them on first use.
    pub(crate) fn bind_permissions(&self, session_id: &str, workspace_id: &str) {
        self.permissions
            .bind(session_id, workspace_id, || self.stored_grants(workspace_id));
    }

    /// Whether every session answers its own asks (Settings), else only those that chose to.
    pub fn auto_accept_all(&self) -> bool {
        self.store.setting(AUTO_ACCEPT_ALL_KEY).ok().flatten().unwrap_or(false)
    }

    /// Auto-accept for one session, stored on it; `None` when there is no such session.
    pub fn set_session_auto_accept(
        &self,
        session_id: &str,
        on: bool,
    ) -> rusqlite::Result<Option<session::types::Session>> {
        let Some(session) = self.store.set_session_auto_accept(session_id, on)? else {
            return Ok(None);
        };
        self.permissions.set_auto_accept(&self.hub, Some(session_id), on);
        Ok(Some(session))
    }

    /// Auto-accept for every session (Settings).
    pub fn set_auto_accept_all(&self, on: bool) -> rusqlite::Result<()> {
        self.store.set_setting(AUTO_ACCEPT_ALL_KEY, &on)?;
        self.permissions.set_auto_accept(&self.hub, None, on);
        Ok(())
    }

    /// A workspace's stored grants, any twin kept by an older build dropped.
    fn stored_grants(&self, workspace_id: &str) -> Vec<permission::Grant> {
        let stored: Vec<permission::Grant> = self
            .store
            .setting(&grants_key(workspace_id))
            .ok()
            .flatten()
            .unwrap_or_default();
        let mut unique: Vec<permission::Grant> = Vec::with_capacity(stored.len());
        for grant in stored {
            if !unique.contains(&grant) {
                unique.push(grant);
            }
        }
        unique
    }

    pub fn permission_grants(&self, workspace_id: &str) -> Vec<permission::Grant> {
        self.permissions
            .grants(workspace_id, || self.stored_grants(workspace_id))
    }

    /// Drops what the engine keeps for a workspace the shell has forgotten: its "always" grants
    /// (stored and cached), the project commands it trusts, and the MCP servers running in it.
    pub fn forget_workspace(&self, workspace_id: &str) -> rusqlite::Result<()> {
        self.stop_workspace_mcp(workspace_id);
        self.store.remove_setting(&grants_key(workspace_id))?;
        self.store.remove_setting(&session::trust::key(workspace_id))?;
        self.permissions.forget_workspace(workspace_id);
        Ok(())
    }

    /// The rules kept in Settings, in the order they are checked.
    pub fn permission_rules(&self) -> Vec<permission::Rule> {
        self.permissions.policy().rules
    }

    /// Replaces the rules kept in Settings; calls checked from now on follow them.
    pub fn set_permission_rules(&self, rules: Vec<permission::Rule>) -> rusqlite::Result<()> {
        self.store.set_setting(PERMISSION_RULES_KEY, &rules)?;
        self.permissions.set_policy(Policy { rules });
        Ok(())
    }

    /// One grant, or all of them with `None`; the stored list is rewritten.
    pub fn revoke_permission_grant(&self, workspace_id: &str, grant: Option<&permission::Grant>) -> bool {
        self.permissions
            .revoke(workspace_id, grant, || self.stored_grants(workspace_id))
    }

    /// The workspace's agents, commands and skills with the user's Settings overrides applied.
    pub fn workspace_config(&self, workspace: &Path) -> config::Config {
        let mut config = config::Config::load_skipping(workspace, &self.disabled_skills());
        config.apply_overrides(&self.agent_overrides.read().unwrap());
        config
    }

    /// Housekeeping at startup and every [`MAINTENANCE_INTERVAL`] after, for as long as the engine
    /// lives: unreferenced snapshot content, old shell output logs and images no call names go.
    pub async fn maintain(self: Arc<Self>) {
        let engine = Arc::downgrade(&self);
        drop(self);
        let mut every = tokio::time::interval(MAINTENANCE_INTERVAL);
        loop {
            every.tick().await;
            let Some(engine) = engine.upgrade() else { return };
            engine.clean_up().await;
        }
    }

    /// One round of housekeeping now (Settings > Storage asks for it); the number of images dropped.
    pub async fn clean_up(&self) -> usize {
        self.prune_snapshots().await;
        self.prune_tool_output(TOOL_OUTPUT_RETENTION);
        self.store.prune_blobs().unwrap_or(0)
    }

    /// Deletes every conversation of a workspace the user removed, with their undo history and shell
    /// output. Refused while the workspace is in use again or one of its conversations is running.
    pub fn purge_removed_workspace(&self, id: &str) -> Result<WorkspacePurge, rusqlite::Error> {
        if self.store.workspace(id)?.is_none() {
            return Ok(WorkspacePurge::Missing);
        }
        let Some(sessions) = self.store.removed_workspace_sessions(id)? else {
            return Ok(WorkspacePurge::InUse);
        };
        if sessions.iter().any(|session| self.turns.is_running(session)) {
            return Ok(WorkspacePurge::Busy);
        }
        let Some(deleted) = self.store.purge_removed_workspace(id)? else {
            return Ok(WorkspacePurge::InUse);
        };
        for session in &sessions {
            self.permissions.forget_session(session);
            self.questions.forget_session(session);
            let _ = std::fs::remove_dir_all(self.data_dir.join("tool-output").join(session));
            self.hub.publish(event::Event::SessionDeleted {
                session_id: session.clone(),
            });
        }
        self.snapshots.forget(id);
        Ok(WorkspacePurge::Purged(deleted))
    }

    /// Deletes spooled shell output older than `age`; a call's result still says what it printed.
    pub fn prune_tool_output(&self, age: std::time::Duration) {
        let root = self.data_dir.join("tool-output");
        let Ok(sessions) = std::fs::read_dir(&root) else { return };
        for session in sessions.flatten() {
            for file in std::fs::read_dir(session.path()).into_iter().flatten().flatten() {
                let old = file
                    .metadata()
                    .and_then(|m| m.modified())
                    .is_ok_and(|at| at.elapsed().is_ok_and(|elapsed| elapsed > age));
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
        let (Ok(workspaces), Ok(mut blobs)) = (self.store.workspaces(), self.store.recorded_blobs()) else {
            return;
        };
        for workspace in workspaces {
            let keep = blobs.remove(&workspace.id).unwrap_or_default();
            let path = tool::canonical(Path::new(&workspace.path));
            self.snapshots.bind(&workspace.id, &path);
            let _ = self.snapshots.prune(&path, &keep).await;
        }
    }

    /// The catalog as the current credentials see it: a ChatGPT sign-in offers only what the Codex backend takes.
    pub fn catalog_view(&self) -> Catalog {
        let mut catalog = self.catalog.read().unwrap().clone();
        if let Some(openai) = catalog.providers.get_mut("openai")
            && matches!(
                self.credentials.resolve("openai", &openai.env),
                Some(llm::Credential::OAuth { .. })
            )
        {
            llm::openai::codex::shape(openai);
        }
        catalog
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
            let base = self
                .catalog
                .read()
                .unwrap()
                .providers
                .get(id)
                .and_then(|p| p.api.clone())
                .unwrap_or_else(|| default.into());
            let found = llm::local::discover(&self.http, id, &base, &self.local_shown).await;
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

/// How a removed workspace's purge went.
#[derive(Debug, PartialEq)]
pub enum WorkspacePurge {
    /// Its conversations are gone, this many.
    Purged(usize),
    /// Not removed, or restored since: nothing was deleted.
    InUse,
    /// One of its conversations is running; try again later.
    Busy,
    Missing,
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

/// How long recovering interrupted tasks waits at startup for MCP servers still connecting.
const RECOVERY_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

pub async fn listen(engine: Arc<Engine>, addr: SocketAddr) -> Result<Server, Error> {
    let _ = engine.runtime.set(tokio::runtime::Handle::current());
    let starting = engine.clone();
    tokio::spawn(engine.clone().watch_local());
    tokio::spawn(engine.clone().stop_idle_mcp());
    tokio::spawn(hook::relay_session_events(engine.clone()));
    tokio::spawn(async move {
        starting.reload_plugins().await;
        starting.connect_all_mcp();
        starting.refresh_catalog().await;
        // Resumed work gets the servers that come up soon, but a dead one never holds it back for long.
        starting.mcp.wait_ready(RECOVERY_WAIT).await;
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
