//! Tools the model can call. Each one declares its schema, its permission and how to run.

pub mod apply_patch;
pub mod bash;
pub mod command;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod patch;
pub mod question;
pub mod read;
pub mod sensitive;
pub mod skill;
pub mod spool;
pub mod task;
pub mod todo;
pub mod webfetch;
pub mod write;

use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;

use crate::llm::catalog::ToolProfile;
use crate::llm::ToolSpec;

/// Shared across one session: which files the model has read, so edits are never blind.
#[derive(Default)]
pub struct SessionFiles {
    read: Mutex<HashSet<PathBuf>>,
}

impl SessionFiles {
    pub fn mark_read(&self, path: &Path) {
        self.read.lock().unwrap().insert(path.to_path_buf());
    }

    pub fn was_read(&self, path: &Path) -> bool {
        self.read.lock().unwrap().contains(path)
    }
}

pub struct Context {
    pub workspace: PathBuf,
    pub session_id: String,
    pub message_id: String,
    pub call_id: String,
    pub files: Arc<SessionFiles>,
    pub abort: CancellationToken,
    /// Session-level tools (todos, questions) read and write through the engine.
    pub engine: Arc<crate::Engine>,
}

impl Context {
    /// The path a call really touches: absolute, `..` folded, symlinks followed. Permission rules see this.
    pub fn resolve(&self, path: &str) -> PathBuf {
        let path = Path::new(path);
        canonical(&if path.is_absolute() { path.to_path_buf() } else { self.workspace.join(path) })
    }

    pub fn inside_workspace(&self, path: &Path) -> bool {
        path.starts_with(&self.workspace)
    }

    /// An ask for anything outside the workspace; reads inside it are free.
    pub fn ask_if_outside(&self, kind: &str, path: &Path, verb: &str) -> Option<Ask> {
        if self.inside_workspace(path) {
            return None;
        }
        Some(Ask::new(kind, path.to_string_lossy(), format!("{verb} {}", path.display())))
    }

    /// Reading asks for anything outside the workspace and for any file likely to hold secrets, even
    /// inside it. Everything else in the workspace is free to read.
    pub fn ask_to_read(&self, path: &Path, verb: &str) -> Option<Ask> {
        if self.owns_output(path) {
            return None;
        }
        read_ask(&self.workspace, path, verb)
    }

    /// Output this session's own calls spilled to disk, which their results name: reading it back asks
    /// nothing. Only this session's directory, compared as resolved paths; nothing else in the data dir.
    fn owns_output(&self, path: &Path) -> bool {
        let owned = canonical(&self.engine.data_dir.join("tool-output").join(&self.session_id));
        path.starts_with(&owned) && path != owned
    }
}

/// The read rule without a call around it, for reads the engine makes itself (@ mentions).
pub fn read_ask(workspace: &Path, path: &Path, verb: &str) -> Option<Ask> {
    if sensitive::is_sensitive(path) {
        return Some(Ask::new("read", path.to_string_lossy(), format!("{verb} {} (it may hold secrets)", display(path, workspace))));
    }
    if path.starts_with(workspace) {
        return None;
    }
    Some(Ask::new("read", path.to_string_lossy(), format!("{verb} {}", path.display())))
}

/// Directories that belong to version control, never to the project's content.
const VCS_DIRS: [&str; 4] = [".git", ".hg", ".svn", ".jj"];

/// Walks `root` as git would list it (ignore rules apply, hidden files included) without descending
/// into version-control internals.
pub fn walk(root: &Path) -> ignore::Walk {
    ignore::WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|entry| !entry.file_name().to_str().is_some_and(|name| VCS_DIRS.contains(&name)))
        .build()
}

/// Resolves through the deepest existing ancestor, so a file that does not exist yet still lands where it will.
pub fn canonical(path: &Path) -> PathBuf {
    let mut lexical = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                lexical.pop();
            }
            std::path::Component::CurDir => {}
            other => lexical.push(other.as_os_str()),
        }
    }
    let mut existing = lexical.as_path();
    let mut rest = Vec::new();
    while !existing.exists() {
        let Some(parent) = existing.parent() else { return lexical };
        rest.push(existing.file_name().map(|n| n.to_os_string()).unwrap_or_default());
        existing = parent;
    }
    let mut out = existing.canonicalize().map(strip_verbatim).unwrap_or_else(|_| existing.to_path_buf());
    for part in rest.into_iter().rev() {
        out.push(part);
    }
    out
}

/// Windows canonical paths carry `\\?\`; nothing downstream wants it.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(plain) if !plain.starts_with("UNC") => PathBuf::from(plain),
        _ => path,
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Output {
    pub title: String,
    pub output: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

impl Output {
    pub fn new(title: impl Into<String>, output: impl Into<String>) -> Self {
        Self { title: title.into(), output: output.into(), metadata: Value::Null }
    }
}

/// Anything that goes back to the model as an error result. Text is written for the model.
#[derive(Debug, PartialEq)]
pub struct ToolError(pub String);

impl<E: std::fmt::Display> From<E> for ToolError {
    fn from(error: E) -> Self {
        Self(error.to_string())
    }
}

/// What a call wants to do, for the permission service to judge before it runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Ask {
    /// `read`, `edit`, `bash`, ...: the rule namespace.
    pub kind: String,
    /// The thing being touched: a path, a command. Rules match it with globs.
    pub pattern: String,
    pub title: String,
    /// For a shell command, the simple commands it runs, each judged on its own; `None` when the line
    /// hides what it runs, so only an exact approval of the whole line allows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<String>>,
    /// Files the shell line's redirections write. Any at all and only an exact approval of the whole
    /// line allows it: approving `git status` never approves `git status > victim.txt`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writes: Vec<String>,
    /// `commands` as deny rules also see them (assignments dropped, aliases spelt out), one for one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub canonical: Vec<String>,
}

impl Ask {
    pub fn new(kind: &str, pattern: impl Into<String>, title: impl Into<String>) -> Self {
        Self { kind: kind.into(), pattern: pattern.into(), title: title.into(), commands: None, writes: Vec::new(), canonical: Vec::new() }
    }

    /// A shell ask as the dialect reads `line`.
    pub fn shell(dialect: command::Dialect, line: &str, title: impl Into<String>) -> Self {
        let mut ask = Self::new("bash", line, title);
        if let Some(split) = command::split(dialect, line) {
            ask.commands = Some(split.commands);
            ask.canonical = split.canonical;
            ask.writes = split.writes;
        }
        ask
    }

    /// Keeps only the commands `keep` says, their canonical forms with them.
    pub fn retain_commands(&mut self, mut keep: impl FnMut(&str) -> bool) {
        let Some(commands) = self.commands.take() else { return };
        let canonical = std::mem::take(&mut self.canonical);
        let pairs: Vec<(String, String)> = commands.into_iter().zip(canonical.into_iter().chain(std::iter::repeat(String::new()))).filter(|(command, _)| keep(command)).collect();
        self.canonical = pairs.iter().map(|(_, canonical)| canonical.clone()).collect();
        self.commands = Some(pairs.into_iter().map(|(command, _)| command).collect());
    }
}

pub type RunFuture<'a> = Pin<Box<dyn Future<Output = Result<Output, ToolError>> + Send + 'a>>;

pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    /// `None` means the call needs no permission at all.
    fn ask(&self, ctx: &Context, input: &Value) -> Option<Ask>;
    /// Everything the call must be allowed, each judged on its own; any refusal refuses the call.
    fn asks(&self, ctx: &Context, input: &Value) -> Vec<Ask> {
        self.ask(ctx, input).into_iter().collect()
    }
    /// Whether this call writes outside memory, so what it changes is recorded for undo.
    fn mutates(&self) -> bool {
        false
    }
    /// The files a writing call will change, when it can say up front. `None` means anything might
    /// change, so the workspace is compared before and after instead.
    fn touches(&self, _ctx: &Context, _input: &Value) -> Option<Vec<PathBuf>> {
        None
    }
    /// A result that still reports failure, for tools whose failures carry metadata the UI needs.
    fn failed(&self, _output: &Output) -> bool {
        false
    }
    /// Metadata to show while the call runs, before its result exists.
    fn running_metadata(&self, _ctx: &Context, _input: &Value) -> Option<Value> {
        None
    }
    /// The call returns promptly by itself once `ctx.abort` fires, with a result worth keeping
    /// (partial output). Otherwise a stop drops the call and records it as aborted.
    fn stops_itself(&self) -> bool {
        false
    }
    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a>;
}

pub struct Registry {
    builtin: Vec<Arc<dyn Tool>>,
    /// Tools that come and go with MCP servers; replaced wholesale when a server connects or drops.
    dynamic: std::sync::RwLock<Vec<Arc<dyn Tool>>>,
}

impl Registry {
    pub fn builtin() -> Self {
        Self {
            builtin: vec![
                Arc::new(read::Read),
                Arc::new(write::Write),
                Arc::new(edit::Edit),
                Arc::new(apply_patch::ApplyPatch),
                Arc::new(bash::Bash::detect()),
                Arc::new(glob::Glob),
                Arc::new(grep::Grep),
                Arc::new(webfetch::WebFetch),
                Arc::new(todo::TodoWrite),
                Arc::new(question::Question),
                Arc::new(skill::Skill),
                Arc::new(task::Task),
                Arc::new(task::TaskOutput),
                Arc::new(task::TaskStop),
                Arc::new(task::ReadThread),
            ],
            dynamic: Default::default(),
        }
    }

    /// The model's profile decides how it edits: search/replace tools or the patch format it was trained on.
    pub fn specs(&self, profile: ToolProfile) -> Vec<ToolSpec> {
        let hidden: &[&str] = match profile {
            ToolProfile::Edit => &["apply_patch"],
            ToolProfile::ApplyPatch => &["edit", "write"],
        };
        self.all().iter().map(|tool| tool.spec()).filter(|spec| !hidden.contains(&spec.name.as_str())).collect()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.all().into_iter().find(|tool| tool.spec().name == name)
    }

    pub fn set_dynamic(&self, tools: Vec<Arc<dyn Tool>>) {
        *self.dynamic.write().unwrap() = tools;
    }

    fn all(&self) -> Vec<Arc<dyn Tool>> {
        self.builtin.iter().cloned().chain(self.dynamic.read().unwrap().iter().cloned()).collect()
    }
}

fn required_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    input[key]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ToolError(format!("`{key}` is required")))
}

pub fn display(path: &Path, workspace: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) struct Sandbox {
        pub ctx: Context,
    }

    impl Sandbox {
        pub(crate) fn new(name: &str) -> Self {
            let _ = rustls::crypto::ring::default_provider().install_default();
            let root = std::env::temp_dir().join(format!("drift-tool-{name}-{}", crate::random_hex(4)));
            let workspace = root.join("ws");
            std::fs::create_dir_all(&workspace).unwrap();
            let workspace = canonical(&workspace);
            let engine = crate::Engine::open_with(&root.join("data"), crate::Options { file_credentials: true, ..Default::default() }).unwrap();
            Self {
                ctx: Context {
                    workspace,
                    session_id: "ses_test".into(),
                    message_id: "msg_test".into(),
                    call_id: "call_test".into(),
                    files: Arc::new(SessionFiles::default()),
                    abort: CancellationToken::new(),
                    engine,
                },
            }
        }

        pub(crate) fn ctx_clone(&self) -> Context {
            Context {
                workspace: self.ctx.workspace.clone(),
                session_id: self.ctx.session_id.clone(),
                message_id: self.ctx.message_id.clone(),
                call_id: self.ctx.call_id.clone(),
                files: self.ctx.files.clone(),
                abort: self.ctx.abort.clone(),
                engine: self.ctx.engine.clone(),
            }
        }

        pub(crate) fn file(&self, path: &str, content: &str) -> PathBuf {
            let full = self.ctx.workspace.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, content).unwrap();
            full
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.ctx.workspace.parent().unwrap());
        }
    }

    #[test]
    fn registry_exposes_every_builtin_with_a_schema() {
        let registry = Registry::builtin();
        let names: Vec<String> = registry.specs(ToolProfile::Edit).into_iter().map(|spec| spec.name).collect();
        assert_eq!(names, ["read", "write", "edit", "bash", "glob", "grep", "webfetch", "todowrite", "question", "skill", "task", "task_output", "task_stop", "read_thread"]);
        let patching: Vec<String> = registry.specs(ToolProfile::ApplyPatch).into_iter().map(|spec| spec.name).collect();
        assert_eq!(patching, ["read", "apply_patch", "bash", "glob", "grep", "webfetch", "todowrite", "question", "skill", "task", "task_output", "task_stop", "read_thread"]);
        for spec in registry.specs(ToolProfile::Edit).into_iter().chain(registry.specs(ToolProfile::ApplyPatch)) {
            assert_eq!(spec.input_schema["type"], "object", "{}", spec.name);
            assert!(!spec.description.is_empty(), "{}", spec.name);
        }
        assert!(registry.get("read").is_some());
        assert!(registry.get("nope").is_none());
    }
}
