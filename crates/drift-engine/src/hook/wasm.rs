//! Plugins as sandboxed WebAssembly components, compiled once into a disk cache; the contract is `wit/drift.wit`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::Mutex;
use wasmtime::component::{
    Component, ComponentNamedList, HasSelf, Instance, InstancePre, Lift, Linker, ResourceTable, TypedFunc,
};
use wasmtime::{Cache, CacheConfig, Config, Engine, Store, StoreLimits, StoreLimitsBuilder, UpdateDeadline};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use super::settings::{Choice, Pair, Setting, SettingKind};
use super::{
    AfterTool, BeforeTool, Compacting, CompactionEvent, Grammar, Hook, LanguageServer, PermissionAsk,
    PermissionDecision, PromptEvent, PromptSubmit, ReplyEvent, SessionEvent, SessionKind, ToolAnswer, ToolCall,
    ToolRequest, ToolResult, TurnEnd,
};
use crate::websearch::{Hit, SearchError};

mod bindings {
    wasmtime::component::bindgen!({
        world: "plugin", path: "wit", imports: { default: async }, exports: { default: async }
    });
}
mod host;

use super::Error;
pub use super::Site;
use bindings::Plugin;
use bindings::drift::plugin::types as wit;

/// A hook call gets this much of its own running time; host calls add theirs. The epoch ticks every 100 ms.
const EPOCH_TICK: Duration = Duration::from_millis(100);
const CALL_BUDGET: Duration = Duration::from_secs(5);
/// However long its host calls take, no call runs past this.
const CALL_CEILING: Duration = Duration::from_secs(120);
const MEMORY_BYTES: usize = 512 * 1024 * 1024;
const WASM_PACKAGE: &str = "drift:plugin/";

struct State {
    name: String,
    site: Site,
    /// The workspace of the event being handled, where files and processes are scoped.
    workspace: PathBuf,
    /// When the current call's time is up; host calls move it on by what they took, never past `ceiling`.
    deadline: Instant,
    ceiling: Instant,
    /// What the plugin declared, so `host.config` fills in their defaults.
    settings: Vec<Setting>,
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
}

impl State {
    fn new(name: String, site: Site, settings: Vec<Setting>) -> Self {
        Self {
            name,
            site,
            workspace: PathBuf::new(),
            deadline: Instant::now() + CALL_BUDGET,
            ceiling: Instant::now() + CALL_CEILING,
            settings,
            wasi: WasiCtxBuilder::new().inherit_stderr().build(),
            table: ResourceTable::new(),
            limits: StoreLimitsBuilder::new().memory_size(MEMORY_BYTES).build(),
        }
    }
}

/// A store whose calls trap once their time is up.
fn timed_store(engine: &Engine, state: State) -> Store<State> {
    let mut store = Store::new(engine, state);
    store.limiter(|state| &mut state.limits);
    store.set_epoch_deadline(1);
    store.epoch_deadline_callback(|store| {
        if Instant::now() < store.data().deadline {
            Ok(UpdateDeadline::Continue(1))
        } else {
            Err(wasmtime::Error::msg("the plugin ran past its time"))
        }
    });

    store
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

/// One compiler and linker for every plugin; compiled code is cached under `cache_dir`.
pub struct Runtime {
    engine: Engine,
    linker: Linker<State>,
}

impl Runtime {
    pub fn new(cache_dir: &Path) -> Result<Self, Error> {
        let mut config = Config::new();
        config.epoch_interruption(true);

        let mut cache = CacheConfig::new();
        cache.with_directory(cache_dir);
        config.cache(Some(
            Cache::new(cache).map_err(|error| Error::Cache(error.to_string()))?,
        ));

        let engine = Engine::new(&config).map_err(|error| Error::Runtime(error.to_string()))?;
        let ticker = engine.clone();
        std::thread::Builder::new()
            .name("drift-plugin-epoch".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(EPOCH_TICK);
                    ticker.increment_epoch();
                }
            })
            .map_err(|error| Error::Runtime(error.to_string()))?;

        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(|error| Error::Runtime(error.to_string()))?;
        Plugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)
            .map_err(|error| Error::Runtime(error.to_string()))?;

        Ok(Self { engine, linker })
    }

    pub async fn load(&self, path: &Path, site: Site) -> Result<WasmPlugin, Error> {
        let engine = self.engine.clone();
        let file = path.to_path_buf();
        let component = tokio::task::spawn_blocking(move || Component::from_file(&engine, &file))
            .await
            .map_err(|error| Error::CompileTask(error.to_string()))?
            .map_err(|error| Error::Compile(error.to_string()))?;

        let mut capabilities = capabilities(&self.engine, &component);
        let pre = self
            .linker
            .instantiate_pre(&component)
            .map_err(|error| Error::Instantiate(error.to_string()))?;
        let mut live = Live::start(&pre, State::new(file_name(path), site, Vec::new()))
            .await
            .map_err(|error| Error::Instantiate(error.to_string()))?;
        let name = within(live.store.data().ceiling, live.bindings.call_name(&mut live.store))
            .await
            .map_err(|error| Error::Name(error.to_string()))?;
        if name.trim().is_empty() {
            return Err(Error::EmptyName);
        }

        live.store.data_mut().name.clone_from(&name);
        let settings: Vec<Setting> = read::<Vec<wit::Setting>>(&mut live, &pre, "settings")
            .await?
            .into_iter()
            .map(Setting::from)
            .collect();
        live.store.data_mut().settings.clone_from(&settings);
        let replacements = read(&mut live, &pre, "replacements").await?;
        let search_providers = if live.search.is_some() {
            read(&mut live, &pre, "search-providers").await?
        } else {
            Vec::new()
        };
        let language_servers: Vec<LanguageServer> =
            read::<Vec<wit::LanguageServer>>(&mut live, &pre, "language-servers")
                .await?
                .into_iter()
                .map(LanguageServer::from)
                .collect();
        let grammars = grammars(&name, read(&mut live, &pre, "grammars").await?);
        let tools = if live.call_tool.is_some() {
            tool_specs(&name, read(&mut live, &pre, "tools").await?)
        } else {
            Vec::new()
        };
        // Drift runs these programs for it and offers these tools, so Settings says so beside the host calls it makes.
        if !language_servers.is_empty() {
            capabilities.push("lsp".to_owned());
        }
        if !tools.is_empty() {
            capabilities.push("tools".to_owned());
        }
        capabilities.sort();

        Ok(WasmPlugin {
            name,
            path: path.to_path_buf(),
            capabilities,
            settings,
            replacements,
            search_providers,
            language_servers,
            grammars,
            tools,
            pre,
            ceiling: CALL_CEILING,
            live: Mutex::new(live),
        })
    }
}

/// The call, failed once the wall clock passes `ceiling` whatever it waits on; wasmtime unwinds a dropped call.
async fn within<T>(
    ceiling: Instant,
    call: impl std::future::Future<Output = wasmtime::Result<T>>,
) -> wasmtime::Result<T> {
    tokio::time::timeout_at(ceiling.into(), call)
        .await
        .unwrap_or_else(|_| Err(wasmtime::Error::msg("the plugin waited past its time")))
}

/// A load-time export's answer, nothing if absent or failed; a failure restarts the store before it goes live.
async fn read<T>(live: &mut Live, pre: &InstancePre<State>, export: &str) -> Result<T, Error>
where
    T: Default + Send + Sync + 'static,
    (T,): ComponentNamedList + Lift,
{
    let Ok(func) = live.instance.get_typed_func::<(), (T,)>(&mut live.store, export) else {
        return Ok(T::default());
    };

    match within(live.store.data().ceiling, func.call_async(&mut live.store, ())).await {
        Ok((value,)) => Ok(value),
        Err(error) => {
            eprintln!("drift: plugin {} failed in {export}: {error}", live.store.data().name);
            live.restart(pre)
                .await
                .map_err(|error| Error::Instantiate(error.to_string()))?;
            Ok(T::default())
        }
    }
}

impl From<wit::Setting> for Setting {
    fn from(setting: wit::Setting) -> Self {
        Self {
            key: setting.key,
            label: setting.label,
            description: setting.description,
            kind: setting.kind.into(),
        }
    }
}

impl From<wit::SettingKind> for SettingKind {
    fn from(kind: wit::SettingKind) -> Self {
        match kind {
            wit::SettingKind::Text(text) => Self::Text {
                default: text.default,
                placeholder: text.placeholder,
                multiline: text.multiline,
                secret: text.secret,
            },
            wit::SettingKind::Number(number) => Self::Number {
                default: number.default,
                min: number.min,
                max: number.max,
                integer: number.integer,
            },
            wit::SettingKind::Toggle(default) => Self::Toggle { default },
            wit::SettingKind::Choice(choice) => Self::Choice {
                default: choice.default,
                choices: choice
                    .choices
                    .into_iter()
                    .map(|(value, label)| Choice { value, label })
                    .collect(),
            },
            wit::SettingKind::Items(items) => Self::Items {
                default: items.default,
                placeholder: items.placeholder,
                secret: items.secret,
            },
            wit::SettingKind::Pairs(pairs) => Self::Pairs {
                default: pairs
                    .default
                    .into_iter()
                    .map(|(key, value)| Pair { key, value })
                    .collect(),
                key_label: pairs.key_label,
                value_label: pairs.value_label,
                secret_keys: pairs.secret_keys,
                secret_values: pairs.secret_values,
            },
        }
    }
}

/// A search answer across the boundary: the hits, or why not and, when rate limited, the seconds to wait.
type Searched = (Result<Vec<(String, String, String)>, (String, Option<u32>)>,);
type SearchFunc = TypedFunc<(String, String, u32), Searched>;
type CallToolFunc = TypedFunc<(wit::ToolRequest,), (wit::ToolAnswer,)>;

/// The `languages` world's grammars, a grammar that is not one logged and left out.
fn grammars(name: &str, declared: Vec<wit::Grammar>) -> Vec<Grammar> {
    declared
        .into_iter()
        .filter_map(|grammar| {
            let label = grammar.name.clone();
            Grammar::parse(name, grammar.name, grammar.aliases, grammar.extensions, &grammar.json)
                .inspect_err(|why| eprintln!("drift: plugin {name}'s grammar {label} {why}"))
                .ok()
        })
        .collect()
}

/// The `tooling` world's tools, a tool whose parameters are not JSON logged and left out.
fn tool_specs(name: &str, declared: Vec<wit::ToolSpec>) -> Vec<crate::llm::ToolSpec> {
    declared
        .into_iter()
        .filter_map(|tool| match serde_json::from_str(&tool.parameters) {
            Ok(input_schema) => Some(crate::llm::ToolSpec {
                name: tool.name,
                description: tool.description,
                input_schema,
            }),
            Err(error) => {
                eprintln!(
                    "drift: plugin {name}'s tool {} has parameters that are not JSON: {error}",
                    tool.name
                );
                None
            }
        })
        .collect()
}

impl From<wit::LanguageServer> for LanguageServer {
    fn from(server: wit::LanguageServer) -> Self {
        Self {
            id: server.id,
            command: server.command,
            extensions: server.extensions,
            language: server.language,
            label: server.label,
            setup: match server.setup {
                wit::ServerSetup::Install(install) => crate::hook::ServerSetup::Install {
                    commands: install.commands,
                    guide: install.guide,
                },
                wit::ServerSetup::Guide(guide) => crate::hook::ServerSetup::Guide(guide),
                wit::ServerSetup::None(why) => crate::hook::ServerSetup::None(why),
            },
        }
    }
}

fn capabilities(engine: &Engine, component: &Component) -> Vec<String> {
    let mut found: Vec<String> = component
        .component_type()
        .imports(engine)
        .filter_map(|(name, _)| name.strip_prefix(WASM_PACKAGE))
        .map(|rest| rest.split('@').next().unwrap_or(rest).to_owned())
        .filter(|name| name != "host" && name != "types")
        .collect();
    found.sort();

    found
}

fn file_name(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// One loaded component; its calls run one at a time, each with a fresh time budget.
pub struct WasmPlugin {
    name: String,
    pub path: PathBuf,
    pub capabilities: Vec<String>,
    settings: Vec<Setting>,
    /// Secret and placeholder pairs from the `masking` world, read once at load.
    replacements: Vec<(String, String)>,
    /// Web search providers from the `searching` world, read once at load.
    search_providers: Vec<(String, String)>,
    /// Language servers and grammars from the `languages` world, read once at load.
    language_servers: Vec<LanguageServer>,
    grammars: Vec<Grammar>,
    /// Tools from the `tooling` world, read once at load.
    tools: Vec<crate::llm::ToolSpec>,
    /// The compiled and linked component, to start afresh from after a trap.
    pre: InstancePre<State>,
    /// How long one call may take by the wall clock, host calls and waits included.
    ceiling: Duration,
    live: Mutex<Live>,
}

/// The running instance and the exports looked up in it, which belong to that store alone.
struct Live {
    store: Store<State>,
    instance: Instance,
    bindings: Plugin,
    search: Option<SearchFunc>,
    call_tool: Option<CallToolFunc>,
}

impl Live {
    /// A fresh store and instance from the linked component, with every export it has looked up in that store.
    async fn start(pre: &InstancePre<State>, state: State) -> wasmtime::Result<Self> {
        let mut store = timed_store(pre.engine(), state);
        let instance = within(store.data().ceiling, pre.instantiate_async(&mut store)).await?;
        let bindings = Plugin::new(&mut store, &instance)?;
        let search = instance.get_typed_func(&mut store, "search").ok();
        let call_tool = instance.get_typed_func(&mut store, "call-tool").ok();

        Ok(Self {
            store,
            instance,
            bindings,
            search,
            call_tool,
        })
    }

    /// Starts afresh keeping what was read at load, since wasmtime refuses every call into a trapped store.
    async fn restart(&mut self, pre: &InstancePre<State>) -> wasmtime::Result<()> {
        let old = self.store.data();
        let site = Site {
            entry: old.site.entry.clone(),
            config: old.site.config.clone(),
            engine: old.site.engine.clone(),
        };
        let state = State::new(old.name.clone(), site, old.settings.clone());

        *self = Self::start(pre, state).await?;
        Ok(())
    }
}

impl WasmPlugin {
    /// Logs a failed or timed out call and starts the plugin afresh.
    async fn failed(&self, live: &mut Live, what: &str, error: &wasmtime::Error) {
        eprintln!("drift: plugin {} failed in {what}: {error}", self.name);
        match live.restart(&self.pre).await {
            Ok(()) => eprintln!("drift: plugin {} restarted", self.name),
            Err(error) => eprintln!("drift: plugin {} could not restart: {error}", self.name),
        }
    }

    /// The instance ready for one call: scoped to the event's workspace, with a fresh budget.
    async fn enter(&self, workspace: &str) -> tokio::sync::MutexGuard<'_, Live> {
        let mut live = self.live.lock().await;
        let state = live.store.data_mut();
        let now = Instant::now();
        state.workspace = PathBuf::from(workspace);
        state.deadline = now + CALL_BUDGET;
        state.ceiling = now + self.ceiling;

        live
    }
}

#[async_trait]
impl Hook for WasmPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn settings(&self) -> Vec<Setting> {
        self.settings.clone()
    }

    fn replacements(&self) -> Vec<(String, String)> {
        self.replacements.clone()
    }

    fn search_providers(&self) -> Vec<(String, String)> {
        self.search_providers.clone()
    }

    fn language_servers(&self) -> Vec<LanguageServer> {
        self.language_servers.clone()
    }

    fn grammars(&self) -> Vec<Grammar> {
        self.grammars.clone()
    }

    async fn search(&self, provider: &str, query: &str, limit: u32) -> Result<Vec<Hit>, SearchError> {
        let mut live = self.enter("").await;
        let live = &mut *live;
        let Some(search) = live.search else {
            return Err(SearchError::Failed(format!("{} does not search", self.name)));
        };
        let answer = within(
            live.store.data().ceiling,
            search.call_async(&mut live.store, (provider.to_string(), query.to_string(), limit)),
        )
        .await;

        match answer {
            Ok((Ok(hits),)) => Ok(hits
                .into_iter()
                .map(|(url, title, snippet)| Hit {
                    url,
                    title,
                    snippet,
                    content: None,
                })
                .collect()),
            Ok((Err((why, Some(seconds))),)) => {
                eprintln!("drift: plugin {} is rate limited: {why}", self.name);
                Err(SearchError::RateLimited(Some(Duration::from_secs(seconds.into()))))
            }
            Ok((Err((why, None)),)) => Err(SearchError::Failed(why)),
            Err(error) => {
                self.failed(live, "search", &error).await;
                Err(SearchError::Failed(error.to_string()))
            }
        }
    }

    fn tools(&self) -> Vec<crate::llm::ToolSpec> {
        self.tools.clone()
    }

    async fn call_tool(&self, request: &ToolRequest) -> ToolAnswer {
        let mut live = self.enter(&request.workspace).await;
        let live = &mut *live;
        let Some(call_tool) = live.call_tool else {
            return ToolAnswer::Failed(format!("{} has no tools", self.name));
        };
        let input = wit::ToolRequest {
            session_id: request.session_id.clone(),
            workspace: request.workspace.clone(),
            agent: request.agent.clone(),
            tool: request.tool.clone(),
            input: request.input.to_string(),
            waited_ms: request.waited_ms,
            worker: request.worker,
        };

        match within(
            live.store.data().ceiling,
            call_tool.call_async(&mut live.store, (input,)),
        )
        .await
        {
            Ok((wit::ToolAnswer::Done(text),)) => ToolAnswer::Done(text),
            Ok((wit::ToolAnswer::Failed(why),)) => ToolAnswer::Failed(why),
            Ok((wit::ToolAnswer::Wait(pending),)) => ToolAnswer::Wait {
                retry_ms: pending.retry_ms,
                status: pending.status,
            },
            Err(error) => {
                self.failed(live, "call-tool", &error).await;
                ToolAnswer::Failed(format!("the {} plugin failed: {error}", self.name))
            }
        }
    }

    async fn before_tool(&self, call: &ToolCall) -> BeforeTool {
        let input = wit::ToolCall {
            session_id: call.session_id.clone(),
            workspace: call.workspace.clone(),
            agent: call.agent.clone(),
            tool: call.tool.clone(),
            input: call.input.to_string(),
        };
        let mut live = self.enter(&call.workspace).await;
        let live = &mut *live;

        match within(
            live.store.data().ceiling,
            live.bindings.call_before_tool(&mut live.store, &input),
        )
        .await
        {
            Ok(wit::BeforeTool::Allow) => BeforeTool::Allow,
            Ok(wit::BeforeTool::Deny(reason)) => BeforeTool::Deny(reason),
            Ok(wit::BeforeTool::Replace(json)) => match serde_json::from_str(&json) {
                Ok(input) => BeforeTool::Replace(input),
                Err(error) => {
                    eprintln!(
                        "drift: plugin {} replaced a tool input with something that is not JSON: {error}",
                        self.name
                    );
                    BeforeTool::Allow
                }
            },
            Err(error) => {
                self.failed(live, "before-tool", &error).await;
                BeforeTool::Allow
            }
        }
    }

    async fn after_tool(&self, result: &ToolResult) -> AfterTool {
        let input = wit::ToolResult {
            session_id: result.session_id.clone(),
            workspace: result.workspace.clone(),
            agent: result.agent.clone(),
            tool: result.tool.clone(),
            input: result.input.to_string(),
            output: result.output.clone(),
            failed: result.failed,
        };
        let mut live = self.enter(&result.workspace).await;
        let live = &mut *live;

        match within(
            live.store.data().ceiling,
            live.bindings.call_after_tool(&mut live.store, &input),
        )
        .await
        {
            Ok(wit::AfterTool::Keep) => AfterTool::Keep,
            Ok(wit::AfterTool::Replace(output)) => AfterTool::Replace(output),
            Ok(wit::AfterTool::Note(note)) => AfterTool::Note(note),
            Err(error) => {
                self.failed(live, "after-tool", &error).await;
                AfterTool::Keep
            }
        }
    }

    async fn prompt_submit(&self, prompt: &PromptEvent) -> PromptSubmit {
        let input = wit::Prompt {
            session_id: prompt.session_id.clone(),
            workspace: prompt.workspace.clone(),
            agent: prompt.agent.clone(),
            text: prompt.text.clone(),
        };
        let mut live = self.enter(&prompt.workspace).await;
        let live = &mut *live;

        match within(
            live.store.data().ceiling,
            live.bindings.call_prompt_submit(&mut live.store, &input),
        )
        .await
        {
            Ok(wit::PromptSubmit::Keep) => PromptSubmit::Keep,
            Ok(wit::PromptSubmit::Replace(text)) => PromptSubmit::Replace(text),
            Ok(wit::PromptSubmit::AddContext(text)) => PromptSubmit::AddContext(text),
            Ok(wit::PromptSubmit::Deny(reason)) => PromptSubmit::Deny(reason),
            Err(error) => {
                self.failed(live, "prompt-submit", &error).await;
                PromptSubmit::Keep
            }
        }
    }

    async fn turn_end(&self, reply: &ReplyEvent) -> TurnEnd {
        let input = wit::Reply {
            session_id: reply.session_id.clone(),
            workspace: reply.workspace.clone(),
            agent: reply.agent.clone(),
            text: reply.text.clone(),
        };
        let mut live = self.enter(&reply.workspace).await;
        let live = &mut *live;

        match within(
            live.store.data().ceiling,
            live.bindings.call_turn_end(&mut live.store, &input),
        )
        .await
        {
            Ok(wit::TurnEnd::Accept) => TurnEnd::Accept,
            Ok(wit::TurnEnd::Note(note)) => TurnEnd::Note(note),
            Ok(wit::TurnEnd::Continue(reason)) => TurnEnd::Continue(reason),
            Err(error) => {
                self.failed(live, "turn-end", &error).await;
                TurnEnd::Accept
            }
        }
    }

    async fn permission(&self, ask: &PermissionAsk) -> PermissionDecision {
        let input = wit::PermissionAsk {
            session_id: ask.session_id.clone(),
            workspace: ask.workspace.clone(),
            agent: ask.agent.clone(),
            tool: ask.tool.clone(),
            kind: ask.kind.clone(),
            pattern: ask.pattern.clone(),
            title: ask.title.clone(),
            commands: ask.commands.clone(),
        };
        let mut live = self.enter(&ask.workspace).await;
        let live = &mut *live;

        match within(
            live.store.data().ceiling,
            live.bindings.call_permission(&mut live.store, &input),
        )
        .await
        {
            Ok(wit::Permission::Pass) => PermissionDecision::Pass,
            Ok(wit::Permission::Allow) => PermissionDecision::Allow,
            Ok(wit::Permission::Deny(reason)) => PermissionDecision::Deny(reason),
            Err(error) => {
                self.failed(live, "permission", &error).await;
                PermissionDecision::Pass
            }
        }
    }

    async fn compaction(&self, event: &CompactionEvent) -> Compacting {
        let input = wit::Compaction {
            session_id: event.session_id.clone(),
            workspace: event.workspace.clone(),
            agent: event.agent.clone(),
        };
        let mut live = self.enter(&event.workspace).await;
        let live = &mut *live;

        match within(
            live.store.data().ceiling,
            live.bindings.call_compaction(&mut live.store, &input),
        )
        .await
        {
            Ok(wit::Compacting::Proceed) => Compacting::Proceed,
            Ok(wit::Compacting::Instruct(text)) => Compacting::Instruct(text),
            Err(error) => {
                self.failed(live, "compaction", &error).await;
                Compacting::Proceed
            }
        }
    }

    async fn session(&self, event: &SessionEvent) {
        let session = wit::Session {
            id: event.id.clone(),
            workspace: event.workspace.clone(),
            title: event.title.clone(),
            agent: event.agent.clone(),
        };
        let kind = match event.kind {
            SessionKind::Created => wit::SessionKind::Created,
            SessionKind::Running => wit::SessionKind::Running,
            SessionKind::Idle => wit::SessionKind::Idle,
            SessionKind::Updated => wit::SessionKind::Updated,
            SessionKind::Deleted => wit::SessionKind::Deleted,
            SessionKind::Compacted => wit::SessionKind::Compacted,
        };
        let mut live = self.enter(&event.workspace).await;
        let live = &mut *live;

        if let Err(error) = within(
            live.store.data().ceiling,
            live.bindings.call_session(&mut live.store, &session, kind),
        )
        .await
        {
            self.failed(live, "session", &error).await;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Weak;

    /// The example plugin, built into the workspace's `target/plugins`; `None` without the wasm32-wasip2 target.
    pub(super) fn guard() -> Option<PathBuf> {
        built("guard")
    }

    /// A plugin under `plugins/`, built into the workspace's `target/plugins`; `None` without the wasm32-wasip2 target.
    pub(crate) fn built(name: &str) -> Option<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let target = root.join("target/plugins");
        let built = std::process::Command::new("cargo")
            .args(["build", "--release", "--target", "wasm32-wasip2"])
            .env("CARGO_TARGET_DIR", &target)
            .current_dir(root.join("plugins").join(name))
            .output()
            .expect("cargo runs");
        if !built.status.success() {
            let stderr = String::from_utf8_lossy(&built.stderr);
            assert!(
                stderr.contains("wasm32-wasip2"),
                "{name} plugin failed to build: {stderr}"
            );
            eprintln!("skipping: the wasm32-wasip2 target is not installed");
            return None;
        }
        Some(target.join(format!("wasm32-wasip2/release/{name}.wasm")))
    }

    pub(super) fn site() -> Site {
        Site {
            entry: "plugins/guard.wasm".into(),
            config: serde_json::json!({}),
            engine: Weak::new(),
        }
    }

    fn call(tool: &str, command: &str) -> ToolCall {
        ToolCall {
            session_id: "s1".into(),
            workspace: "C:/work".into(),
            agent: "build".into(),
            tool: tool.into(),
            input: serde_json::json!({ "command": command }),
        }
    }

    #[tokio::test]
    async fn the_secrets_plugin_names_its_pairs_and_a_plain_plugin_names_none() {
        let (Some(secrets), Some(guard)) = (built("secrets"), guard()) else {
            return;
        };
        let cache = crate::platform::temp::test_run().join(format!("drift-plugin-cache-{}", crate::random_hex(4)));
        let runtime = Runtime::new(&cache).unwrap();
        let configured = Site {
            entry: "plugins/secrets.wasm".into(),
            config: serde_json::json!({ "replacements": { "sk-\"quoted\"-1": "API_KEY" } }),
            engine: Weak::new(),
        };

        let plugin = runtime.load(&secrets, configured).await.unwrap();
        assert_eq!(plugin.name(), "secrets");
        assert!(plugin.capabilities.is_empty(), "it reads its config and nothing else");
        assert_eq!(
            plugin.replacements(),
            vec![("sk-\"quoted\"-1".to_owned(), "API_KEY".to_owned())]
        );

        let plain = runtime.load(&guard, site()).await.unwrap();
        assert!(
            plain.replacements().is_empty(),
            "a plugin built before the masking world still loads"
        );
        assert!(plain.settings().is_empty(), "and declares no settings");

        let declared = plugin.settings();
        assert_eq!(declared.len(), 1);
        assert_eq!(declared[0].key, "replacements");
        assert!(matches!(
            &declared[0].kind,
            SettingKind::Pairs { secret_keys: true, secret_values: false, key_label, .. } if key_label == "Secret"
        ));
    }

    #[tokio::test]
    async fn the_roc_plugin_adds_its_grammar_and_its_server_and_has_no_settings() {
        let Some(roc) = built("roc") else { return };
        let cache = crate::platform::temp::test_run().join(format!("drift-plugin-cache-{}", crate::random_hex(4)));
        let runtime = Runtime::new(&cache).unwrap();
        let site = |config| Site {
            entry: "plugins/roc.wasm".into(),
            config,
            engine: Weak::new(),
        };

        let plugin = runtime.load(&roc, site(serde_json::json!({}))).await.unwrap();
        assert_eq!(
            plugin.capabilities,
            ["lsp"],
            "it calls nothing, but Drift starts its server"
        );
        assert_eq!(
            plugin.language_servers(),
            [LanguageServer {
                id: "roc".into(),
                command: vec!["roc_language_server".into()],
                extensions: vec![".roc".into()],
                language: Some("roc".into()),
                label: "Roc".into(),
                setup: crate::hook::ServerSetup::Guide("https://www.roc-lang.org/install".into()),
            }]
        );
        let grammars = plugin.grammars();
        assert_eq!((grammars[0].name.as_str(), grammars[0].plugin.as_str()), ("roc", "roc"));
        assert_eq!(grammars[0].grammar["scopeName"], "source.roc");

        assert!(
            plugin.settings.is_empty(),
            "Settings > Language servers turns its server off, not a setting"
        );
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[tokio::test]
    async fn a_declared_setting_left_unset_reaches_the_plugin_as_its_default() {
        let Some(secrets) = built("secrets") else { return };
        let cache = crate::platform::temp::test_run().join(format!("drift-plugin-cache-{}", crate::random_hex(4)));
        let unset = Site {
            entry: "plugins/secrets.wasm".into(),
            config: serde_json::json!({}),
            engine: Weak::new(),
        };

        let plugin = Runtime::new(&cache).unwrap().load(&secrets, unset).await.unwrap();
        let live = plugin.live.lock().await;
        let state = live.store.data();
        assert_eq!(
            crate::hook::settings::with_defaults(&state.settings, &state.site.config),
            serde_json::json!({ "replacements": {} })
        );
        drop(live);
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[tokio::test]
    async fn the_guard_plugin_refuses_history_rewrites_and_notes_failures() {
        let Some(path) = guard() else { return };
        let cache = crate::platform::temp::test_run().join(format!("drift-plugin-cache-{}", crate::random_hex(4)));
        let runtime = Runtime::new(&cache).unwrap();
        let plugin = runtime.load(&path, site()).await.unwrap();

        assert_eq!(plugin.name(), "guard");
        assert_eq!(
            plugin.capabilities,
            vec!["notify".to_owned(), "process".to_owned()],
            "guard runs a program and tells the user, nothing else"
        );
        assert_eq!(plugin.before_tool(&call("bash", "git status")).await, BeforeTool::Allow);
        assert_eq!(
            plugin.before_tool(&call("bash", "git push --force origin main")).await,
            BeforeTool::Deny("`git push --force` rewrites history; ask the user to run it".into())
        );
        assert_eq!(
            plugin.before_tool(&call("bash", "git  push	--force origin main")).await,
            BeforeTool::Deny("`git push --force` rewrites history; ask the user to run it".into()),
            "spacing does not hide a refused command"
        );
        assert_eq!(
            plugin.before_tool(&call("read", "git push --force")).await,
            BeforeTool::Allow
        );

        let failed = ToolResult {
            session_id: "s1".into(),
            workspace: "C:/work".into(),
            agent: "build".into(),
            tool: "bash".into(),
            input: serde_json::json!({}),
            output: "boom".into(),
            failed: true,
        };
        assert_eq!(
            plugin.after_tool(&failed).await,
            AfterTool::Note("saw this command fail".into())
        );
        assert_eq!(
            plugin
                .after_tool(&ToolResult {
                    failed: false,
                    ..failed
                })
                .await,
            AfterTool::Keep
        );

        plugin
            .session(&SessionEvent {
                id: "s1".into(),
                workspace: "C:/work".into(),
                title: String::new(),
                agent: "build".into(),
                kind: SessionKind::Created,
            })
            .await;

        // A second load of the same file comes from the cache the first one wrote.
        assert!(std::fs::read_dir(&cache).is_ok_and(|entries| entries.count() > 0));
        runtime.load(&path, site()).await.unwrap();
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[tokio::test]
    async fn the_guard_plugin_runs_the_workspaces_tests_on_request_and_keeps_the_turn_going_when_they_fail() {
        let Some(path) = guard() else { return };
        let cache = crate::platform::temp::test_run().join(format!("drift-plugin-cache-{}", crate::random_hex(4)));
        // Outside the cache: wasmtime's cache worker deletes files it does not recognise there.
        let workspace = crate::platform::temp::test_run().join(format!("drift-guard-ws-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&workspace).unwrap();
        let runtime = Runtime::new(&cache).unwrap();
        let mut site = site();
        let failing = workspace.join("tests-fail");
        let fails_when_marked = "process.exit(require('fs').existsSync(process.argv[1]) ? 1 : 0)";
        site.config = serde_json::json!({ "test": ["node", "-e", fails_when_marked, failing.to_string_lossy()] });
        let plugin = runtime.load(&path, site).await.unwrap();
        let reply = |text: &str| ReplyEvent {
            session_id: "s1".into(),
            workspace: workspace.to_string_lossy().into_owned(),
            agent: "build".into(),
            text: text.into(),
        };

        assert_eq!(
            plugin.turn_end(&reply("Changed nothing.")).await,
            TurnEnd::Accept,
            "a reply without the trigger runs nothing"
        );
        assert_eq!(
            plugin.turn_end(&reply("Done. @guard test")).await,
            TurnEnd::Note("tests passed".into())
        );

        std::fs::write(&failing, "").unwrap();
        let outcome = plugin.turn_end(&reply("Done. @guard test")).await;
        assert!(
            matches!(&outcome, TurnEnd::Continue(reason) if reason.contains("exit code 1")),
            "{outcome:?}"
        );

        let prompt = PromptEvent {
            session_id: "s1".into(),
            workspace: workspace.to_string_lossy().into_owned(),
            agent: "build".into(),
            text: "hello".into(),
        };
        assert_eq!(plugin.prompt_submit(&prompt).await, PromptSubmit::Keep);

        let _ = std::fs::remove_dir_all(&cache);
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// The locks plugin loaded on the sandbox's engine, which keeps its store, and its plugin cache.
    async fn locks(sandbox: &crate::tool::tests::Sandbox) -> Option<(std::sync::Arc<WasmPlugin>, PathBuf)> {
        let path = built("locks")?;
        let cache = crate::platform::temp::test_run().join(format!("drift-plugin-cache-{}", crate::random_hex(4)));
        let site = Site {
            entry: "plugins/locks.wasm".into(),
            config: serde_json::json!({}),
            engine: std::sync::Arc::downgrade(&sandbox.ctx.engine),
        };
        let plugin = Runtime::new(&cache).unwrap().load(&path, site).await.unwrap();

        Some((std::sync::Arc::new(plugin), cache))
    }

    #[tokio::test]
    async fn the_locks_plugin_makes_a_second_session_wait_until_the_first_releases() {
        let sandbox = crate::tool::tests::Sandbox::new("locks-plugin");
        let Some((plugin, cache)) = locks(&sandbox).await else {
            return;
        };
        assert_eq!(plugin.capabilities, ["store", "tools"]);
        sandbox.ctx.engine.hooks.set(vec![plugin], vec![]);
        let lock = sandbox.ctx.engine.plugin_tools().remove(0);
        let first = sandbox.ctx_clone();
        let mut second = sandbox.ctx_clone();
        second.session_id = "ses_second".into();
        let run = |ctx: &'static str, input: serde_json::Value| {
            let ctx = if ctx == "first" { &first } else { &second };
            lock.run(ctx, input)
        };

        let taken = run(
            "first",
            serde_json::json!({ "action": "acquire", "name": "merge", "reason": "gates" }),
        );
        assert!(taken.await.unwrap().output.starts_with("Acquired merge"));
        let refused = run(
            "second",
            serde_json::json!({ "action": "acquire", "name": "merge", "wait_seconds": 0 }),
        )
        .await
        .unwrap_err();
        assert!(
            refused.0.contains("held by build (session ses_test)") && refused.0.contains("for gates"),
            "{}",
            refused.0
        );
        let mistaken = run("second", serde_json::json!({ "action": "release", "name": "merge" })).await;
        assert!(mistaken.is_err(), "only the holder releases without force");

        let waiting = run("second", serde_json::json!({ "action": "acquire", "name": "merge" }));
        let release = async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            run("first", serde_json::json!({ "action": "release", "name": "merge" })).await
        };
        let (acquired, released) = tokio::join!(waiting, release);
        assert_eq!(released.unwrap().output, "Released merge.");
        assert!(acquired.unwrap().output.starts_with("Acquired merge"));
        let listed = run("first", serde_json::json!({ "action": "list" })).await.unwrap();
        assert!(
            listed.output.starts_with("merge: held by build (session ses_second)"),
            "{}",
            listed.output
        );
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[tokio::test]
    async fn a_finished_subagents_locks_free_and_a_conversations_stay_until_it_is_deleted() {
        let sandbox = crate::tool::tests::Sandbox::new("locks-plugin-ends");
        let Some((plugin, cache)) = locks(&sandbox).await else {
            return;
        };
        let ask = |session: &str, worker: bool, input: serde_json::Value| crate::hook::ToolRequest {
            session_id: session.into(),
            workspace: String::new(),
            agent: "general".into(),
            tool: "lock".into(),
            input,
            waited_ms: 0,
            worker,
        };
        let take = |session, worker, name| {
            ask(
                session,
                worker,
                serde_json::json!({ "action": "acquire", "name": name }),
            )
        };
        let ended = |id: &str, kind| SessionEvent {
            id: id.into(),
            workspace: String::new(),
            title: String::new(),
            agent: String::new(),
            kind,
        };
        let list = || async {
            match plugin
                .call_tool(&ask("ses_any", false, serde_json::json!({ "action": "list" })))
                .await
            {
                crate::hook::ToolAnswer::Done(text) => text,
                other => panic!("{other:?}"),
            }
        };

        for request in [take("ses_worker", true, "bench"), take("ses_chat", false, "merge")] {
            assert!(matches!(
                plugin.call_tool(&request).await,
                crate::hook::ToolAnswer::Done(_)
            ));
        }
        plugin.session(&ended("ses_worker", SessionKind::Idle)).await;
        plugin.session(&ended("ses_chat", SessionKind::Idle)).await;
        let listed = list().await;
        assert!(listed.starts_with("merge:") && !listed.contains("bench"), "{listed}");

        plugin.session(&ended("ses_chat", SessionKind::Deleted)).await;
        assert_eq!(list().await, "No locks are held.");
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[tokio::test]
    async fn a_huge_ttl_or_wait_neither_frees_the_lock_nor_gives_up_at_once() {
        let sandbox = crate::tool::tests::Sandbox::new("locks-plugin-huge");
        let Some((plugin, cache)) = locks(&sandbox).await else {
            return;
        };
        let ask = |session: &str, input: serde_json::Value| crate::hook::ToolRequest {
            session_id: session.into(),
            workspace: String::new(),
            agent: "general".into(),
            tool: "lock".into(),
            input,
            waited_ms: 0,
            worker: false,
        };

        let forever = serde_json::json!({ "action": "acquire", "name": "merge", "ttl_minutes": u64::MAX });
        let taken = plugin.call_tool(&ask("ses_a", forever)).await;
        assert!(
            matches!(&taken, crate::hook::ToolAnswer::Done(text) if text.contains("in 1440 minutes")),
            "{taken:?}"
        );
        // 2^61 seconds in milliseconds wraps to 0 without saturation.
        let patient = serde_json::json!({ "action": "acquire", "name": "merge", "wait_seconds": 1u64 << 61 });
        let waiting = plugin.call_tool(&ask("ses_b", patient)).await;
        assert!(matches!(waiting, crate::hook::ToolAnswer::Wait { .. }), "{waiting:?}");
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[tokio::test]
    async fn a_plugin_that_trapped_is_started_afresh_and_guards_the_next_call() {
        let Some(path) = guard() else { return };
        let cache = crate::platform::temp::test_run().join(format!("drift-plugin-cache-{}", crate::random_hex(4)));
        let plugin = Runtime::new(&cache).unwrap().load(&path, site()).await.unwrap();
        let force = call("bash", "git push --force origin main");
        let refused = BeforeTool::Deny("`git push --force` rewrites history; ask the user to run it".into());

        let mut live = plugin.live.lock().await;
        let trapped = &mut *live;
        trapped.store.data_mut().deadline = Instant::now();
        trapped.store.set_epoch_deadline(0);
        let input = wit::ToolCall {
            session_id: force.session_id.clone(),
            workspace: force.workspace.clone(),
            agent: force.agent.clone(),
            tool: force.tool.clone(),
            input: force.input.to_string(),
        };
        let ran = trapped.bindings.call_before_tool(&mut trapped.store, &input).await;
        assert!(ran.is_err(), "it ran past its time");
        drop(live);

        assert_eq!(
            plugin.before_tool(&force).await,
            BeforeTool::Allow,
            "the call into the trapped store keeps its fallback"
        );
        assert_eq!(
            plugin.before_tool(&force).await,
            refused,
            "and the next is guarded again"
        );
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[tokio::test]
    async fn a_trap_while_reading_an_export_at_load_starts_afresh_with_every_export() {
        let sandbox = crate::tool::tests::Sandbox::new("locks-plugin-load-trap");
        let Some((plugin, cache)) = locks(&sandbox).await else {
            return;
        };
        let mut live = plugin.live.lock().await;
        live.call_tool = None;
        live.store.data_mut().deadline = Instant::now();
        live.store.set_epoch_deadline(0);

        let trapped: Vec<wit::ToolSpec> = read(&mut live, &plugin.pre, "tools").await.unwrap();
        assert!(trapped.is_empty(), "a trapped export gives nothing");
        assert!(
            live.call_tool.is_some(),
            "the fresh store looks up every export, not only those it had"
        );
        let declared: Vec<wit::ToolSpec> = read(&mut live, &plugin.pre, "tools").await.unwrap();
        assert_eq!(declared.len(), plugin.tools().len(), "and answers the next read");
        drop(live);
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[tokio::test]
    async fn a_call_waiting_past_its_ceiling_falls_back_and_the_next_gets_a_fresh_store() {
        let Some(path) = guard() else { return };
        let cache = crate::platform::temp::test_run().join(format!("drift-plugin-cache-{}", crate::random_hex(4)));
        let workspace = crate::platform::temp::test_run().join(format!("drift-guard-wait-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&workspace).unwrap();
        let mut site = site();
        site.config = serde_json::json!({ "test": ["node", "-e", "setTimeout(() => {}, 30000)"] });
        let mut plugin = Runtime::new(&cache).unwrap().load(&path, site).await.unwrap();
        plugin.ceiling = Duration::from_millis(500);
        let reply = ReplyEvent {
            session_id: "s1".into(),
            workspace: workspace.to_string_lossy().into_owned(),
            agent: "build".into(),
            text: "Done. @guard test".into(),
        };

        let started = Instant::now();
        assert_eq!(plugin.turn_end(&reply).await, TurnEnd::Accept, "it keeps its fallback");
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        assert_eq!(
            plugin.before_tool(&call("bash", "git push --force origin main")).await,
            BeforeTool::Deny("`git push --force` rewrites history; ask the user to run it".into()),
            "and the next call is guarded again"
        );
        let _ = std::fs::remove_dir_all(&cache);
        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[tokio::test]
    async fn a_file_that_is_not_a_component_reports_why() {
        let cache = crate::platform::temp::test_run().join(format!("drift-plugin-cache-{}", crate::random_hex(4)));
        let bad = cache.join("bad.wasm");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(&bad, b"not wasm").unwrap();
        let error = Runtime::new(&cache)
            .unwrap()
            .load(&bad, site())
            .await
            .err()
            .expect("refused");
        assert!(matches!(error, Error::Compile(_)), "{error}");
        assert!(error.to_string().starts_with("could not compile:"));
        let _ = std::fs::remove_dir_all(&cache);
    }
}

#[cfg(test)]
mod load_tests {
    use crate::hook::{Hooks, Listed};

    #[tokio::test]
    async fn a_plugin_switched_off_is_listed_and_runs_nothing_and_a_bad_entry_keeps_its_error() {
        let Some(path) = super::tests::guard() else { return };
        let cache = crate::platform::temp::test_run().join(format!("drift-plugin-cache-{}", crate::random_hex(4)));
        let hooks = Hooks::default();
        let entries = || {
            vec![
                Listed {
                    entry: "plugins/guard.wasm".to_owned(),
                    path: Ok(path.clone()),
                    config: serde_json::json!({}),
                },
                Listed {
                    entry: "plugins/x.js".to_owned(),
                    path: Err(super::Error::Resolve("a plugin is a .wasm component".to_owned())),
                    config: serde_json::json!({}),
                },
            ]
        };

        let listed = hooks
            .load(
                &cache,
                entries(),
                &["plugins/guard.wasm".to_owned()],
                std::sync::Weak::new(),
            )
            .await;
        assert_eq!(listed.len(), 2);
        assert_eq!(
            (listed[0].name.as_str(), listed[0].enabled, listed[0].error.as_deref()),
            ("guard", false, None)
        );
        assert_eq!(
            (listed[1].name.as_str(), listed[1].enabled, listed[1].error.as_deref()),
            ("x", true, Some("a plugin is a .wasm component"))
        );
        assert!(hooks.is_empty(), "a plugin that is off is not consulted");

        let listed = hooks.load(&cache, entries(), &[], std::sync::Weak::new()).await;
        assert!(listed[0].enabled && !hooks.is_empty());
        assert_eq!(listed[0].capabilities, vec!["notify".to_owned(), "process".to_owned()]);

        let _ = std::fs::remove_dir_all(&cache);
    }
}
