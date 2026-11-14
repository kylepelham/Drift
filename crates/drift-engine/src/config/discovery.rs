use std::path::{Component, Path, PathBuf};

use super::frontmatter;

const INSTRUCTION_FILES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];
/// Skill roots looked up under the workspace and the home directory, in this order.
/// Skill folders in a project directory, and (without the dot for Drift's own) in the home directory.
const SKILL_DIRS: [&str; 3] = [".drift/skills", ".agents/skills", ".claude/skills"];
pub(crate) const HOME_SKILL_DIRS: [&str; 3] = [".config/drift/skills", ".agents/skills", ".claude/skills"];
/// How deep under a skill folder a `SKILL.md` is looked for; folders such as `node_modules` are never entered.
pub(super) const SKILL_DEPTH: usize = 6;
const MAX_INSTRUCTION_CHARS: usize = 40_000;
/// The most files one `instructions` glob brings in, so `**/*.md` in a large repository stays bounded.
const MAX_INSTRUCTION_MATCHES: usize = 50;

/// The files an `instructions` entry names, with the name each is shown under: the entry itself for a
/// plain path (relative to its drift.json, absolute, or `~/`), each match for a glob, walked from the
/// part of the path before its first wildcard as git lists files, in name order.
pub(super) fn instruction_files(listed: &str, resolved: &Path) -> Vec<(String, PathBuf)> {
    let text = resolved.to_string_lossy().replace('\\', "/");
    let Some(wildcard) = text.find(['*', '?', '[', '{']) else {
        return vec![(listed.to_string(), resolved.to_path_buf())];
    };

    let separator = text[..wildcard].rfind('/').unwrap_or(0);
    let base = PathBuf::from(&text[..separator]);
    let Ok(glob) = globset::GlobBuilder::new(&text).literal_separator(true).build() else {
        return Vec::new();
    };
    let matcher = glob.compile_matcher();
    let mut found: Vec<PathBuf> = crate::tool::walk(&base)
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .map(ignore::DirEntry::into_path)
        .filter(|path| matcher.is_match(path.to_string_lossy().replace('\\', "/")))
        .take(MAX_INSTRUCTION_MATCHES * 4)
        .collect();

    found.sort();
    found.truncate(MAX_INSTRUCTION_MATCHES);

    found
        .into_iter()
        .map(|path| {
            let name = path.strip_prefix(&base).map_or_else(
                |_| path.display().to_string(),
                |relative| relative.to_string_lossy().replace('\\', "/"),
            );
            (name, path)
        })
        .collect()
}

/// Skill folders in precedence order: project ancestors, configured paths, then home folders, each with the
/// folder its files must stay inside (none for the user's own).
pub(crate) fn skill_folders(
    workspace: &Path,
    home: Option<&Path>,
    listed: Vec<(PathBuf, Option<PathBuf>)>,
) -> Vec<(PathBuf, Option<PathBuf>)> {
    let project = ancestors_to_repo_root(workspace)
        .into_iter()
        .flat_map(|dir| SKILL_DIRS.map(|skills| (dir.join(skills), Some(dir.clone()))));
    let user = home
        .into_iter()
        .flat_map(|home| HOME_SKILL_DIRS.map(|skills| (home.join(skills), None)));

    project.chain(listed).chain(user).collect()
}

/// The `SKILL.md` files under `dir`, sorted, so the same tree always yields the same skills. Under `within`, a
/// link is followed only while it stays inside, so a project's skill never reads a file from elsewhere.
pub(crate) fn skill_files(dir: &Path, within: Option<&Path>) -> Vec<PathBuf> {
    let kept = |path: &Path| within.is_none_or(|root| confined(root, path));
    if !kept(dir) {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut pending = vec![(dir.to_path_buf(), 0)];

    while let Some((folder, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let linked = entry.file_type().is_ok_and(|kind| kind.is_symlink());
            let skipped = matches!(entry.file_name().to_str(), Some("node_modules" | ".git"));
            if skipped || (linked && !kept(&path)) {
                continue;
            }
            if path.is_dir() && depth < SKILL_DEPTH {
                pending.push((path, depth + 1));
            } else if entry.file_name() == "SKILL.md" {
                found.push(path);
            }
        }
    }

    found.sort();
    found
}

/// The workspace and its parents up to the repository root (the nearest holding `.git`), nearest
/// first; just the workspace when it is in no repository.
pub(super) fn ancestors_to_repo_root(workspace: &Path) -> Vec<PathBuf> {
    let chain: Vec<PathBuf> = workspace.ancestors().map(Path::to_path_buf).collect();

    match chain.iter().position(|dir| dir.join(".git").exists()) {
        Some(root) => chain[..=root].to_vec(),
        None => vec![workspace.to_path_buf()],
    }
}

/// Whether the workspace is in a git repository.
pub fn in_repository(workspace: &Path) -> bool {
    workspace.ancestors().any(|dir| dir.join(".git").exists())
}

/// The first of AGENTS.md and CLAUDE.md in `dir` that stays inside `within` and is no secret.
pub(super) fn instruction_file(dir: &Path, within: &Path) -> Option<(&'static str, String)> {
    INSTRUCTION_FILES
        .iter()
        .map(|name| (*name, dir.join(name)))
        .filter(|(_, path)| confined(within, path))
        .find_map(|(name, path)| std::fs::read_to_string(path).ok().map(|text| (name, text)))
}

/// Whether `path` is named as an instruction file is.
pub fn is_instruction_file(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| INSTRUCTION_FILES.iter().any(|file| name == *file))
}

/// Instruction files in the directories between the workspace (not included) and `file`, outermost
/// first: rules for a part of the tree that the system prompt does not carry.
pub fn nested_instructions(workspace: &Path, file: &Path) -> Vec<(PathBuf, String)> {
    let Ok(relative) = file.parent().unwrap_or(file).strip_prefix(workspace) else {
        return Vec::new();
    };

    let mut dir = workspace.to_path_buf();
    let mut found = Vec::new();
    for part in relative.components() {
        dir.push(part);
        if let Some((name, text)) = instruction_file(&dir, workspace) {
            found.push((dir.join(name), clip(&text)));
        }
    }

    found
}

/// Whether a project's entry names a path relative to its folder: no drive, share, root or `~`, so refusing
/// one touches nothing.
pub(super) fn relative_entry(path: &Path) -> bool {
    !path.to_string_lossy().starts_with(['/', '\\', '~'])
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir | Component::ParentDir))
}

/// Whether `path`, under `root` as written, stays inside it with every link resolved and is neither a secret nor
/// version-control internals (a remote's URL may carry a token). Nothing outside `root` is touched to tell.
pub(super) fn confined(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let root = crate::tool::canonical(root);
    let Some(resolved) = resolve_within(&root, relative) else {
        return false;
    };
    let in_vcs = resolved.strip_prefix(&root).is_ok_and(|inside| {
        inside.components().any(|part| {
            crate::tool::VCS_DIRS
                .iter()
                .any(|name| part.as_os_str().eq_ignore_ascii_case(name))
        })
    });

    !in_vcs && !crate::tool::sensitive::is_sensitive(&resolved)
}

/// `relative` under `root`, each link followed by reading where it points rather than opening it; `None` once a
/// step leaves `root`, so a link to another drive or a share is refused unopened.
fn resolve_within(root: &Path, relative: &Path) -> Option<PathBuf> {
    const MAX_LINKS: usize = 40;
    let mut pending = steps(relative)?;
    let mut current = root.to_path_buf();
    let mut links = 0;

    while let Some(step) = pending.pop() {
        if step == "." {
            continue;
        }
        if step == ".." {
            if current == root {
                return None;
            }
            current.pop();
            continue;
        }
        let next = current.join(&step);
        if !next.symlink_metadata().is_ok_and(|meta| meta.file_type().is_symlink()) {
            current = next;
            continue;
        }
        links += 1;
        let target = std::fs::read_link(&next).ok().filter(|_| links <= MAX_LINKS)?;
        if !relative_entry(&target) {
            // An absolute target (a junction's always is) is followed only when it names a place inside `root`.
            current = root.to_path_buf();
            pending.extend(steps(crate::tool::strip_verbatim(target).strip_prefix(root).ok()?)?);
            continue;
        }
        pending.extend(steps(&target)?);
    }

    Some(current)
}

/// A relative path's parts, last first, to be taken off the end one by one.
fn steps(path: &Path) -> Option<Vec<std::ffi::OsString>> {
    relative_entry(path).then(|| {
        path.components()
            .rev()
            .map(|part| part.as_os_str().to_owned())
            .collect()
    })
}

/// A Markdown file's text after its front matter.
pub fn body(text: &str) -> String {
    frontmatter::parse(text).body
}

/// The Markdown files directly in `dir`, by name; under `within`, only those that stay inside it, as [`confined`] says.
pub(super) fn markdown_files(dir: &Path, within: Option<&Path>) -> Vec<(String, frontmatter::Document)> {
    let kept = |path: &Path| within.is_none_or(|root| confined(root, path));
    if !kept(dir) {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut files: Vec<(String, frontmatter::Document)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md") && kept(path))
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            let name = path.file_stem()?.to_string_lossy().into_owned();
            Some((name, frontmatter::parse(&text)))
        })
        .collect();

    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

/// An instruction file's text as prompts carry it: trimmed, and cut past the limit.
pub fn clip(text: &str) -> String {
    if text.chars().count() <= MAX_INSTRUCTION_CHARS {
        return text.trim().into();
    }

    let kept = text.chars().take(MAX_INSTRUCTION_CHARS).collect::<String>();
    format!("{kept}\n\n(truncated)")
}

pub fn home() -> Option<PathBuf> {
    // Tests pass a home directory so they never read the user's real config.
    if cfg!(test) {
        return None;
    }

    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}
