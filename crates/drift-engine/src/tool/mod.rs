//! Tools the model can call. Each one declares its schema, its permission and how to run.

pub mod apply_patch;
pub mod bash;
pub mod command;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod image;
pub(crate) mod lock;
pub mod patch;
pub mod question;
pub mod read;
pub mod schema;
pub mod sensitive;
pub mod skill;
pub mod spool;
pub(crate) mod stage;
pub mod task;
mod text;
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

/// Shared across one session: which files the model has read, so edits are never blind, and which
/// subdirectory instruction files it has already been shown.
#[derive(Default)]
pub struct SessionFiles {
    read: Mutex<HashSet<PathBuf>>,
    shown: Mutex<HashSet<PathBuf>>,
    /// Where the reads are kept, so they outlast a restart; none in a bare test context.
    kept: Option<(Arc<crate::store::Store>, String)>,
}

impl SessionFiles {
    /// A session's files, starting from the reads its earlier runs kept.
    pub fn kept(store: Arc<crate::store::Store>, session_id: &str) -> Self {
        let read = store.read_files(session_id).unwrap_or_default().into_iter().map(PathBuf::from).collect();
        Self { read: Mutex::new(read), kept: Some((store, session_id.into())), ..Self::default() }
    }

    /// True the first time an instruction file is shown in this session; later reads near it say nothing.
    pub fn first_showing(&self, path: &Path) -> bool {
        self.shown.lock().unwrap().insert(path.to_path_buf())
    }

    /// After a compaction the reads that carried instruction files are summarised away; show them again.
    pub fn forget_shown(&self) {
        self.shown.lock().unwrap().clear();
    }

    pub fn mark_read(&self, path: &Path) {
        let new = self.read.lock().unwrap().insert(path.to_path_buf());
        if let (true, Some((store, session_id))) = (new, &self.kept) {
            let _ = store.mark_read(session_id, &path.to_string_lossy());
        }
    }

    pub fn was_read(&self, path: &Path) -> bool {
        self.read.lock().unwrap().contains(path)
    }

    /// Isolates speculative reads until their result is accepted via [`Self::absorb`].
    pub fn scratch(&self) -> Self {
        let read = self.read.lock().unwrap().clone();
        let shown = self.shown.lock().unwrap().clone();
        Self { read: Mutex::new(read), shown: Mutex::new(shown), kept: None }
    }

    pub fn absorb(&self, scratch: &SessionFiles) {
        for path in scratch.read.lock().unwrap().iter() {
            self.mark_read(path);
        }
        self.shown.lock().unwrap().extend(scratch.shown.lock().unwrap().iter().cloned());
    }
}

pub struct Context {
    pub workspace: PathBuf,
    pub session_id: String,
    pub agent: String,
    pub message_id: String,
    pub call_id: String,
    pub files: Arc<SessionFiles>,
    pub abort: CancellationToken,
    /// Session-level tools (todos, questions) read and write through the engine.
    pub engine: Arc<crate::Engine>,
    /// The configuration the turn was admitted with; a call never reads a newer one.
    pub config: Arc<crate::config::Config>,
    /// Shows what a running call has done so far on its part; unset outside a turn.
    pub progress: Progress,
    /// The model a user's command chose, set only on the call the engine makes for that command.
    pub command_model: Option<crate::session::types::ModelRef>,
}

/// Metadata a running call publishes as it goes (a command's output so far). It is shown, never
/// stored: the part's saved state is its result.
#[derive(Clone, Default)]
pub struct Progress(Option<Arc<dyn Fn(Value) + Send + Sync>>);

impl Progress {
    pub fn new(show: impl Fn(Value) + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(show)))
    }

    pub fn show(&self, metadata: Value) {
        if let Some(show) = &self.0 {
            show(metadata);
        }
    }
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

    /// Rules apply everywhere; workspace and scratch operations default to allow.
    pub fn ask_if_outside(&self, kind: &str, path: &Path, verb: &str) -> Option<Ask> {
        let mut ask = Ask::path(kind, path, &self.workspace, format!("{verb} {}", path.display()));
        ask.default_allow = self.inside_workspace(path) || in_scratch(path);
        Some(ask)
    }

    /// Reading asks for anything outside the workspace and for any file likely to hold secrets, even
    /// inside it. Everything else in the workspace, and the scratch directory, is free to read.
    pub fn ask_to_read(&self, path: &Path, verb: &str) -> Option<Ask> {
        if self.owns_output(path) || (in_scratch(path) && !sensitive::is_sensitive(path)) {
            return Some(Ask::path("read", path, &self.workspace, format!("{verb} {}", display(path, &self.workspace))).allow_by_default());
        }
        read_ask(&self.workspace, path, verb)
    }

    /// Writing evaluates policy; scratch writes default to allow.
    pub fn ask_to_write(&self, path: &Path, verb: &str) -> Option<Ask> {
        let mut ask = Ask::path("edit", path, &self.workspace, format!("{verb} {}", display(path, &self.workspace)));
        ask.default_allow = in_scratch(path);
        Some(ask)
    }

    /// Output this session's own calls spilled to disk, which their results name: reading it back asks
    /// nothing. Only this session's directory, compared as resolved paths; nothing else in the data dir.
    fn owns_output(&self, path: &Path) -> bool {
        let owned = canonical(&self.engine.data_dir.join("tool-output").join(&self.session_id));
        path.starts_with(&owned) && path != owned
    }
}

/// A directory the model may read and write in without asking, for scratch files kept out of the
/// workspace: `Drift` in the system's temporary directory. It is made when the engine opens.
pub fn scratch_dir() -> PathBuf {
    canonical(&std::env::temp_dir().join("Drift"))
}

fn in_scratch(path: &Path) -> bool {
    let scratch = scratch_dir();
    path.starts_with(&scratch) && path != scratch
}

/// The read rule without a call around it, for reads the engine makes itself (@ mentions).
pub fn read_ask(workspace: &Path, path: &Path, verb: &str) -> Option<Ask> {
    if sensitive::is_sensitive(path) {
        return Some(Ask::path("read", path, workspace, format!("{verb} {} (it may hold secrets)", display(path, workspace))));
    }
    let mut ask = Ask::path("read", path, workspace, format!("{verb} {}", display(path, workspace)));
    ask.default_allow = path.starts_with(workspace);
    Some(ask)
}

/// Directories that belong to version control, never to the project's content.
const VCS_DIRS: [&str; 4] = [".git", ".hg", ".svn", ".jj"];

/// Walks `root` as git would list it (ignore rules apply, hidden files included) without descending
/// into version-control internals.
pub fn walk(root: &Path) -> ignore::Walk {
    walker(root).build()
}

/// [`walk`]'s settings, for a walk that runs on several threads.
pub fn walker(root: &Path) -> ignore::WalkBuilder {
    let mut builder = ignore::WalkBuilder::new(root);
    builder.hidden(false).require_git(false).filter_entry(|entry| !entry.file_name().to_str().is_some_and(|name| VCS_DIRS.contains(&name)));
    builder
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

/// The most of a proposed change an approval shows; the rest is said to be cut.
pub const MAX_ASK_DIFF: usize = 64 * 1024;

fn clip_diff(diff: &str) -> String {
    if diff.len() <= MAX_ASK_DIFF {
        return diff.to_string();
    }
    let cut = diff[..diff.floor_char_boundary(MAX_ASK_DIFF)].rfind('\n').unwrap_or(0);
    format!("{}\n... (the rest of the change is not shown)", &diff[..cut])
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
    /// A path inside the workspace, relative with `/`, so a committed rule such as `src/**` matches it too.
    #[serde(skip)]
    pub relative: Option<String>,
    /// Proposed diff for review, excluded from permission rule and approval matching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    /// Default decision when no explicit rule or approval matches; never skips policy evaluation.
    #[serde(skip)]
    pub default_allow: bool,
}

impl Ask {
    pub fn new(kind: &str, pattern: impl Into<String>, title: impl Into<String>) -> Self {
        Self { kind: kind.into(), pattern: pattern.into(), title: title.into(), commands: None, writes: Vec::new(), canonical: Vec::new(), relative: None, diff: None, default_allow: false }
    }

    pub fn allow_by_default(mut self) -> Self {
        self.default_allow = true;
        self
    }

    /// The ask with the change it would make, cut to [`MAX_ASK_DIFF`] bytes at a line.
    pub fn with_diff(mut self, diff: Option<String>) -> Self {
        self.diff = diff.filter(|diff| !diff.is_empty()).map(|diff| clip_diff(&diff));
        self
    }

    /// An ask about a file: the absolute path, and the workspace-relative one when it is inside.
    pub fn path(kind: &str, path: &Path, workspace: &Path, title: impl Into<String>) -> Self {
        let relative = path.strip_prefix(workspace).ok().filter(|r| !r.as_os_str().is_empty()).map(|r| r.to_string_lossy().replace('\\', "/"));
        Self { relative, ..Self::new(kind, path.to_string_lossy(), title) }
    }

    /// What rules and approvals are matched against: the pattern, then the relative path if there is one.
    pub fn targets(&self) -> Vec<&str> {
        std::iter::once(self.pattern.as_str()).chain(self.relative.as_deref()).collect()
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
    /// The MCP server a tool comes from; built-ins have none.
    fn server(&self) -> Option<&str> {
        None
    }
    /// `None` means the call needs no permission at all.
    fn ask(&self, ctx: &Context, input: &Value) -> Option<Ask>;
    /// Everything the call must be allowed, each judged on its own; any refusal refuses the call.
    fn asks(&self, ctx: &Context, input: &Value) -> Vec<Ask> {
        let ask = self.ask(ctx, input).unwrap_or_else(|| Ask::new(&self.spec().name, "*", format!("Use {}", self.spec().name)).allow_by_default());
        vec![ask]
    }
    /// Whether this tool can write outside memory, so what its calls change is recorded for undo.
    fn mutates(&self) -> bool {
        false
    }
    /// Whether this particular call may write; a tool that can tell a call that only reads says so here.
    fn call_mutates(&self, _input: &Value) -> bool {
        self.mutates()
    }
    /// Whether a read-only agent may make this call: nothing it does, or sets going, changes anything.
    fn stays_read_only(&self, _ctx: &Context, input: &Value) -> bool {
        !self.call_mutates(input)
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
    /// May run speculatively while streaming; the ordering and result rules are in docs/engine-rewrite.md.
    fn starts_early(&self) -> bool {
        false
    }
    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a>;
}

/// The built-in tools. MCP tools come from the connected servers themselves (`Engine::offered_tools`).
pub struct Registry {
    builtin: Vec<Arc<dyn Tool>>,
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
        }
    }

    /// The model's profile decides how it edits: search/replace tools or the patch format it was trained on.
    pub fn specs(&self, profile: ToolProfile) -> Vec<ToolSpec> {
        self.offered(profile).iter().map(|tool| tool.spec()).collect()
    }

    pub fn offered(&self, profile: ToolProfile) -> Vec<Arc<dyn Tool>> {
        let hidden: &[&str] = match profile {
            ToolProfile::Edit => &["apply_patch"],
            ToolProfile::ApplyPatch => &["edit", "write"],
        };
        self.builtin.iter().filter(|tool| !hidden.contains(&tool.spec().name.as_str())).cloned().collect()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.builtin.iter().find(|tool| tool.spec().name == name).cloned()
    }
}

impl crate::Engine {
    /// Every tool a turn starting now could be offered: the built-ins for the profile, then every connected server's.
    pub fn offered_tools(&self, profile: ToolProfile) -> Vec<Arc<dyn Tool>> {
        self.tools.offered(profile).into_iter().chain(self.mcp.tools(&self.store)).collect()
    }

    pub fn tool_specs(&self, profile: ToolProfile) -> Vec<ToolSpec> {
        self.offered_tools(profile).iter().map(|tool| tool.spec()).collect()
    }
}

pub(crate) fn required_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    input[key]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ToolError(format!("`{key}` is required")))
}

/// Refuses, before anything is written, a file too large for undo to keep: it could never be put back.
fn fits_history(name: &str, bytes: usize) -> Result<(), ToolError> {
    let limit = crate::session::snapshot::MAX_RECORDED_BYTES;
    if bytes as u64 > limit {
        return Err(ToolError(format!("{name} would be {} MB, over the {} MB undo can keep, so it was not written", bytes / 1024 / 1024, limit / 1024 / 1024)));
    }
    Ok(())
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
                    agent: "build".into(),
                    config: Arc::new(engine.workspace_config(&workspace)),
                    progress: Default::default(),
                    command_model: None,
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
                agent: self.ctx.agent.clone(),
                workspace: self.ctx.workspace.clone(),
                session_id: self.ctx.session_id.clone(),
                message_id: self.ctx.message_id.clone(),
                call_id: self.ctx.call_id.clone(),
                files: self.ctx.files.clone(),
                abort: self.ctx.abort.clone(),
                engine: self.ctx.engine.clone(),
                config: self.ctx.config.clone(),
                progress: self.ctx.progress.clone(),
                command_model: self.ctx.command_model.clone(),
            }
        }

        pub(crate) fn file(&self, path: &str, content: &str) -> PathBuf {
            let full = self.ctx.workspace.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, content).unwrap();
            full
        }

        /// The configuration as a turn admitted now would see it.
        pub(crate) fn reload_config(&mut self) {
            self.ctx.config = Arc::new(self.ctx.engine.workspace_config(&self.ctx.workspace));
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

    #[test]
    fn the_scratch_directory_is_free_to_read_and_write_and_the_rest_of_temp_is_not() {
        let sandbox = Sandbox::new("scratch");
        let scratch = scratch_dir().join(format!("notes-{}.txt", crate::random_hex(4)));
        let elsewhere = canonical(&std::env::temp_dir().join("not-drift").join("notes.txt"));
        assert!(sandbox.ctx.ask_to_write(&scratch, "Write").unwrap().default_allow && sandbox.ctx.ask_to_read(&scratch, "Read").unwrap().default_allow);
        assert!(sandbox.ctx.ask_to_write(&elsewhere, "Write").is_some() && sandbox.ctx.ask_to_read(&elsewhere, "Read").is_some());
        assert!(sandbox.ctx.ask_to_write(&scratch_dir(), "Write").is_some(), "the directory itself is not a file to write");
        assert!(sandbox.ctx.ask_to_read(&scratch_dir().join(".env"), "Read").is_some(), "a secret is a secret even there");
    }
}
