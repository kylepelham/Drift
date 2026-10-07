//! The engine's plugin seam: moments a plugin may observe or answer. `Hook` is runtime-neutral;
//! `wasm` runs WebAssembly components against it.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

#[cfg(feature = "wasm-plugins")]
pub mod wasm;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub session_id: String,
    pub workspace: String,
    pub agent: String,
    pub tool: String,
    pub input: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", content = "value", rename_all = "camelCase")]
pub enum BeforeTool {
    Allow,
    /// The call does not run; the model is told why.
    Deny(String),
    /// The call runs with this input instead.
    Replace(Value),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub session_id: String,
    pub workspace: String,
    pub agent: String,
    pub tool: String,
    pub input: Value,
    pub output: String,
    pub failed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", content = "value", rename_all = "camelCase")]
pub enum AfterTool {
    Keep,
    /// The model reads this instead.
    Replace(String),
    /// Appended to the output as a note from Drift.
    Note(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionKind {
    Created,
    Running,
    Idle,
    Updated,
    Deleted,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEvent {
    pub id: String,
    pub workspace: String,
    pub title: String,
    pub agent: String,
    pub kind: SessionKind,
}

/// One plugin. Every method has a do-nothing default so a hook answers only what it cares about.
#[async_trait]
pub trait Hook: Send + Sync {
    fn name(&self) -> &str;
    async fn before_tool(&self, _call: &ToolCall) -> BeforeTool {
        BeforeTool::Allow
    }
    async fn after_tool(&self, _result: &ToolResult) -> AfterTool {
        AfterTool::Keep
    }
    async fn session(&self, _event: &SessionEvent) {}
}

/// A loaded plugin as the API reports it; `error` set means it is not running.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    pub name: String,
    pub path: String,
    /// Off in Settings: listed, not loaded.
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The hooks in the order they were configured; every event visits each in turn.
#[derive(Default)]
pub struct Hooks {
    hooks: RwLock<Vec<Arc<dyn Hook>>>,
    loaded: RwLock<Vec<PluginInfo>>,
    #[cfg(feature = "wasm-plugins")]
    runtime: std::sync::OnceLock<Result<wasm::Runtime, String>>,
}

impl Hooks {
    pub fn set(&self, hooks: Vec<Arc<dyn Hook>>, loaded: Vec<PluginInfo>) {
        *self.hooks.write().unwrap() = hooks;
        *self.loaded.write().unwrap() = loaded;
    }

    /// Loads the listed plugins (drift.json entries with their resolved paths), replacing the set
    /// loaded before. One that fails stays in the report with its error; one in `disabled` is listed and left alone.
    pub async fn load(&self, cache_dir: &std::path::Path, entries: Vec<(String, Result<std::path::PathBuf, String>)>, disabled: &[String]) -> Vec<PluginInfo> {
        let mut hooks: Vec<Arc<dyn Hook>> = Vec::new();
        let mut loaded = Vec::new();
        for (entry, path) in entries {
            if disabled.contains(&entry) {
                loaded.push(PluginInfo { name: plugin_name(&entry), path: entry, enabled: false, error: None });
                continue;
            }
            let outcome = match path {
                Ok(path) => self.load_one(cache_dir, &path).await,
                Err(error) => Err(error),
            };
            match outcome {
                Ok((name, hook)) => {
                    loaded.push(PluginInfo { name, path: entry, enabled: true, error: None });
                    hooks.push(hook);
                }
                Err(error) => loaded.push(PluginInfo { name: plugin_name(&entry), path: entry, enabled: true, error: Some(error) }),
            }
        }
        self.set(hooks, loaded.clone());
        loaded
    }

    #[cfg(feature = "wasm-plugins")]
    async fn load_one(&self, cache_dir: &std::path::Path, path: &std::path::Path) -> Result<(String, Arc<dyn Hook>), String> {
        let runtime = self.runtime.get_or_init(|| wasm::Runtime::new(cache_dir)).as_ref().map_err(Clone::clone)?;
        let plugin = runtime.load(path).await?;
        Ok((plugin.name().to_owned(), Arc::new(plugin)))
    }

    #[cfg(not(feature = "wasm-plugins"))]
    async fn load_one(&self, _cache_dir: &std::path::Path, _path: &std::path::Path) -> Result<(String, Arc<dyn Hook>), String> {
        Err("this build of Drift runs no plugins".into())
    }

    pub fn loaded(&self) -> Vec<PluginInfo> {
        self.loaded.read().unwrap().clone()
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.read().unwrap().is_empty()
    }

    fn list(&self) -> Vec<Arc<dyn Hook>> {
        self.hooks.read().unwrap().clone()
    }

    /// The first refusal wins; a replacement feeds the hooks after it. Returns the deciding hook's name with a denial.
    pub async fn before_tool(&self, mut call: ToolCall) -> (ToolCall, Option<(String, String)>) {
        for hook in self.list() {
            match hook.before_tool(&call).await {
                BeforeTool::Allow => {}
                BeforeTool::Deny(reason) => return (call, Some((hook.name().to_owned(), reason))),
                BeforeTool::Replace(input) => call.input = input,
            }
        }
        (call, None)
    }

    /// Replacements chain; notes collect in order.
    pub async fn after_tool(&self, mut result: ToolResult) -> (String, Vec<String>) {
        let mut notes = Vec::new();
        for hook in self.list() {
            match hook.after_tool(&result).await {
                AfterTool::Keep => {}
                AfterTool::Replace(output) => result.output = output,
                AfterTool::Note(note) => notes.push(note),
            }
        }
        (result.output, notes)
    }

    pub async fn session(&self, event: &SessionEvent) {
        for hook in self.list() {
            hook.session(event).await;
        }
    }
}

/// A plugin's name before it has said one: its file's stem.
fn plugin_name(entry: &str) -> String {
    std::path::Path::new(entry).file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_else(|| entry.to_owned())
}

/// Session events as the hub publishes them, handed to the hooks until the hub closes.
pub async fn relay_session_events(engine: Arc<crate::Engine>) {
    use crate::event::{Event, SessionStatus};
    let mut rx = engine.hub.attach(None).rx;
    loop {
        let envelope = match rx.recv().await {
            Ok(envelope) => envelope,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        };
        if engine.hooks.is_empty() {
            continue;
        }
        let event = match envelope.event {
            Event::SessionCreated { session } => session_event(&engine, &session, SessionKind::Created),
            Event::SessionUpdated { session } => session_event(&engine, &session, SessionKind::Updated),
            Event::SessionDeleted { session_id } => SessionEvent { id: session_id, workspace: String::new(), title: String::new(), agent: String::new(), kind: SessionKind::Deleted },
            Event::SessionStatusChanged { session_id, status } => {
                let kind = if status == SessionStatus::Running { SessionKind::Running } else { SessionKind::Idle };
                match engine.store.session(&session_id).ok().flatten() {
                    Some(session) => session_event(&engine, &session, kind),
                    None => continue,
                }
            }
            _ => continue,
        };
        engine.hooks.session(&event).await;
    }
}

fn session_event(engine: &crate::Engine, session: &crate::session::types::Session, kind: SessionKind) -> SessionEvent {
    let workspace = engine.store.workspace(&session.workspace_id).ok().flatten().map(|workspace| workspace.path).unwrap_or_default();
    SessionEvent { id: session.id.clone(), workspace, title: session.title.clone(), agent: session.agent.clone(), kind }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(&'static str, BeforeTool, AfterTool);

    #[async_trait]
    impl Hook for Fixed {
        fn name(&self) -> &str {
            self.0
        }
        async fn before_tool(&self, _call: &ToolCall) -> BeforeTool {
            self.1.clone()
        }
        async fn after_tool(&self, _result: &ToolResult) -> AfterTool {
            self.2.clone()
        }
    }

    fn call() -> ToolCall {
        ToolCall { session_id: "s".into(), workspace: "w".into(), agent: "build".into(), tool: "bash".into(), input: serde_json::json!({ "command": "ls" }) }
    }

    #[tokio::test]
    async fn a_replacement_reaches_the_next_hook_and_the_first_denial_wins() {
        let hooks = Hooks::default();
        hooks.set(
            vec![
                Arc::new(Fixed("a", BeforeTool::Replace(serde_json::json!({ "command": "ls -la" })), AfterTool::Note("seen".into()))),
                Arc::new(Fixed("b", BeforeTool::Deny("no".into()), AfterTool::Replace("short".into()))),
                Arc::new(Fixed("c", BeforeTool::Deny("never asked".into()), AfterTool::Keep)),
            ],
            vec![],
        );
        let (call, denied) = hooks.before_tool(call()).await;
        assert_eq!(call.input["command"], "ls -la");
        assert_eq!(denied, Some(("b".to_owned(), "no".to_owned())));
        let result = ToolResult { session_id: "s".into(), workspace: "w".into(), agent: "build".into(), tool: "bash".into(), input: Value::Null, output: "long".into(), failed: false };
        assert_eq!(hooks.after_tool(result).await, ("short".to_owned(), vec!["seen".to_owned()]));
    }
}
