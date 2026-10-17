//! What a turn, the working tree, a branch or a commit changed, as whole-file line diffs for the review panel.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};
use utoipa::ToSchema;

use super::revert::{Endpoint, is_prompt};
use super::snapshot::plain_git;
use crate::Engine;

/// Either side past this size is listed without its lines.
const MAX_DIFF_BYTES: usize = 1024 * 1024;
/// A review lists at most this many files.
const MAX_FILES: usize = 300;
const DIFF_TIMEOUT: Duration = Duration::from_secs(2);
const RECENT_COMMITS: &str = "50";
/// Git's tree with nothing in it, the base of a repository without commits.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

#[derive(Debug, Default, Serialize, ToSchema)]
pub struct Review {
    pub files: Vec<ReviewFile>,
    /// Changed files past the listed ones.
    pub omitted: usize,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReviewFile {
    /// Relative to the workspace with `/` separators, or absolute for a file outside it.
    pub path: String,
    pub status: FileStatus,
    pub additions: usize,
    pub deletions: usize,
    pub preview: Preview,
    /// Both versions merged line by line, each line prefixed with ` `, `+` or `-`; empty unless `preview` is `lines`.
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
}

/// Whether the file's lines are listed, and why not.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Preview {
    Lines,
    Binary,
    Large,
    /// Its recorded content is gone, as when the workspace that wrote it was removed.
    Unavailable,
}

/// What a workspace review compares the working tree with.
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitSource {
    /// `HEAD`, untracked files included.
    Uncommitted,
    /// Where the current branch left `target`, untracked files included.
    Branch,
    /// Only what commit `target` changed.
    Commit,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReviewRefs {
    /// The workspace is inside a git repository; nothing else is filled in when it is not.
    pub repo: bool,
    /// The checked-out branch; none when detached.
    pub branch: Option<String>,
    /// Local and remote branches other than the checked-out one.
    pub branches: Vec<String>,
    /// The latest commits on `HEAD`, newest first.
    pub commits: Vec<ReviewCommit>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReviewCommit {
    pub id: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    /// Unix seconds.
    pub time: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum ReviewError {
    #[error("session not found")]
    NoSession,
    #[error("a turn starts at a prompt you sent")]
    NotAPrompt,
    #[error("the workspace is not a git repository")]
    NotARepo,
    #[error("{0} is not a branch or commit here")]
    UnknownTarget(String),
    #[error("git: {0}")]
    Git(String),
    #[error("{0}")]
    Store(#[from] rusqlite::Error),
}

/// One side of a changed file: its bytes, or `None` for no file.
enum Side {
    Present(Vec<u8>),
    Absent,
    Unavailable,
}

impl Engine {
    /// Everything the turn started by `message_id` and its subagents changed, each file from before its first change to after its last.
    pub async fn turn_review(&self, session_id: &str, message_id: &str) -> Result<Review, ReviewError> {
        let session = self.store.session(session_id)?.ok_or(ReviewError::NoSession)?;
        let transcript = self.store.transcript(session_id)?;
        let start = transcript
            .iter()
            .position(|message| message.info.id == message_id && is_prompt(message))
            .ok_or(ReviewError::NotAPrompt)?;
        let end = transcript[start + 1..]
            .iter()
            .find(|message| is_prompt(message))
            .map(|message| message.info.id.clone());
        let (nets, _) = self
            .net_changes(&session, message_id, end.as_deref())
            .map_err(|_| ReviewError::NoSession)?;

        let mut review = Review::default();
        for net in nets {
            let before = self.recorded(&net.before).await;
            let after = self.recorded(&net.after).await;
            review.push(net.path, before, after);
        }

        Ok(review)
    }

    async fn recorded(&self, endpoint: &Endpoint) -> Side {
        let Some(blob) = &endpoint.blob else {
            return Side::Absent;
        };
        let Some(root) = self.root_of(&endpoint.owner) else {
            return Side::Unavailable;
        };

        match self.snapshots.blob(&root, blob).await {
            Ok(bytes) => Side::Present(bytes),
            Err(_) => Side::Unavailable,
        }
    }
}

/// Branches and recent commits to pick a review source from, or `repo: false` outside a repository.
pub async fn git_refs(root: &Path) -> ReviewRefs {
    let mut refs = ReviewRefs {
        repo: git_text(root, &["rev-parse", "--is-inside-work-tree"]).await.as_deref() == Some("true"),
        branch: None,
        branches: Vec::new(),
        commits: Vec::new(),
    };
    if !refs.repo {
        return refs;
    }

    refs.branch = git_text(root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).await;
    let listed = git_text(
        root,
        &[
            "for-each-ref",
            "--format=%(refname)%1f%(refname:short)",
            "refs/heads",
            "refs/remotes",
        ],
    )
    .await
    .unwrap_or_default();
    refs.branches = listed
        .lines()
        .filter_map(|line| line.split_once('\x1f'))
        .filter(|(full, short)| !full.ends_with("/HEAD") && Some(*short) != refs.branch.as_deref())
        .map(|(_, short)| short.to_string())
        .collect();

    let log = git_text(
        root,
        &["log", "-n", RECENT_COMMITS, "--format=%H%x1f%h%x1f%s%x1f%an%x1f%ct%x1e"],
    )
    .await
    .unwrap_or_default();
    refs.commits = log.split('\x1e').filter_map(parse_commit).collect();

    refs
}

fn parse_commit(record: &str) -> Option<ReviewCommit> {
    let mut fields = record.trim().split('\x1f');
    let (id, short, subject, author, time) = (
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
    );

    Some(ReviewCommit {
        id: id.into(),
        short: short.into(),
        subject: subject.into(),
        author: author.into(),
        time: time.parse().ok()?,
    })
}

/// The workspace's changes against `source`; paths are relative to `root`, and changes outside it are left out.
pub async fn git_review(root: &Path, source: GitSource, target: Option<&str>) -> Result<Review, ReviewError> {
    if git_text(root, &["rev-parse", "--is-inside-work-tree"]).await.as_deref() != Some("true") {
        return Err(ReviewError::NotARepo);
    }

    match source {
        GitSource::Uncommitted => {
            let head = commit_id(root, "HEAD").await.unwrap_or_else(|| EMPTY_TREE.into());
            working_tree(root, &head).await
        }
        GitSource::Branch => {
            let target = known_target(root, target).await?;
            let base = git_text(root, &["merge-base", "HEAD", &target])
                .await
                .ok_or_else(|| ReviewError::Git("the branches share no history".into()))?;
            working_tree(root, &base).await
        }
        GitSource::Commit => {
            let target = known_target(root, target).await?;
            let parent = commit_id(root, &format!("{target}^"))
                .await
                .unwrap_or_else(|| EMPTY_TREE.into());
            commit(root, &parent, &target).await
        }
    }
}

async fn known_target(root: &Path, target: Option<&str>) -> Result<String, ReviewError> {
    let name = target.unwrap_or_default();
    // A leading dash would be read as an option.
    if name.is_empty() || name.starts_with('-') {
        return Err(ReviewError::UnknownTarget(name.into()));
    }

    commit_id(root, name)
        .await
        .ok_or_else(|| ReviewError::UnknownTarget(name.into()))
}

async fn commit_id(root: &Path, name: &str) -> Option<String> {
    git_text(
        root,
        &["rev-parse", "--verify", "--quiet", &format!("{name}^{{commit}}")],
    )
    .await
}

/// `base` against the files on disk, untracked ones included.
async fn working_tree(root: &Path, base: &str) -> Result<Review, ReviewError> {
    let raw = git_bytes(root, &["diff-index", "--no-renames", "-z", "--relative", base]).await?;
    let mut review = Review::default();
    for change in parse_raw(&raw) {
        let before = stored(root, change.before.as_deref()).await;
        let after = on_disk(root, &change.path);
        review.push(change.path, before, after);
    }

    let untracked = git_bytes(root, &["ls-files", "--others", "--exclude-standard", "-z"]).await?;
    for path in String::from_utf8_lossy(&untracked)
        .split('\0')
        .filter(|path| !path.is_empty())
    {
        let after = on_disk(root, path);
        review.push(path.to_string(), Side::Absent, after);
    }

    Ok(review)
}

async fn commit(root: &Path, parent: &str, target: &str) -> Result<Review, ReviewError> {
    let raw = git_bytes(
        root,
        &["diff-tree", "-r", "--no-renames", "-z", "--relative", parent, target],
    )
    .await?;
    let mut review = Review::default();
    for change in parse_raw(&raw) {
        let before = stored(root, change.before.as_deref()).await;
        let after = stored(root, change.after.as_deref()).await;
        review.push(change.path, before, after);
    }

    Ok(review)
}

struct RawChange {
    path: String,
    before: Option<String>,
    after: Option<String>,
}

/// Git's NUL-delimited raw diff records; an all-zero blob id is no file, or for `diff-index` a file only on disk.
fn parse_raw(raw: &[u8]) -> Vec<RawChange> {
    let text = String::from_utf8_lossy(raw);
    let mut fields = text.split('\0');
    let mut changes = Vec::new();
    while let (Some(header), Some(path)) = (fields.next(), fields.next()) {
        let parts: Vec<&str> = header.trim_start_matches(':').split(' ').collect();
        let [_, _, before, after, _] = parts[..] else { continue };
        let blob = |id: &str| (!id.chars().all(|character| character == '0')).then(|| id.to_string());
        changes.push(RawChange {
            path: path.to_string(),
            before: blob(before),
            after: blob(after),
        });
    }

    changes
}

async fn stored(root: &Path, blob: Option<&str>) -> Side {
    let Some(blob) = blob else { return Side::Absent };

    match git_bytes(root, &["cat-file", "blob", blob]).await {
        Ok(bytes) => Side::Present(bytes),
        Err(_) => Side::Unavailable,
    }
}

fn on_disk(root: &Path, path: &str) -> Side {
    match std::fs::read(root.join(path)) {
        Ok(bytes) => Side::Present(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Side::Absent,
        Err(_) => Side::Unavailable,
    }
}

async fn git_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>, ReviewError> {
    plain_git(root, args, None)
        .await
        .ok_or_else(|| ReviewError::Git(format!("{} failed", args[0])))
}

async fn git_text(root: &Path, args: &[&str]) -> Option<String> {
    let bytes = plain_git(root, args, None).await?;
    let text = String::from_utf8_lossy(&bytes).trim().to_string();

    (!text.is_empty()).then_some(text)
}

impl Review {
    /// Adds a file unless both sides hold the same bytes, as a stat-only change does.
    fn push(&mut self, path: String, before: Side, after: Side) {
        if let (Side::Present(left), Side::Present(right)) = (&before, &after)
            && left == right
        {
            return;
        }
        if self.files.len() == MAX_FILES {
            self.omitted += 1;
            return;
        }

        self.files.push(review_file(path, &before, &after));
    }
}

fn review_file(path: String, before: &Side, after: &Side) -> ReviewFile {
    let status = match (before, after) {
        (Side::Absent, _) => FileStatus::Added,
        (_, Side::Absent) => FileStatus::Deleted,
        _ => FileStatus::Modified,
    };
    let mut file = ReviewFile {
        path,
        status,
        additions: 0,
        deletions: 0,
        preview: Preview::Lines,
        lines: Vec::new(),
    };

    let (Some(old), Some(new)) = (text(before), text(after)) else {
        file.preview = preview_without_lines(before, after);
        return file;
    };
    let diff = TextDiff::configure().timeout(DIFF_TIMEOUT).diff_lines(old, new);
    for change in diff.iter_all_changes() {
        let mark = match change.tag() {
            ChangeTag::Equal => ' ',
            ChangeTag::Insert => {
                file.additions += 1;
                '+'
            }
            ChangeTag::Delete => {
                file.deletions += 1;
                '-'
            }
        };
        file.lines
            .push(format!("{mark}{}", change.value().trim_end_matches(['\n', '\r'])));
    }

    file
}

/// A side's text, empty for no file; `None` when it cannot be shown as lines.
fn text(side: &Side) -> Option<&str> {
    match side {
        Side::Absent => Some(""),
        Side::Present(bytes) if bytes.len() <= MAX_DIFF_BYTES && !bytes.contains(&0) => std::str::from_utf8(bytes).ok(),
        _ => None,
    }
}

fn preview_without_lines(before: &Side, after: &Side) -> Preview {
    let sides = [before, after];
    if sides.iter().any(|side| matches!(side, Side::Unavailable)) {
        return Preview::Unavailable;
    }
    let large = sides
        .iter()
        .any(|side| matches!(side, Side::Present(bytes) if bytes.len() > MAX_DIFF_BYTES));

    if large { Preview::Large } else { Preview::Binary }
}

#[cfg(test)]
mod tests;
