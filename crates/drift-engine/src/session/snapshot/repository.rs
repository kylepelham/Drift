use super::git::plain_git;
use super::*;

/// The repository `workspace` is the top of, with its index and object stores; `None` otherwise.
pub(super) async fn find_source(workspace: &Path) -> Option<Source> {
    let output = plain_git(
        workspace,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
        ],
        None,
    )
    .await?;
    let text = String::from_utf8_lossy(&output);
    let [top, git_dir, common] = text.lines().collect::<Vec<_>>()[..] else {
        return None;
    };
    if crate::tool::canonical(Path::new(top)) != crate::tool::canonical(workspace) {
        return None;
    }

    let objects = PathBuf::from(common).join("objects");
    let chained = std::fs::read_to_string(objects.join("info/alternates")).unwrap_or_default();
    let more = chained
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| objects.join(line));
    let stores: Vec<PathBuf> = std::iter::once(objects.clone())
        .chain(more)
        .filter(|directory| directory.is_dir())
        .collect();
    let index = PathBuf::from(git_dir).join("index");
    if stores.is_empty() || !index.is_file() {
        return None;
    }

    Some(Source {
        index,
        objects: std::env::join_paths(stores).ok()?,
    })
}

/// Drops timestamp-only changes when the repository's filtered bytes still match the recorded before blob.
pub(super) async fn unconverted_changes(workspace: &Path, changes: Vec<FileChange>) -> Vec<FileChange> {
    let modified: Vec<_> = changes
        .iter()
        .filter(|change| change.before.is_some() && change.after.is_some())
        .collect();
    if modified.is_empty() {
        return changes;
    }

    let mut paths = String::new();
    for change in &modified {
        let _ = writeln!(paths, "{}", change.path);
    }
    let Some(hashed) = plain_git(workspace, &["hash-object", "--stdin-paths"], Some(paths.as_bytes())).await else {
        return changes;
    };

    let hashed = String::from_utf8_lossy(&hashed);
    let same: HashSet<_> = modified
        .iter()
        .zip(hashed.lines())
        .filter(|(change, id)| change.before.as_deref() == Some(id.trim()))
        .map(|(change, _)| change.path.clone())
        .collect();

    changes
        .into_iter()
        .filter(|change| !same.contains(&change.path))
        .collect()
}
