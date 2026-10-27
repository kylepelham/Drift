//! What a turn, the working tree, a branch or a commit changed, as whole-file line diffs for the review panel.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
pub(crate) const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

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
    /// The file on this computer, to open it; none when where it lives is unknown.
    pub location: Option<String>,
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
    /// Shown as it is, for a review of files rather than a change.
    Unchanged,
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
    /// The files `target` names, one workspace-relative path a line, as they are on disk; no repository needed.
    Files,
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
    #[error("{0} is not a path inside the workspace")]
    OutsidePath(String),
    #[error("git: {0}")]
    Git(String),
    #[error("{0}")]
    Store(#[from] rusqlite::Error),
}

/// One side of a changed file: its bytes, or `None` for no file.
enum Side {
    Present(Vec<u8>),
    /// There, but past [`MAX_DIFF_BYTES`], so it was never read.
    Large,
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
        let (nets, unrecorded) = self
            .net_changes(&session, message_id, end.as_deref())
            .map_err(|_| ReviewError::NoSession)?;

        let mut review = Review::default();
        for net in nets {
            if review.full() {
                review.omitted += 1;
                continue;
            }
            let before = self.recorded(&net.before).await;
            let after = self.recorded(&net.after).await;
            review.push(net.path, net.file, before, after);
        }

        // A write that left no undo record still changed the file; it is listed without lines rather than dropped.
        for path in unrecorded {
            let key = path.replace('\\', "/").to_lowercase();
            if !review
                .files
                .iter()
                .any(|file| file.path.replace('\\', "/").to_lowercase() == key)
            {
                review.push(path, None, Side::Unavailable, Side::Unavailable);
            }
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
    let repo = matches!(source, GitSource::Files)
        || git_text(root, &["rev-parse", "--is-inside-work-tree"]).await.as_deref() == Some("true");

    match source {
        _ if !repo => Err(ReviewError::NotARepo),
        GitSource::Files => files(root, target.unwrap_or_default()).await,
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

/// Each named file as it is on disk, every line unchanged; a missing one is left out.
async fn files(root: &Path, listed: &str) -> Result<Review, ReviewError> {
    let paths: Vec<&str> = listed.lines().map(str::trim).filter(|path| !path.is_empty()).collect();
    if let Some(outside) = paths.iter().find(|path| !inside(path)) {
        return Err(ReviewError::OutsidePath((*outside).into()));
    }

    let mut review = Review::default();
    for path in paths {
        if review.full() {
            review.omitted += usize::from(tokio::fs::try_exists(root.join(path)).await.unwrap_or(false));
            continue;
        }
        let side = on_disk(root, path).await;
        if matches!(side, Side::Absent) {
            continue;
        }
        review.files.push(ReviewFile {
            location: openable(root, path).await.as_deref().map(location),
            ..as_it_is(path.into(), &side)
        });
    }

    Ok(review)
}

/// A relative path naming something, with no `..`, root or drive in it.
fn inside(path: &str) -> bool {
    use std::path::Component;

    let components = || Path::new(path).components();
    components().all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
        && components().any(|component| matches!(component, Component::Normal(_)))
}

fn as_it_is(path: String, side: &Side) -> ReviewFile {
    let mut file = ReviewFile {
        path,
        location: None,
        status: FileStatus::Unchanged,
        additions: 0,
        deletions: 0,
        preview: Preview::Lines,
        lines: Vec::new(),
    };
    match text(side) {
        Some(content) => file.lines = content.lines().map(|line| format!(" {line}")).collect(),
        None => file.preview = preview_without_lines(side, side),
    }

    file
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
    push_changes(root, &mut review, parse_raw(&raw), Source::Disk).await;

    let untracked = git_bytes(root, &["ls-files", "--others", "--exclude-standard", "-z"]).await?;
    for path in String::from_utf8_lossy(&untracked)
        .split('\0')
        .filter(|path| !path.is_empty())
    {
        if review.full() {
            review.omitted += 1;
            continue;
        }
        let after = on_disk(root, path).await;
        review.push(path.to_string(), openable(root, path).await, Side::Absent, after);
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
    push_changes(root, &mut review, parse_raw(&raw), Source::Blobs).await;

    Ok(review)
}

/// Where a change's new side is read from.
#[derive(Clone, Copy, PartialEq)]
enum Source {
    Disk,
    Blobs,
}

/// Adds changes a batch of blobs at a time, so changes that turn out to be equal never leave later files unread.
async fn push_changes(root: &Path, review: &mut Review, changes: Vec<RawChange>, source: Source) {
    // Only the mode changed: the same blob on both sides.
    let changes: Vec<_> = changes
        .into_iter()
        .filter(|change| change.before.is_none() || change.before != change.after)
        .collect();
    for batch in changes.chunks(MAX_FILES) {
        if review.full() {
            review.omitted += batch.len();
            continue;
        }
        let ids = batch
            .iter()
            .flat_map(|change| {
                [
                    change.before.as_deref(),
                    change.after.as_deref().filter(|_| source == Source::Blobs),
                ]
            })
            .flatten()
            .collect();
        let blobs = stored_all(root, ids).await;
        for change in batch {
            if review.full() {
                review.omitted += 1;
                continue;
            }
            let before = stored(&blobs, change.before.as_deref());
            let after = match source {
                Source::Disk => on_disk(root, &change.path).await,
                Source::Blobs => stored(&blobs, change.after.as_deref()),
            };
            let opens = openable(root, &change.path).await;
            review.push(change.path.clone(), opens, before, after);
        }
    }
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

/// Blob contents by id, and the ids too large to have been read.
#[derive(Default)]
struct Blobs {
    read: HashMap<String, Vec<u8>>,
    large: std::collections::HashSet<String>,
}

fn stored(blobs: &Blobs, blob: Option<&str>) -> Side {
    let Some(blob) = blob else { return Side::Absent };
    if blobs.large.contains(blob) {
        return Side::Large;
    }

    blobs
        .read
        .get(blob)
        .map_or(Side::Unavailable, |bytes| Side::Present(bytes.clone()))
}

/// Every blob's content from one `git cat-file --batch`, by id: a git process per file costs seconds on Windows. Their
/// sizes are asked first, so one past the limit is never read.
async fn stored_all(root: &Path, ids: Vec<&str>) -> Blobs {
    if ids.is_empty() {
        return Blobs::default();
    }
    let listed = ids.join("\n") + "\n";
    let sizes = plain_git(root, &["cat-file", "--batch-check"], Some(listed.as_bytes()))
        .await
        .map(|output| parse_sizes(&output))
        .unwrap_or_default();
    let (small, large): (Vec<&str>, Vec<&str>) = ids
        .into_iter()
        .filter(|id| sizes.contains_key(*id))
        .partition(|id| sizes[*id] <= MAX_DIFF_BYTES);
    let read = if small.is_empty() {
        HashMap::new()
    } else {
        let wanted = small.join("\n") + "\n";
        plain_git(root, &["cat-file", "--batch"], Some(wanted.as_bytes()))
            .await
            .map(|output| parse_batch(&output))
            .unwrap_or_default()
    };

    Blobs {
        read,
        large: large.into_iter().map(str::to_owned).collect(),
    }
}

/// `cat-file --batch-check` output: per object `<id> <type> <size>`, or `<id> missing`.
fn parse_sizes(output: &[u8]) -> HashMap<String, usize> {
    String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(' ').collect();
            let [id, _, size] = fields[..] else { return None };
            Some((id.to_string(), size.parse().ok()?))
        })
        .collect()
}

/// `cat-file --batch` output: per object `<id> <type> <size>`, its bytes and a newline, or `<id> missing`.
fn parse_batch(output: &[u8]) -> HashMap<String, Vec<u8>> {
    let mut found = HashMap::new();
    let mut rest = output;
    while let Some(end) = rest.iter().position(|byte| *byte == b'\n') {
        let header = String::from_utf8_lossy(&rest[..end]).into_owned();
        rest = &rest[end + 1..];
        let fields: Vec<&str> = header.split(' ').collect();
        let [id, _, size] = fields[..] else { continue };
        let Some(size) = size.parse::<usize>().ok().filter(|size| *size <= rest.len()) else {
            break;
        };
        found.insert(id.to_string(), rest[..size].to_vec());
        rest = &rest[(size + 1).min(rest.len())..];
    }

    found
}

/// A file in the working tree, read no further than one byte past the limit, so a large one is never held whole.
async fn on_disk(root: &Path, path: &str) -> Side {
    use tokio::io::AsyncReadExt;

    let missing = |error: &std::io::Error| error.kind() == std::io::ErrorKind::NotFound;
    let mut bytes = Vec::new();
    let read = match contained(root, path).await {
        Ok(path) => match tokio::fs::File::open(path).await {
            Ok(file) => file.take(MAX_DIFF_BYTES as u64 + 1).read_to_end(&mut bytes).await,
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    };
    match read {
        Ok(_) if bytes.len() > MAX_DIFF_BYTES => Side::Large,
        Ok(_) => Side::Present(bytes),
        Err(error) if missing(&error) => Side::Absent,
        Err(_) => Side::Unavailable,
    }
}

/// `path` under `root` with its links followed; a link leading out of the workspace is refused.
async fn contained(root: &Path, path: &str) -> std::io::Result<PathBuf> {
    let root = tokio::fs::canonicalize(root).await?;
    let path = tokio::fs::canonicalize(root.join(path)).await?;
    if path.starts_with(&root) {
        Ok(path)
    } else {
        Err(std::io::ErrorKind::PermissionDenied.into())
    }
}

/// Where the review panel may open `path`: under `root` with its links resolved, so retargeting one later moves nothing.
async fn openable(root: &Path, path: &str) -> Option<PathBuf> {
    let real_root = tokio::fs::canonicalize(root).await.ok()?;
    let real = contained(root, path).await.ok()?;

    real.strip_prefix(&real_root).ok().map(|inside| root.join(inside))
}

async fn git_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>, ReviewError> {
    plain_git(root, args, None)
        .await
        .ok_or_else(|| ReviewError::Git(format!("{} failed", args[0])))
}

/// Git's trimmed output; `None` when it fails or prints nothing.
pub(crate) async fn git_text(root: &Path, args: &[&str]) -> Option<String> {
    let bytes = plain_git(root, args, None).await?;
    let text = String::from_utf8_lossy(&bytes).trim().to_string();

    (!text.is_empty()).then_some(text)
}

impl Review {
    /// Adds a file unless both sides hold the same bytes, as a stat-only change does.
    fn full(&self) -> bool {
        self.files.len() == MAX_FILES
    }

    fn push(&mut self, path: String, location: Option<PathBuf>, before: Side, after: Side) {
        if let (Side::Present(left), Side::Present(right)) = (&before, &after)
            && left == right
        {
            return;
        }
        if self.files.len() == MAX_FILES {
            self.omitted += 1;
            return;
        }

        self.files.push(ReviewFile {
            location: location.as_deref().map(self::location),
            ..review_file(path, &before, &after)
        });
    }
}

/// Rebuilt from its components so a `/` from git becomes the platform's separator.
fn location(file: &Path) -> String {
    file.components().collect::<PathBuf>().to_string_lossy().into_owned()
}

fn review_file(path: String, before: &Side, after: &Side) -> ReviewFile {
    let status = match (before, after) {
        (Side::Absent, _) => FileStatus::Added,
        (_, Side::Absent) => FileStatus::Deleted,
        _ => FileStatus::Modified,
    };
    let mut file = ReviewFile {
        path,
        location: None,
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
    let large = sides.iter().any(|side| match side {
        Side::Large => true,
        Side::Present(bytes) => bytes.len() > MAX_DIFF_BYTES,
        _ => false,
    });

    if large { Preview::Large } else { Preview::Binary }
}

#[cfg(test)]
mod tests;
