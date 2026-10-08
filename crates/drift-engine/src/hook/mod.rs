//! The engine's plugin seam: moments a plugin may observe or answer; `wasm` runs components against `Hook`.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

mod error;
#[cfg(feature = "wasm-plugins")]
pub mod wasm;

pub use error::Error;

/// Where a plugin lives and what it may reach: its drift.json entry, its config, and the engine for host calls.
pub struct Site {
    pub entry: String,
    pub config: Value,
    pub engine: std::sync::Weak<crate::Engine>,
}

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

/// A call the rules would ask the user about.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionAsk {
    pub session_id: String,
    pub workspace: String,
    pub agent: String,
    pub tool: String,
    pub kind: String,
    pub pattern: String,
    pub title: String,
    pub commands: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", content = "value", rename_all = "camelCase")]
pub enum PermissionDecision {
    /// The user is asked as usual.
    Pass,
    Allow,
    /// The call does not run; the model is told why.
    Deny(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionEvent {
    pub session_id: String,
    pub workspace: String,
    pub agent: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", content = "value", rename_all = "camelCase")]
pub enum Compacting {
    Proceed,
    /// Added to the summariser's instructions.
    Instruct(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionKind {
    Created,
    Running,
    Idle,
    Updated,
    Deleted,
    Compacted,
}

/// The user's prompt before the model sees it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptEvent {
    pub session_id: String,
    pub workspace: String,
    pub agent: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", content = "value", rename_all = "camelCase")]
pub enum PromptSubmit {
    Keep,
    /// The model reads this instead of the user's text.
    Replace(String),
    /// Added beside the prompt as context from the plugin.
    AddContext(String),
    /// The prompt is not sent; the user is told why.
    Deny(String),
}

/// The reply that ended a turn.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyEvent {
    pub session_id: String,
    pub workspace: String,
    pub agent: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", content = "value", rename_all = "camelCase")]
pub enum TurnEnd {
    Accept,
    /// The turn ends; the line is shown under the reply and the model never sees it.
    Note(String),
    /// The turn goes on with this as the plugin's prompt to the model.
    Continue(String),
}

/// What the plugins made of a reply: lines to show under it, and the first continuation if any.
#[derive(Debug, Default, PartialEq)]
pub struct Ended {
    pub notes: Vec<(String, String)>,
    pub continued: Option<(String, String)>,
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
    async fn prompt_submit(&self, _prompt: &PromptEvent) -> PromptSubmit {
        PromptSubmit::Keep
    }
    async fn turn_end(&self, _reply: &ReplyEvent) -> TurnEnd {
        TurnEnd::Accept
    }
    async fn permission(&self, _ask: &PermissionAsk) -> PermissionDecision {
        PermissionDecision::Pass
    }
    async fn compaction(&self, _event: &CompactionEvent) -> Compacting {
        Compacting::Proceed
    }
    async fn session(&self, _event: &SessionEvent) {}
}

/// One `plugins` entry of drift.json: a path, or a path with the plugin's own config.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum PluginEntry {
    Path(String),
    Configured {
        path: String,
        #[serde(default)]
        config: Value,
    },
}

impl PluginEntry {
    pub fn path(&self) -> &str {
        match self {
            Self::Path(path) | Self::Configured { path, .. } => path,
        }
    }

    pub fn config(&self) -> Value {
        match self {
            Self::Path(_) => Value::Object(Default::default()),
            Self::Configured { config, .. } => config.clone(),
        }
    }
}

/// A listed plugin as the loader takes it: its entry, where it resolved to (or why not), and its config.
pub struct Listed {
    pub entry: String,
    pub path: Result<std::path::PathBuf, Error>,
    pub config: Value,
}

/// A loaded plugin as the API reports it; `error` set means it is not running.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    pub name: String,
    pub path: String,
    /// Off in Settings: listed, not loaded.
    pub enabled: bool,
    /// Its config object from drift.json, so Settings can show and edit it.
    #[serde(default)]
    pub config: Value,
    /// The host interfaces it imports: `store`, `files`, `process`, `http`.
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The hooks in the order they were configured; every event visits each in turn.
#[derive(Default)]
pub struct Hooks {
    hooks: RwLock<Vec<Arc<dyn Hook>>>,
    loaded: RwLock<Vec<PluginInfo>>,
    #[cfg(feature = "wasm-plugins")]
    runtime: std::sync::OnceLock<Result<wasm::Runtime, Error>>,
}

/// What a loaded plugin reports about itself.
pub struct Loaded {
    pub name: String,
    pub capabilities: Vec<String>,
    pub hook: Arc<dyn Hook>,
}

impl Hooks {
    pub fn set(&self, hooks: Vec<Arc<dyn Hook>>, loaded: Vec<PluginInfo>) {
        *self.hooks.write().unwrap() = hooks;
        *self.loaded.write().unwrap() = loaded;
    }

    /// Replaces the loaded plugins; a failed one is reported with its error, a `disabled` one listed and left alone.
    pub async fn load(
        &self,
        cache_dir: &std::path::Path,
        entries: Vec<Listed>,
        disabled: &[String],
        engine: std::sync::Weak<crate::Engine>,
    ) -> Vec<PluginInfo> {
        let mut hooks: Vec<Arc<dyn Hook>> = Vec::new();
        let mut loaded = Vec::new();
        for Listed { entry, path, config } in entries {
            if disabled.contains(&entry) {
                loaded.push(PluginInfo {
                    name: plugin_name(&entry),
                    path: entry,
                    enabled: false,
                    config,
                    capabilities: vec![],
                    error: None,
                });
                continue;
            }
            let outcome = match path {
                Ok(path) => {
                    self.load_one(
                        cache_dir,
                        &path,
                        Site {
                            entry: entry.clone(),
                            config: config.clone(),
                            engine: engine.clone(),
                        },
                    )
                    .await
                }
                Err(error) => Err(error),
            };
            match outcome {
                Ok(plugin) => {
                    loaded.push(PluginInfo {
                        name: plugin.name,
                        path: entry,
                        enabled: true,
                        config,
                        capabilities: plugin.capabilities,
                        error: None,
                    });
                    hooks.push(plugin.hook);
                }
                Err(error) => loaded.push(PluginInfo {
                    name: plugin_name(&entry),
                    path: entry,
                    enabled: true,
                    config,
                    capabilities: vec![],
                    error: Some(error.to_string()),
                }),
            }
        }
        self.set(hooks, loaded.clone());
        loaded
    }

    #[cfg(feature = "wasm-plugins")]
    async fn load_one(&self, cache_dir: &std::path::Path, path: &std::path::Path, site: Site) -> Result<Loaded, Error> {
        let runtime = self
            .runtime
            .get_or_init(|| wasm::Runtime::new(cache_dir))
            .as_ref()
            .map_err(Clone::clone)?;
        let plugin = runtime.load(path, site).await?;
        Ok(Loaded {
            name: plugin.name().to_owned(),
            capabilities: plugin.capabilities.clone(),
            hook: Arc::new(plugin),
        })
    }

    #[cfg(not(feature = "wasm-plugins"))]
    async fn load_one(
        &self,
        _cache_dir: &std::path::Path,
        _path: &std::path::Path,
        _site: Site,
    ) -> Result<Loaded, Error> {
        Err(Error::Unsupported)
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

    /// Replacements chain; notes collect in order, one line each, named for the plugin that wrote it.
    pub async fn after_tool(&self, mut result: ToolResult) -> (String, Vec<String>) {
        let mut notes = Vec::new();
        for hook in self.list() {
            match hook.after_tool(&result).await {
                AfterTool::Keep => {}
                AfterTool::Replace(output) => result.output = output,
                AfterTool::Note(note) => notes.push(note_line(hook.name(), &note)),
            }
        }
        (result.output, notes)
    }

    /// The first refusal wins; a replacement feeds the hooks after it; context collects in order with each plugin's name.
    pub async fn prompt_submit(
        &self,
        mut prompt: PromptEvent,
    ) -> Result<(String, Vec<(String, String)>), (String, String)> {
        let mut context = Vec::new();
        for hook in self.list() {
            match hook.prompt_submit(&prompt).await {
                PromptSubmit::Keep => {}
                PromptSubmit::Replace(text) => prompt.text = text,
                PromptSubmit::AddContext(text) => context.push((hook.name().to_owned(), text)),
                PromptSubmit::Deny(reason) => return Err((hook.name().to_owned(), reason)),
            }
        }
        Ok((prompt.text, context))
    }

    /// Every plugin sees the reply; notes collect, and the first that wants the turn to go on decides.
    pub async fn turn_end(&self, reply: &ReplyEvent) -> Ended {
        let mut ended = Ended::default();
        for hook in self.list() {
            match hook.turn_end(reply).await {
                TurnEnd::Accept => {}
                TurnEnd::Note(note) => ended.notes.push((hook.name().to_owned(), note)),
                TurnEnd::Continue(reason) => {
                    ended.continued = Some((hook.name().to_owned(), reason));
                    break;
                }
            }
        }
        ended
    }

    /// The first plugin that answers decides, with its name; none answering leaves the ask to the user.
    pub async fn permission(&self, ask: &PermissionAsk) -> Option<(String, PermissionDecision)> {
        for hook in self.list() {
            match hook.permission(ask).await {
                PermissionDecision::Pass => {}
                decision => return Some((hook.name().to_owned(), decision)),
            }
        }
        None
    }

    /// Every plugin's instructions for the summary, in order, each under its name.
    pub async fn compaction(&self, event: &CompactionEvent) -> Vec<String> {
        let mut out = Vec::new();
        for hook in self.list() {
            if let Compacting::Instruct(text) = hook.compaction(event).await {
                out.push(format!("From the {} plugin: {text}", hook.name()));
            }
        }
        out
    }

    pub async fn session(&self, event: &SessionEvent) {
        for hook in self.list() {
            hook.session(event).await;
        }
    }
}

/// A note's limit: enough to say what happened, not a report; the output itself holds the detail.
const NOTE_CHARS: usize = 160;

/// A plugin's note as the card and the model see it: its first line, bounded, under the plugin's name.
fn note_line(plugin: &str, note: &str) -> String {
    let line = note
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let cut = line.char_indices().nth(NOTE_CHARS).map(|(at, _)| at);
    match cut {
        Some(at) => format!("{plugin}: {}...", line[..at].trim_end()),
        None => format!("{plugin}: {line}"),
    }
}

/// A plugin's name before it has said one: its file's stem.
fn plugin_name(entry: &str) -> String {
    std::path::Path::new(entry)
        .file_stem()
        .map_or_else(|| entry.to_owned(), |stem| stem.to_string_lossy().into_owned())
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
            Event::SessionDeleted { session_id } => SessionEvent {
                id: session_id,
                workspace: String::new(),
                title: String::new(),
                agent: String::new(),
                kind: SessionKind::Deleted,
            },
            Event::SessionStatusChanged { session_id, status } => {
                let kind = if status == SessionStatus::Running {
                    SessionKind::Running
                } else {
                    SessionKind::Idle
                };
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

pub(crate) fn session_event(
    engine: &crate::Engine,
    session: &crate::session::types::Session,
    kind: SessionKind,
) -> SessionEvent {
    let workspace = engine
        .store
        .workspace(&session.workspace_id)
        .ok()
        .flatten()
        .map(|workspace| workspace.path)
        .unwrap_or_default();
    SessionEvent {
        id: session.id.clone(),
        workspace,
        title: session.title.clone(),
        agent: session.agent.clone(),
        kind,
    }
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
        ToolCall {
            session_id: "s".into(),
            workspace: "w".into(),
            agent: "build".into(),
            tool: "bash".into(),
            input: serde_json::json!({ "command": "ls" }),
        }
    }

    #[tokio::test]
    async fn a_replacement_reaches_the_next_hook_and_the_first_denial_wins() {
        let hooks = Hooks::default();
        hooks.set(
            vec![
                Arc::new(Fixed(
                    "a",
                    BeforeTool::Replace(serde_json::json!({ "command": "ls -la" })),
                    AfterTool::Note("seen".into()),
                )),
                Arc::new(Fixed(
                    "b",
                    BeforeTool::Deny("no".into()),
                    AfterTool::Replace("short".into()),
                )),
                Arc::new(Fixed("c", BeforeTool::Deny("never asked".into()), AfterTool::Keep)),
            ],
            vec![],
        );
        let (call, denied) = hooks.before_tool(call()).await;
        assert_eq!(call.input["command"], "ls -la");
        assert_eq!(denied, Some(("b".to_owned(), "no".to_owned())));
        let result = ToolResult {
            session_id: "s".into(),
            workspace: "w".into(),
            agent: "build".into(),
            tool: "bash".into(),
            input: Value::Null,
            output: "long".into(),
            failed: false,
        };
        assert_eq!(
            hooks.after_tool(result).await,
            ("short".to_owned(), vec!["a: seen".to_owned()])
        );
    }

    #[test]
    fn a_note_is_one_bounded_line_under_the_plugins_name() {
        assert_eq!(
            note_line("guard", "\n  saw it fail  \nand more\n"),
            "guard: saw it fail"
        );
        let long = "x".repeat(200);
        let line = note_line("guard", &long);
        assert_eq!(line.chars().count(), "guard: ".len() + NOTE_CHARS + 3);
        assert!(line.ends_with("..."));
    }
}
