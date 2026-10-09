use super::{Ask, ToolMetadata, canonical, display, scratch_dir, sensitive};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// Shared across one session: which files the model has read, so edits are never blind, and which
/// subdirectory instruction files it has already been shown.
#[derive(Default)]
pub struct SessionFiles {
    read: Mutex<HashSet<PathBuf>>,
    shown: Mutex<HashSet<PathBuf>>,
    /// Where the reads are kept, so they outlast a restart; none in a bare test context.
    kept: Option<(Arc<crate::store::Store>, String)>,
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
pub struct Progress(Option<Arc<dyn Fn(ToolMetadata) + Send + Sync>>);

impl SessionFiles {
    /// A session's files, starting from the reads its earlier runs kept.
    pub fn kept(store: Arc<crate::store::Store>, session_id: &str) -> Self {
        let read = store
            .read_files(session_id)
            .unwrap_or_default()
            .into_iter()
            .map(PathBuf::from)
            .collect();

        Self {
            read: Mutex::new(read),
            kept: Some((store, session_id.into())),
            ..Self::default()
        }
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

        Self {
            read: Mutex::new(read),
            shown: Mutex::new(shown),
            kept: None,
        }
    }

    pub fn absorb(&self, scratch: &SessionFiles) {
        for path in scratch.read.lock().unwrap().iter() {
            self.mark_read(path);
        }

        self.shown
            .lock()
            .unwrap()
            .extend(scratch.shown.lock().unwrap().iter().cloned());
    }
}

impl Progress {
    pub fn new(show: impl Fn(ToolMetadata) + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(show)))
    }

    pub fn show(&self, metadata: ToolMetadata) {
        if let Some(show) = &self.0 {
            show(metadata);
        }
    }
}

impl Context {
    /// The path a call really touches: absolute, `..` folded, symlinks followed. Permission rules see this.
    pub fn resolve(&self, path: &str) -> PathBuf {
        let path = Path::new(path);
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace.join(path)
        };

        canonical(&resolved)
    }

    pub fn inside_workspace(&self, path: &Path) -> bool {
        path.starts_with(&self.workspace)
    }

    /// Rules apply everywhere; workspace and scratch operations default to allow, as does reading a skill's folder.
    pub fn ask_if_outside(&self, kind: &str, path: &Path, verb: &str) -> Option<Ask> {
        let mut ask = Ask::path(kind, path, &self.workspace, format!("{verb} {}", path.display()));
        ask.default_allow = self.inside_workspace(path) || in_scratch(path) || (kind == "read" && self.in_skill(path));

        Some(ask)
    }

    /// Reading asks for anything outside the workspace and for any file likely to hold secrets, even
    /// inside it. Everything else in the workspace, the scratch directory and the folders of the
    /// skills this session was offered (which `skill` names by absolute path) is free to read.
    pub fn ask_to_read(&self, path: &Path, verb: &str) -> Option<Ask> {
        if self.owns_output(path) || ((in_scratch(path) || self.in_skill(path)) && !sensitive::is_sensitive(path)) {
            let ask = Ask::path(
                "read",
                path,
                &self.workspace,
                format!("{verb} {}", display(path, &self.workspace)),
            );
            return Some(ask.allow_by_default());
        }

        read_ask(&self.workspace, path, verb)
    }

    /// Writing evaluates policy. Scratch and workspace files default to allow (undo can put them back),
    /// except files that would widen what the agent may do or hold secrets.
    pub fn ask_to_write(&self, path: &Path, verb: &str) -> Option<Ask> {
        let mut ask = Ask::path(
            "edit",
            path,
            &self.workspace,
            format!("{verb} {}", display(path, &self.workspace)),
        );
        ask.default_allow = in_scratch(path) || (self.inside_workspace(path) && !guarded(path, &self.workspace));

        Some(ask)
    }

    /// Inside the folder of a skill this session's config offers, as opencode allows skill directories.
    fn in_skill(&self, path: &Path) -> bool {
        self.config
            .skills
            .iter()
            .any(|skill| path.starts_with(canonical(Path::new(&skill.path))))
    }

    /// Output this session's own calls spilled to disk, which their results name: reading it back asks
    /// nothing. Only this session's directory, compared as resolved paths; nothing else in the data dir.
    fn owns_output(&self, path: &Path) -> bool {
        let owned = canonical(&self.engine.data_dir.join("tool-output").join(&self.session_id));

        path.starts_with(&owned) && path != owned
    }
}

fn in_scratch(path: &Path) -> bool {
    let scratch = scratch_dir();

    path.starts_with(&scratch) && path != scratch
}

/// A workspace file an edit always asks about: Drift's own config (`drift.json`, `.drift/`), which holds
/// permission rules and agents, version-control internals, and files likely to hold secrets.
fn guarded(path: &Path, workspace: &Path) -> bool {
    let relative = path.strip_prefix(workspace).unwrap_or(path);
    let named = |name: &str| {
        relative
            .components()
            .any(|part| part.as_os_str().eq_ignore_ascii_case(name))
    };

    sensitive::is_sensitive(path)
        || named(crate::config::FILE)
        || named(".drift")
        || super::paths::VCS_DIRS.iter().any(|directory| named(directory))
}

/// The read rule without a call around it, for reads the engine makes itself (@ mentions).
pub fn read_ask(workspace: &Path, path: &Path, verb: &str) -> Option<Ask> {
    if sensitive::is_sensitive(path) {
        return Some(Ask::path(
            "read",
            path,
            workspace,
            format!("{verb} {} (it may hold secrets)", display(path, workspace)),
        ));
    }

    let mut ask = Ask::path("read", path, workspace, format!("{verb} {}", display(path, workspace)));
    ask.default_allow = path.starts_with(workspace);

    Some(ask)
}
