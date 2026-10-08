use std::path::{Path, PathBuf};

/// Directories that belong to version control, never to the project's content.
pub(super) const VCS_DIRS: [&str; 4] = [".git", ".hg", ".svn", ".jj"];

/// A file glob as ripgrep's `--glob` reads it, which is how opencode runs both `glob` and `grep`'s
/// `include`: without a `/` it matches a file name at any depth (`*.ts`), with one it matches from
/// the root (`src/*.ts`), and `**` crosses directories.
pub struct FileGlob(ignore::overrides::Override);

impl FileGlob {
    pub fn new(root: &Path, pattern: &str) -> Result<Self, ignore::Error> {
        let mut builder = ignore::overrides::OverrideBuilder::new(root);
        builder.add(pattern.trim_start_matches("./"))?;

        builder.build().map(Self)
    }

    /// Whether a file under the root matches.
    pub fn matches(&self, path: &Path) -> bool {
        self.0.matched(path, false).is_whitelist()
    }
}

/// A directory the model may read and write in without asking, for scratch files kept out of the
/// workspace: `Drift` in the system's temporary directory. It is made when the engine opens.
pub fn scratch_dir() -> PathBuf {
    canonical(&std::env::temp_dir().join("Drift"))
}

/// Walks `root` as git would list it (ignore rules apply, hidden files included) without descending
/// into version-control internals.
pub fn walk(root: &Path) -> ignore::Walk {
    walker(root).build()
}

/// [`walk`]'s settings, for a walk that runs on several threads.
pub fn walker(root: &Path) -> ignore::WalkBuilder {
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .hidden(false)
        .require_git(false)
        .filter_entry(|entry| !entry.file_name().to_str().is_some_and(|name| VCS_DIRS.contains(&name)));

    builder
}

/// Resolves through the deepest existing ancestor, so a file that does not exist yet still lands where it will.
pub fn canonical(path: &Path) -> PathBuf {
    let mut lexical = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                lexical.pop();
            }
            std::path::Component::CurDir => {}
            other => lexical.push(other.as_os_str()),
        }
    }

    let mut existing = lexical.as_path();
    let mut missing = Vec::new();
    while !existing.exists() {
        let Some(parent) = existing.parent() else {
            return lexical;
        };
        missing.push(
            existing
                .file_name()
                .map(std::ffi::OsStr::to_os_string)
                .unwrap_or_default(),
        );
        existing = parent;
    }

    let mut resolved = existing
        .canonicalize()
        .map_or_else(|_| existing.to_path_buf(), strip_verbatim);
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }

    resolved
}

/// Windows canonical paths carry `\\?\`; nothing downstream wants it.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();

    match text.strip_prefix(r"\\?\") {
        Some(plain) if !plain.starts_with("UNC") => PathBuf::from(plain),
        _ => path,
    }
}

pub fn display(path: &Path, workspace: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
