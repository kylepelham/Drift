use std::path::Path;

use crate::session::review::{EMPTY_TREE, git_text};
use crate::session::snapshot::plain_git;

/// Branches a review compares against, in the order the review panel picks its default base.
const BASES: [&str; 6] = [
    "main",
    "master",
    "origin/main",
    "origin/master",
    "develop",
    "origin/develop",
];
/// Untracked files counted line by line, and the largest one counted.
const MAX_COUNTED: usize = 200;
const MAX_COUNTED_BYTES: u64 = 1024 * 1024;
/// Untracked files named in the prompt.
const MAX_NAMED: usize = 20;

/// What `/review` looks at when no target is named.
#[derive(Debug, PartialEq)]
pub(super) enum Scope {
    NotRepo,
    /// Git could not list the changes, named by the command that failed; never taken for a clean tree.
    Unlisted(&'static str),
    Clean {
        base_name: String,
    },
    Changes(Changes),
}

#[derive(Debug, PartialEq)]
pub(super) struct Changes {
    /// The commit the diff starts from: the merge-base with `base_name`.
    pub base: String,
    pub base_name: String,
    /// Tracked and untracked together.
    pub files: usize,
    pub lines: usize,
    pub untracked: Vec<String>,
}

impl Changes {
    /// The sentence naming the untracked files, or nothing when there are none.
    pub(super) fn untracked_sentence(&self) -> String {
        if self.untracked.is_empty() {
            return String::new();
        }

        let named: Vec<String> = self
            .untracked
            .iter()
            .take(MAX_NAMED)
            .map(|path| format!("`{path}`"))
            .collect();
        let more = self.untracked.len().saturating_sub(MAX_NAMED);
        let tail = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };

        format!(" These untracked files are new as well: {}{tail}.", named.join(", "))
    }
}

/// The branch's work and uncommitted changes in `workspace`, measured.
pub(super) async fn resolve(workspace: &Path) -> Scope {
    if git_text(workspace, &["rev-parse", "--is-inside-work-tree"])
        .await
        .as_deref()
        != Some("true")
    {
        return Scope::NotRepo;
    }

    let (base, base_name) = base(workspace).await;
    let Some(numstat) = plain_git(workspace, &["diff", "--numstat", &base, "--"], None).await else {
        return Scope::Unlisted("git diff");
    };
    let (tracked, tracked_lines) = count(&String::from_utf8_lossy(&numstat));

    let Some(listed) = plain_git(workspace, &["ls-files", "--others", "--exclude-standard", "-z"], None).await else {
        return Scope::Unlisted("git ls-files");
    };
    let untracked: Vec<String> = String::from_utf8_lossy(&listed)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect();
    let untracked_lines = lines_in(workspace, &untracked).await;

    if tracked == 0 && untracked.is_empty() {
        return Scope::Clean { base_name };
    }
    Scope::Changes(Changes {
        base,
        base_name,
        files: tracked + untracked.len(),
        lines: tracked_lines + untracked_lines,
        untracked,
    })
}

/// The merge-base with the first base branch that is not the current one, else with the upstream, else `HEAD`.
async fn base(workspace: &Path) -> (String, String) {
    let current = git_text(workspace, &["rev-parse", "--abbrev-ref", "HEAD"]).await;
    let upstream = git_text(
        workspace,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"],
    )
    .await;
    let candidates = BASES.iter().map(|name| (*name).to_string()).chain(upstream);

    for name in candidates.filter(|name| Some(name) != current.as_ref()) {
        if let Some(base) = git_text(workspace, &["merge-base", "HEAD", &name]).await {
            return (base, name);
        }
    }

    let head = git_text(workspace, &["rev-parse", "--verify", "--quiet", "HEAD"])
        .await
        .unwrap_or_else(|| EMPTY_TREE.into());
    (head, "HEAD".into())
}

/// Files and added plus deleted lines in `git diff --numstat` output; binary files count no lines.
pub(super) fn count(numstat: &str) -> (usize, usize) {
    let mut files = 0;
    let mut lines = 0;
    for row in numstat.lines().filter(|row| !row.trim().is_empty()) {
        let mut fields = row.split('\t');
        let added: usize = fields.next().and_then(|field| field.parse().ok()).unwrap_or(0);
        let deleted: usize = fields.next().and_then(|field| field.parse().ok()).unwrap_or(0);
        files += 1;
        lines += added + deleted;
    }

    (files, lines)
}

async fn lines_in(workspace: &Path, paths: &[String]) -> usize {
    let mut total = 0;
    for path in paths.iter().take(MAX_COUNTED) {
        let full = workspace.join(path);
        let small = tokio::fs::metadata(&full)
            .await
            .is_ok_and(|meta| meta.len() <= MAX_COUNTED_BYTES);
        if !small {
            continue;
        }
        let Ok(bytes) = tokio::fs::read(&full).await else {
            continue;
        };
        let breaks = bytes.iter().filter(|byte| **byte == b'\n').count();
        let unterminated = usize::from(bytes.last().is_some_and(|byte| *byte != b'\n'));
        total += breaks + unterminated;
    }

    total
}
