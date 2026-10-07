//! Plugins as WebAssembly components: sandboxed, any language with a component toolchain, compiled
//! once per file into a disk cache. The contract is `wit/drift.wit`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Cache, CacheConfig, Config, Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use super::{AfterTool, BeforeTool, Hook, SessionEvent, SessionKind, ToolCall, ToolResult};

mod bindings {
    wasmtime::component::bindgen!({ world: "plugin", path: "wit", imports: { default: async }, exports: { default: async } });
}

use bindings::drift::plugin::host::{Host, Level};
use bindings::drift::plugin::types as wit;
use bindings::Plugin;

/// A hook call that runs past this is stopped; the epoch ticks every 100 ms.
const EPOCH_TICK: Duration = Duration::from_millis(100);
const CALL_TICKS: u64 = 50;

struct State {
    name: String,
    wasi: WasiCtx,
    table: ResourceTable,
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

impl wit::Host for State {}

impl Host for State {
    async fn log(&mut self, level: Level, message: String) {
        let level = match level {
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        };
        eprintln!("drift: plugin {} [{level}]: {message}", self.name);
    }
}

/// One compiler and linker for every plugin; compiled code is cached under `cache_dir`.
pub struct Runtime {
    engine: Engine,
    linker: Linker<State>,
}

impl Runtime {
    pub fn new(cache_dir: &Path) -> Result<Self, String> {
        let mut config = Config::new();
        config.epoch_interruption(true);
        let mut cache = CacheConfig::new();
        cache.with_directory(cache_dir);
        config.cache(Some(Cache::new(cache).map_err(|error| format!("plugin cache: {error}"))?));
        let engine = Engine::new(&config).map_err(|error| format!("plugin runtime: {error}"))?;
        let ticker = engine.clone();
        std::thread::Builder::new()
            .name("drift-plugin-epoch".into())
            .spawn(move || loop {
                std::thread::sleep(EPOCH_TICK);
                ticker.increment_epoch();
            })
            .map_err(|error| format!("plugin runtime: {error}"))?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(|error| format!("plugin runtime: {error}"))?;
        Plugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state).map_err(|error| format!("plugin runtime: {error}"))?;
        Ok(Self { engine, linker })
    }

    pub async fn load(&self, path: &Path) -> Result<WasmPlugin, String> {
        let engine = self.engine.clone();
        let file = path.to_path_buf();
        let component = tokio::task::spawn_blocking(move || Component::from_file(&engine, &file))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| format!("could not compile: {error}"))?;
        let state = State { name: file_name(path), wasi: WasiCtxBuilder::new().inherit_stderr().build(), table: ResourceTable::new() };
        let mut store = Store::new(&self.engine, state);
        store.set_epoch_deadline(CALL_TICKS);
        let bindings = Plugin::instantiate_async(&mut store, &component, &self.linker).await.map_err(|error| format!("could not instantiate: {error}"))?;
        let name = bindings.call_name(&mut store).await.map_err(|error| format!("name(): {error}"))?;
        if name.trim().is_empty() {
            return Err("name() returned nothing".into());
        }
        store.data_mut().name.clone_from(&name);
        Ok(WasmPlugin { name, path: path.to_path_buf(), store: Mutex::new(store), bindings })
    }
}

fn file_name(path: &Path) -> String {
    path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default()
}

/// One loaded component; its calls run one at a time, each with a fresh time budget.
pub struct WasmPlugin {
    name: String,
    pub path: PathBuf,
    store: Mutex<Store<State>>,
    bindings: Plugin,
}

impl WasmPlugin {
    fn failed(&self, what: &str, error: &wasmtime::Error) {
        eprintln!("drift: plugin {} failed in {what}: {error}", self.name);
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
        let mut store = self.store.lock().await;
        store.set_epoch_deadline(CALL_TICKS);
        match self.bindings.call_before_tool(&mut *store, &input).await {
            Ok(wit::BeforeTool::Allow) => BeforeTool::Allow,
            Ok(wit::BeforeTool::Deny(reason)) => BeforeTool::Deny(reason),
            Ok(wit::BeforeTool::Replace(json)) => match serde_json::from_str(&json) {
                Ok(input) => BeforeTool::Replace(input),
                Err(error) => {
                    eprintln!("drift: plugin {} replaced a tool input with something that is not JSON: {error}", self.name);
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
        let mut store = self.store.lock().await;
        store.set_epoch_deadline(CALL_TICKS);
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

    async fn session(&self, event: &SessionEvent) {
        let session = wit::Session { id: event.id.clone(), workspace: event.workspace.clone(), title: event.title.clone(), agent: event.agent.clone() };
        let kind = match event.kind {
            SessionKind::Created => wit::SessionKind::Created,
            SessionKind::Running => wit::SessionKind::Running,
            SessionKind::Idle => wit::SessionKind::Idle,
            SessionKind::Updated => wit::SessionKind::Updated,
            SessionKind::Deleted => wit::SessionKind::Deleted,
        };
        let mut store = self.store.lock().await;
        store.set_epoch_deadline(CALL_TICKS);
        if let Err(error) = self.bindings.call_session(&mut *store, &session, kind).await {
            self.failed("session", &error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example plugin, built for the test; `None` when the wasm32-wasip2 target is not installed.
    pub(super) fn guard() -> Option<PathBuf> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/guard");
        let built = std::process::Command::new("cargo").args(["build", "--release", "--target", "wasm32-wasip2"]).current_dir(&dir).output().expect("cargo runs");
        if !built.status.success() {
            let stderr = String::from_utf8_lossy(&built.stderr);
            assert!(stderr.contains("wasm32-wasip2"), "guard plugin failed to build: {stderr}");
            eprintln!("skipping: the wasm32-wasip2 target is not installed");
            return None;
        }
        Some(dir.join("target/wasm32-wasip2/release/guard.wasm"))
    }

    fn call(tool: &str, command: &str) -> ToolCall {
        ToolCall { session_id: "s1".into(), workspace: "C:/work".into(), agent: "build".into(), tool: tool.into(), input: serde_json::json!({ "command": command }) }
    }

    #[tokio::test]
    async fn the_guard_plugin_refuses_history_rewrites_and_notes_failures() {
        let Some(path) = guard() else { return };
        let cache = std::env::temp_dir().join(format!("drift-plugin-cache-{}", crate::random_hex(4)));
        let runtime = Runtime::new(&cache).unwrap();
        let plugin = runtime.load(&path).await.unwrap();
        assert_eq!(plugin.name(), "guard");
        assert_eq!(plugin.before_tool(&call("bash", "git status")).await, BeforeTool::Allow);
        assert_eq!(plugin.before_tool(&call("bash", "git push --force origin main")).await, BeforeTool::Deny("`git push --force` rewrites history; ask the user to run it".into()));
        assert_eq!(plugin.before_tool(&call("read", "git push --force")).await, BeforeTool::Allow);
        let failed = ToolResult { session_id: "s1".into(), workspace: "C:/work".into(), agent: "build".into(), tool: "bash".into(), input: serde_json::json!({}), output: "boom".into(), failed: true };
        assert_eq!(plugin.after_tool(&failed).await, AfterTool::Note("saw this command fail".into()));
        assert_eq!(plugin.after_tool(&ToolResult { failed: false, ..failed }).await, AfterTool::Keep);
        plugin.session(&SessionEvent { id: "s1".into(), workspace: "C:/work".into(), title: String::new(), agent: "build".into(), kind: SessionKind::Created }).await;
        // A second load of the same file comes from the cache the first one wrote.
        assert!(std::fs::read_dir(&cache).map(|entries| entries.count() > 0).unwrap_or(false));
        runtime.load(&path).await.unwrap();
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[tokio::test]
    async fn a_file_that_is_not_a_component_reports_why() {
        let cache = std::env::temp_dir().join(format!("drift-plugin-cache-{}", crate::random_hex(4)));
        let bad = cache.join("bad.wasm");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(&bad, b"not wasm").unwrap();
        let error = Runtime::new(&cache).unwrap().load(&bad).await.err().expect("refused");
        assert!(error.starts_with("could not compile:"), "{error}");
        let _ = std::fs::remove_dir_all(&cache);
    }
}

#[cfg(test)]
mod load_tests {
    use crate::hook::Hooks;

    #[tokio::test]
    async fn a_plugin_switched_off_is_listed_and_runs_nothing_and_a_bad_entry_keeps_its_error() {
        let Some(path) = super::tests::guard() else { return };
        let cache = std::env::temp_dir().join(format!("drift-plugin-cache-{}", crate::random_hex(4)));
        let hooks = Hooks::default();
        let entries = || vec![("plugins/guard.wasm".to_owned(), Ok(path.clone())), ("plugins/x.js".to_owned(), Err("a plugin is a .wasm component".to_owned()))];
        let listed = hooks.load(&cache, entries(), &["plugins/guard.wasm".to_owned()]).await;
        assert_eq!(listed.len(), 2);
        assert_eq!((listed[0].name.as_str(), listed[0].enabled, listed[0].error.as_deref()), ("guard", false, None));
        assert_eq!((listed[1].name.as_str(), listed[1].enabled, listed[1].error.as_deref()), ("x", true, Some("a plugin is a .wasm component")));
        assert!(hooks.is_empty(), "a plugin that is off is not consulted");
        let listed = hooks.load(&cache, entries(), &[]).await;
        assert!(listed[0].enabled && !hooks.is_empty());
        let _ = std::fs::remove_dir_all(&cache);
    }
}
