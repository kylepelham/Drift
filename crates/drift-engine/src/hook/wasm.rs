//! Plugins as sandboxed WebAssembly components, compiled once into a disk cache; the contract is `wit/drift.wit`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::Mutex;
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Cache, CacheConfig, Config, Engine, Store, UpdateDeadline};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use super::{
    AfterTool, BeforeTool, Compacting, CompactionEvent, Hook, PermissionAsk, PermissionDecision, PromptEvent,
    PromptSubmit, ReplyEvent, SessionEvent, SessionKind, ToolCall, ToolResult, TurnEnd,
};

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
const WASM_PACKAGE: &str = "drift:plugin/";

struct State {
    name: String,
    site: Site,
    /// The workspace of the event being handled, where files and processes are scoped.
    workspace: PathBuf,
    /// When the current call's time is up; host calls move it on by what they took.
    deadline: Instant,
    wasi: WasiCtx,
    table: ResourceTable,
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

        let capabilities = capabilities(&self.engine, &component);
        let state = State {
            name: file_name(path),
            site,
            workspace: PathBuf::new(),
            deadline: Instant::now() + CALL_BUDGET,
            wasi: WasiCtxBuilder::new().inherit_stderr().build(),
            table: ResourceTable::new(),
        };

        let mut store = Store::new(&self.engine, state);
        store.set_epoch_deadline(1);
        store.epoch_deadline_callback(|store| {
            if Instant::now() < store.data().deadline {
                Ok(UpdateDeadline::Continue(1))
            } else {
                Err(wasmtime::Error::msg("the plugin ran past its time"))
            }
        });

        let bindings = Plugin::instantiate_async(&mut store, &component, &self.linker)
            .await
            .map_err(|error| Error::Instantiate(error.to_string()))?;
        let name = bindings
            .call_name(&mut store)
            .await
            .map_err(|error| Error::Name(error.to_string()))?;
        if name.trim().is_empty() {
            return Err(Error::EmptyName);
        }

        store.data_mut().name.clone_from(&name);

        Ok(WasmPlugin {
            name,
            path: path.to_path_buf(),
            capabilities,
            store: Mutex::new(store),
            bindings,
        })
    }
}

/// The host interfaces a component imports besides `host` itself: what it can reach.
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
    store: Mutex<Store<State>>,
    bindings: Plugin,
}

impl WasmPlugin {
    fn failed(&self, what: &str, error: &wasmtime::Error) {
        eprintln!("drift: plugin {} failed in {what}: {error}", self.name);
    }

    /// The store ready for one call: scoped to the event's workspace, with a fresh budget.
    async fn enter(&self, workspace: &str) -> tokio::sync::MutexGuard<'_, Store<State>> {
        let mut store = self.store.lock().await;
        let state = store.data_mut();
        state.workspace = PathBuf::from(workspace);
        state.deadline = Instant::now() + CALL_BUDGET;

        store
    }
}

#[async_trait]
impl Hook for WasmPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    async fn before_tool(&self, call: &ToolCall) -> BeforeTool {
        let input = wit::ToolCall {
            session_id: call.session_id.clone(),
            workspace: call.workspace.clone(),
            agent: call.agent.clone(),
            tool: call.tool.clone(),
            input: call.input.to_string(),
        };
        let mut store = self.enter(&call.workspace).await;

        match self.bindings.call_before_tool(&mut *store, &input).await {
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
                self.failed("before-tool", &error);
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
        let mut store = self.enter(&result.workspace).await;

        match self.bindings.call_after_tool(&mut *store, &input).await {
            Ok(wit::AfterTool::Keep) => AfterTool::Keep,
            Ok(wit::AfterTool::Replace(output)) => AfterTool::Replace(output),
            Ok(wit::AfterTool::Note(note)) => AfterTool::Note(note),
            Err(error) => {
                self.failed("after-tool", &error);
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
        let mut store = self.enter(&prompt.workspace).await;

        match self.bindings.call_prompt_submit(&mut *store, &input).await {
            Ok(wit::PromptSubmit::Keep) => PromptSubmit::Keep,
            Ok(wit::PromptSubmit::Replace(text)) => PromptSubmit::Replace(text),
            Ok(wit::PromptSubmit::AddContext(text)) => PromptSubmit::AddContext(text),
            Ok(wit::PromptSubmit::Deny(reason)) => PromptSubmit::Deny(reason),
            Err(error) => {
                self.failed("prompt-submit", &error);
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
        let mut store = self.enter(&reply.workspace).await;

        match self.bindings.call_turn_end(&mut *store, &input).await {
            Ok(wit::TurnEnd::Accept) => TurnEnd::Accept,
            Ok(wit::TurnEnd::Note(note)) => TurnEnd::Note(note),
            Ok(wit::TurnEnd::Continue(reason)) => TurnEnd::Continue(reason),
            Err(error) => {
                self.failed("turn-end", &error);
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
        let mut store = self.enter(&ask.workspace).await;

        match self.bindings.call_permission(&mut *store, &input).await {
            Ok(wit::Permission::Pass) => PermissionDecision::Pass,
            Ok(wit::Permission::Allow) => PermissionDecision::Allow,
            Ok(wit::Permission::Deny(reason)) => PermissionDecision::Deny(reason),
            Err(error) => {
                self.failed("permission", &error);
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
        let mut store = self.enter(&event.workspace).await;

        match self.bindings.call_compaction(&mut *store, &input).await {
            Ok(wit::Compacting::Proceed) => Compacting::Proceed,
            Ok(wit::Compacting::Instruct(text)) => Compacting::Instruct(text),
            Err(error) => {
                self.failed("compaction", &error);
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
        let mut store = self.enter(&event.workspace).await;

        if let Err(error) = self.bindings.call_session(&mut *store, &session, kind).await {
            self.failed("session", &error);
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::sync::Weak;

    /// The example plugin, built into the workspace's own `target/plugins`; `None` when the wasm32-wasip2 target is not installed.
    pub(super) fn guard() -> Option<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let target = root.join("target/plugins");
        let built = std::process::Command::new("cargo")
            .args(["build", "--release", "--target", "wasm32-wasip2"])
            .env("CARGO_TARGET_DIR", &target)
            .current_dir(root.join("plugins/guard"))
            .output()
            .expect("cargo runs");
        if !built.status.success() {
            let stderr = String::from_utf8_lossy(&built.stderr);
            assert!(
                stderr.contains("wasm32-wasip2"),
                "guard plugin failed to build: {stderr}"
            );
            eprintln!("skipping: the wasm32-wasip2 target is not installed");
            return None;
        }
        Some(target.join("wasm32-wasip2/release/guard.wasm"))
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
    async fn the_guard_plugin_refuses_history_rewrites_and_notes_failures() {
        let Some(path) = guard() else { return };
        let cache = std::env::temp_dir().join(format!("drift-plugin-cache-{}", crate::random_hex(4)));
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
        let cache = std::env::temp_dir().join(format!("drift-plugin-cache-{}", crate::random_hex(4)));
        let workspace = cache.join("ws");
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
    }

    #[tokio::test]
    async fn a_file_that_is_not_a_component_reports_why() {
        let cache = std::env::temp_dir().join(format!("drift-plugin-cache-{}", crate::random_hex(4)));
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
        let cache = std::env::temp_dir().join(format!("drift-plugin-cache-{}", crate::random_hex(4)));
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
