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
/// workspace: `scratch` in Drift's temporary folder. It is made when the engine opens.
pub fn scratch_dir() -> PathBuf {
    canonical(&crate::platform::temp::root().join("scratch"))
}

/// Folders a search skips unless its `path` is in one or it asks for everything: dependencies, build output, caches.
const HEAVY_DIRS: [&str; 8] = [
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "__pycache__",
    ".next",
    ".gradle",
];
/// Folders at the top of a drive or the home folder that a search there skips too (`Program Files*` by prefix).
const SYSTEM_DIRS: [&str; 5] = ["Windows", "Program Files", "ProgramData", "AppData", "$Recycle.Bin"];

/// Walks `root` as git would list it (ignore rules apply, hidden files included) without descending
/// into version-control internals.
pub fn walk(root: &Path) -> ignore::Walk {
    walker(root).build()
}

/// [`walk`]'s settings, for a walk that runs on several threads.
pub fn walker(root: &Path) -> ignore::WalkBuilder {
    keeping(root, |_| true)
}

fn keeping(root: &Path, keep: impl Fn(&ignore::DirEntry) -> bool + Send + Sync + 'static) -> ignore::WalkBuilder {
    let mut builder = ignore::WalkBuilder::new(root);
    builder.hidden(false).require_git(false).filter_entry(move |entry| {
        !entry.file_name().to_str().is_some_and(|name| VCS_DIRS.contains(&name)) && keep(entry)
    });

    builder
}

/// The folders a search walk left out by name, to say so.
#[derive(Default)]
pub struct Skipped(std::sync::Mutex<std::collections::BTreeSet<String>>);

impl Skipped {
    pub fn names(&self) -> Vec<String> {
        self.0.lock().unwrap().iter().cloned().collect()
    }
}

/// A `grep` or `glob` walk: [`walker`] without [`HEAVY_DIRS`], and at the top of a drive or the home folder without
/// [`SYSTEM_DIRS`], unless `everything`, which also drops ignore rules, or `root` is inside such a folder already.
pub fn search_walker(root: &Path, everything: bool, skipped: &std::sync::Arc<Skipped>) -> ignore::WalkBuilder {
    let named = root
        .components()
        .any(|part| part.as_os_str().to_str().is_some_and(|name| HEAVY_DIRS.contains(&name)));
    if everything || named {
        let mut builder = walker(root);
        builder.standard_filters(!everything).hidden(false);
        return builder;
    }

    let (system, skipped) = (top_of_system(root), skipped.clone());
    keeping(root, move |entry| {
        let skip = entry.file_type().is_some_and(|kind| kind.is_dir())
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| HEAVY_DIRS.contains(&name) || (system && entry.depth() == 1 && system_dir(name)));
        if skip {
            skipped
                .0
                .lock()
                .unwrap()
                .insert(entry.file_name().to_string_lossy().into_owned());
        }
        !skip
    })
}

fn system_dir(name: &str) -> bool {
    SYSTEM_DIRS.contains(&name) || name.starts_with("Program Files")
}

/// A drive's root or the home folder, where system folders sit beside the user's own.
fn top_of_system(root: &Path) -> bool {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(|home| canonical(Path::new(&home)));

    root.parent().is_none() || home.is_some_and(|home| home == root)
}

/// Resolves through the deepest existing ancestor, so a file that does not exist yet still lands where it will.
pub fn canonical(path: &Path) -> PathBuf {
    let mut lexical = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                if matches!(
                    lexical.components().next_back(),
                    None | Some(std::path::Component::ParentDir)
                ) {
                    lexical.push("..");
                } else {
                    lexical.pop();
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leading_parent_step_stays_a_step_out_of_the_current_directory() {
        let here = std::env::current_dir().unwrap();
        let parent = canonical(here.parent().unwrap());
        assert_eq!(canonical(Path::new("../drift-missing")), parent.join("drift-missing"));
        assert_eq!(
            canonical(Path::new("a/../../drift-missing")),
            parent.join("drift-missing")
        );
    }
}
