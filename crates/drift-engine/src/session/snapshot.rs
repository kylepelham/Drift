//! File history in a shadow git dir, kept out of the workspace: what each writing call changed, as
//! blobs, so an undo can put back exactly those files and nothing else.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::process::Command;

/// One shadow repository per workspace, kept out of the workspace so it never shows up in git status.
pub struct Snapshots {
    root: PathBuf,
    /// Index operations serialise per workspace: concurrent sessions and subagents share one index.
    locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    /// Each bound workspace directory's shadow repo, named for its owner rather than its path.
    owners: Mutex<HashMap<PathBuf, PathBuf>>,
    /// Workspaces found to hold more than `max_tree_files`; not walked again while the engine runs.
    too_many: Mutex<HashSet<PathBuf>>,
    max_tree_files: usize,
    /// The git repository each workspace is the top of, if any, looked up once per run.
    sources: Mutex<HashMap<PathBuf, Option<Source>>>,
}

/// A repository a workspace is the top of; a capture there starts from its index, so one of any size is captured.
#[derive(Clone, Debug)]
struct Source {
    index: PathBuf,
    /// Its object stores, lent to tree commands only; undo's blobs always go to the shadow store, out of reach of its gc.
    objects: std::ffi::OsString,
}

/// One path a call changed: its content before and after as shadow blobs, `None` for no file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileChange {
    /// Relative to the workspace with `/` separators, or absolute for a file outside it.
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
    /// Seen changing while the call ran rather than written by it: anyone could have made it, so undo
    /// and redo leave it alone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub observed: bool,
}

/// A whole-tree capture and the files it could not hold because they were over the size limit, each
/// with its size and modification time so a later capture can tell whether it changed.
#[derive(Clone, Debug, PartialEq)]
pub struct Tree {
    pub id: String,
    pub oversized: Vec<(String, Stamp)>,
}

pub type Stamp = (u64, Option<std::time::SystemTime>);

/// What differs between two trees. A path over the size limit on either side is `unrecorded`, never
/// a creation or deletion: its absence from a tree says nothing about the file.
#[derive(Debug, Default, PartialEq)]
pub struct TreeChanges {
    pub changes: Vec<FileChange>,
    pub unrecorded: Vec<String>,
}

/// Files past this size are never copied into the store: a write to one is refused, since it could not
/// be undone, and whole-tree captures leave them out.
pub const MAX_RECORDED_BYTES: u64 = 10 * 1024 * 1024;
/// A whole-tree capture of more files than this (a drive, a home folder) is not taken: the first would run for minutes.
/// A git repository's own top folder has no limit, since its capture starts from the repository's index.
pub const MAX_TREE_FILES: usize = 50_000;
/// The shadow repo stores bytes exactly as they are on disk, whatever the workspace's
/// `.gitattributes` say: no line ending conversion, filters, keyword expansion or re-encoding. This
/// file outranks every in-tree attributes file.
const RAW_ATTRIBUTES: &str = "* -text -eol -filter -ident -working-tree-encoding\n";
/// Unreferenced objects younger than this survive a prune: a capture in flight has not been saved yet.
const PRUNE_GRACE: &str = "2.hours.ago";
const KEEP_REF: &str = "refs/drift/keep";

#[derive(Debug, PartialEq)]
pub enum Error {
    NoGit,
    TooLarge(String),
    TooManyFiles,
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoGit => write!(f, "git is not installed"),
            Self::TooLarge(path) => write!(
                f,
                "{path} is larger than {} MB, too large to keep for undo",
                MAX_RECORDED_BYTES / 1024 / 1024
            ),
            Self::TooManyFiles => write!(
                f,
                "the workspace holds more than {MAX_TREE_FILES} files, too many to record"
            ),
            Self::Failed(message) => write!(f, "git: {message}"),
        }
    }
}

impl Snapshots {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("snapshots"),
            locks: Mutex::default(),
            owners: Mutex::default(),
            too_many: Mutex::default(),
            max_tree_files: MAX_TREE_FILES,
            sources: Mutex::default(),
        }
    }

    pub(super) fn lock_for(&self, workspace: &Path) -> Arc<tokio::sync::Mutex<()>> {
        self.locks
            .lock()
            .unwrap()
            .entry(self.git_dir(workspace))
            .or_default()
            .clone()
    }

    /// Ties a workspace directory to its owner (the workspace id), whose history does not depend on
    /// where the directory is now: a session moved elsewhere, or a workspace pointed at a new path,
    /// still finds it. A repo kept under the directory's old path-derived name is taken over once.
    pub fn bind(&self, owner: &str, root: &Path) {
        let owned = self.owned_dir(owner);
        let legacy = self.path_dir(root);
        if !owned.exists() && legacy.join("HEAD").exists() {
            let _ = std::fs::rename(&legacy, &owned);
        }
        self.owners.lock().unwrap().insert(root.to_path_buf(), owned);
    }

    fn owned_dir(&self, owner: &str) -> PathBuf {
        self.root.join(format!(
            "ws-{}",
            owner.replace(|c: char| !c.is_ascii_alphanumeric() && c != '_', "-")
        ))
    }

    /// Deletes a workspace's whole history, once nothing of it is kept: its conversations are gone.
    pub fn forget(&self, owner: &str) {
        let owned = self.owned_dir(owner);
        self.owners.lock().unwrap().retain(|_, dir| *dir != owned);
        let _ = std::fs::remove_dir_all(owned);
    }

    fn git_dir(&self, workspace: &Path) -> PathBuf {
        self.owners
            .lock()
            .unwrap()
            .get(workspace)
            .cloned()
            .unwrap_or_else(|| self.path_dir(workspace))
    }

    /// Where an unbound directory's history lives, derived from its path.
    fn path_dir(&self, workspace: &Path) -> PathBuf {
        let key = workspace.to_string_lossy().to_lowercase().replace('\\', "/");
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in key.bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        self.root.join(format!("{hash:016x}"))
    }

    async fn git(&self, workspace: &Path, args: &[&str]) -> Result<String, Error> {
        let bytes = self.git_bytes(workspace, args).await?;
        Ok(String::from_utf8_lossy(&bytes).trim().to_string())
    }

    async fn git_bytes(&self, workspace: &Path, args: &[&str]) -> Result<Vec<u8>, Error> {
        self.run(workspace, args, None).await
    }

    async fn run(&self, workspace: &Path, args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>, Error> {
        self.run_with(workspace, args, input, None).await
    }

    /// `run`, with a repository's objects readable for a tree command.
    async fn run_with(
        &self,
        workspace: &Path,
        args: &[&str],
        input: Option<&[u8]>,
        source: Option<&Source>,
    ) -> Result<Vec<u8>, Error> {
        use tokio::io::AsyncWriteExt;
        let mut command = self.command(workspace, args, input.is_some());
        if let Some(source) = source {
            command.env("GIT_ALTERNATE_OBJECT_DIRECTORIES", &source.objects);
        }
        let mut child = command.spawn().map_err(|_| Error::NoGit)?;
        if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
            stdin.write_all(input).await.map_err(|e| Error::Failed(e.to_string()))?;
        }
        let output = child
            .wait_with_output()
            .await
            .map_err(|e| Error::Failed(e.to_string()))?;
        if !output.status.success() {
            return Err(Error::Failed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(output.stdout)
    }

    fn command(&self, workspace: &Path, args: &[&str], piped: bool) -> Command {
        let mut command = Command::new("git");
        // From the workspace root: pathspecs such as `.` resolve against git's working directory, not `--work-tree`.
        if workspace.is_dir() {
            command.current_dir(workspace);
        }
        command
            .arg("--git-dir")
            .arg(self.git_dir(workspace))
            .arg("--work-tree")
            .arg(workspace)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(if piped { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        command
    }

    /// Creates the shadow repository once; concurrent first captures wait for the one that creates it.
    async fn ensure(&self, workspace: &Path) -> Result<(), Error> {
        let git_dir = self.git_dir(workspace);
        if git_dir.join("HEAD").exists() {
            return Ok(());
        }
        let lock = self.lock_for(workspace);
        let _held = lock.lock().await;
        if git_dir.join("HEAD").exists() {
            return Ok(());
        }
        tokio::fs::create_dir_all(&git_dir)
            .await
            .map_err(|e| Error::Failed(e.to_string()))?;
        self.git(workspace, &["init", "-q"]).await?;
        self.git(workspace, &["config", "core.autocrlf", "false"]).await?;
        self.git(workspace, &["config", "user.email", "drift@localhost"])
            .await?;
        self.git(workspace, &["config", "user.name", "Drift"]).await?;
        Ok(())
    }

    /// Records the whole tree as it is now (the workspace's ignore rules apply). Serialised per
    /// workspace: concurrent sessions and subagents share this index.
    pub async fn take(&self, workspace: &Path) -> Result<Tree, Error> {
        let source = self.source(workspace).await;
        if source.is_none() && self.too_many.lock().unwrap().contains(workspace) {
            return Err(Error::TooManyFiles);
        }
        self.ensure(workspace).await?;
        let lock = self.lock_for(workspace);
        let _held = lock.lock().await;
        // Every index write in this process holds the lock, so a lock file now is a stopped capture's.
        let _ = tokio::fs::remove_file(self.git_dir(workspace).join("index.lock")).await;
        self.store_raw_bytes(workspace).await?;
        let oversized = match &source {
            Some(source) => {
                self.seed(workspace, source).await;
                self.leave_out_large_changes(workspace, source).await?
            }
            None => self.leave_out_large_files(workspace).await?,
        };
        // A file git cannot index (an unusual name, a locked file) must not stop every other file
        // being recorded; `--ignore-errors` still exits non-zero, so only the tree write decides.
        let _ = self
            .run_with(
                workspace,
                &["add", "-A", "--ignore-errors", "--", "."],
                None,
                source.as_ref(),
            )
            .await;
        // A seeded tree may name blobs only the repository holds; its ids are compared, never read.
        let write: &[&str] = if source.is_some() {
            &["write-tree", "--missing-ok"]
        } else {
            &["write-tree"]
        };
        let id = String::from_utf8_lossy(&self.run_with(workspace, write, None, source.as_ref()).await?)
            .trim()
            .to_string();
        Ok(Tree { id, oversized })
    }

    /// The repository `workspace` is the top of, looked up once; none for a plain folder or a folder inside a repository.
    async fn source(&self, workspace: &Path) -> Option<Source> {
        if let Some(known) = self.sources.lock().unwrap().get(workspace) {
            return known.clone();
        }
        let found = find_source(workspace).await;
        self.sources
            .lock()
            .unwrap()
            .insert(workspace.to_path_buf(), found.clone());
        found
    }

    /// Starts the shadow index from the repository's once; an index this git cannot read is dropped and rebuilt.
    async fn seed(&self, workspace: &Path, source: &Source) {
        let git_dir = self.git_dir(workspace);
        let marker = git_dir.join("drift-seeded");
        if marker.exists() {
            return;
        }
        let index = git_dir.join("index");
        if tokio::fs::copy(&source.index, &index).await.is_err() {
            return;
        }
        if self
            .run_with(
                workspace,
                &["ls-files", "-z", "--", ":(literal)drift-seed-probe"],
                None,
                Some(source),
            )
            .await
            .is_err()
        {
            let _ = tokio::fs::remove_file(&index).await;
        }
        let _ = tokio::fs::write(marker, "").await;
    }

    /// `leave_out_large_files` for a repository: only what git names as changed or untracked is sized, with no walk.
    async fn leave_out_large_changes(&self, workspace: &Path, source: &Source) -> Result<Vec<(String, Stamp)>, Error> {
        let modified = self
            .run_with(workspace, &["diff-files", "--name-only", "-z"], None, Some(source))
            .await?;
        let untracked = self
            .run_with(
                workspace,
                &["ls-files", "--others", "--exclude-standard", "-z"],
                None,
                Some(source),
            )
            .await?;
        let excluded = tokio::fs::read_to_string(self.git_dir(workspace).join("info").join("exclude"))
            .await
            .unwrap_or_default();
        let listed = [modified, untracked].map(|raw| String::from_utf8_lossy(&raw).into_owned());
        let mut candidates: Vec<String> = listed
            .iter()
            .flat_map(|text| text.split('\0'))
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect();
        candidates.extend(
            excluded
                .lines()
                .filter_map(|line| line.strip_prefix('/'))
                .map(unescape_pattern),
        );
        candidates.sort();
        candidates.dedup();
        let root = workspace.to_path_buf();
        let sized = move || -> Vec<(String, Stamp)> {
            let large = |path: String| {
                let meta = std::fs::metadata(root.join(&path))
                    .ok()
                    .filter(|m| m.is_file() && m.len() > MAX_RECORDED_BYTES)?;
                Some((path, (meta.len(), meta.modified().ok())))
            };
            candidates.into_iter().filter_map(large).collect()
        };
        let large = tokio::task::spawn_blocking(sized)
            .await
            .map_err(|e| Error::Failed(e.to_string()))?;
        self.exclude(workspace, &large).await?;
        Ok(large)
    }

    /// Makes sure the shadow repo stores bytes exactly, repos made before this rule included. When the
    /// rule is new, the index goes: its cached entries may hold converted blobs for unchanged files.
    async fn store_raw_bytes(&self, workspace: &Path) -> Result<(), Error> {
        let info = self.git_dir(workspace).join("info");
        let path = info.join("attributes");
        if tokio::fs::read_to_string(&path)
            .await
            .is_ok_and(|current| current == RAW_ATTRIBUTES)
        {
            return Ok(());
        }
        tokio::fs::create_dir_all(&info)
            .await
            .map_err(|e| Error::Failed(e.to_string()))?;
        tokio::fs::write(&path, RAW_ATTRIBUTES)
            .await
            .map_err(|e| Error::Failed(e.to_string()))?;
        match tokio::fs::remove_file(self.git_dir(workspace).join("index")).await {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(Error::Failed(error.to_string())),
            _ => Ok(()),
        }
    }

    /// Keeps files over the size limit out of the capture: the shadow repo's own exclude file stops
    /// new ones being added, and any already indexed from when they were small are dropped from the
    /// index. The workspace's `.gitignore` is never touched. Returns what was left out.
    async fn leave_out_large_files(&self, workspace: &Path) -> Result<Vec<(String, Stamp)>, Error> {
        let (root, limit) = (workspace.to_path_buf(), self.max_tree_files);
        let walked = tokio::task::spawn_blocking(move || large_files(&root, limit))
            .await
            .map_err(|e| Error::Failed(e.to_string()))?;
        let Some(large) = walked else {
            self.too_many.lock().unwrap().insert(workspace.to_path_buf());
            return Err(Error::TooManyFiles);
        };
        self.exclude(workspace, &large).await?;
        Ok(large)
    }

    /// Keeps `large` out of the shadow index from now on, and drops any already in it.
    async fn exclude(&self, workspace: &Path, large: &[(String, Stamp)]) -> Result<(), Error> {
        let info = self.git_dir(workspace).join("info");
        tokio::fs::create_dir_all(&info)
            .await
            .map_err(|e| Error::Failed(e.to_string()))?;
        let lines: String = large
            .iter()
            .map(|(path, _)| format!("/{}\n", escape_pattern(path)))
            .collect();
        tokio::fs::write(info.join("exclude"), lines)
            .await
            .map_err(|e| Error::Failed(e.to_string()))?;
        if !large.is_empty() {
            let paths: String = large.iter().map(|(path, _)| format!("{path}\0")).collect();
            self.run(
                workspace,
                &["update-index", "--force-remove", "-z", "--stdin"],
                Some(paths.as_bytes()),
            )
            .await?;
        }
        Ok(())
    }

    /// Drops every stored object no call's recorded change refers to (and that is older than the
    /// grace period, so a capture in flight keeps its blobs). `keep` pins the rest under a private ref.
    pub async fn prune(&self, workspace: &Path, keep: &[String]) -> Result<(), Error> {
        self.prune_older_than(workspace, keep, PRUNE_GRACE).await
    }

    async fn prune_older_than(&self, workspace: &Path, keep: &[String], expire: &str) -> Result<(), Error> {
        if !self.git_dir(workspace).join("HEAD").exists() {
            return Ok(());
        }
        let lock = self.lock_for(workspace);
        let _held = lock.lock().await;
        if keep.is_empty() {
            let _ = self.git(workspace, &["update-ref", "-d", KEEP_REF]).await;
        } else {
            let listing: String = keep
                .iter()
                .enumerate()
                .map(|(i, blob)| format!("100644 blob {blob}\t{i}\n"))
                .collect();
            let tree = self
                .run(workspace, &["mktree", "--missing"], Some(listing.as_bytes()))
                .await?;
            let tree = String::from_utf8_lossy(&tree).trim().to_string();
            let commit = self
                .git(
                    workspace,
                    &["commit-tree", &tree, "-m", "blobs referenced by recorded changes"],
                )
                .await?;
            self.git(workspace, &["update-ref", KEEP_REF, &commit]).await?;
        }
        self.git(workspace, &["prune", &format!("--expire={expire}")])
            .await
            .map(|_| ())
    }

    /// Stores `path`'s content now and returns its blob, or `None` when there is no such file.
    pub async fn record(&self, workspace: &Path, path: &str) -> Result<Option<String>, Error> {
        self.hash(workspace, path, true).await
    }

    /// Stores `bytes` as a blob of the workspace's history, for content that is not on disk now
    /// (an imported edit's earlier version).
    pub async fn store_bytes(&self, workspace: &Path, bytes: &[u8]) -> Result<String, Error> {
        self.ensure(workspace).await?;
        let blob = self
            .run(
                workspace,
                &["hash-object", "-w", "--no-filters", "--stdin"],
                Some(bytes),
            )
            .await?;
        Ok(String::from_utf8_lossy(&blob).trim().to_string())
    }

    /// `path`'s blob id now without storing it, to compare against a recorded one.
    pub async fn current(&self, workspace: &Path, path: &str) -> Result<Option<String>, Error> {
        self.hash(workspace, path, false).await
    }

    async fn hash(&self, workspace: &Path, path: &str, store: bool) -> Result<Option<String>, Error> {
        // Recording needs the store even for a file not there yet: its after state must be storable.
        if store {
            self.ensure(workspace).await?;
        }
        let file = workspace.join(path);
        let Ok(meta) = std::fs::metadata(&file).map(|m| (m.is_file(), m.len())) else {
            return Ok(None);
        };
        if !meta.0 {
            return Ok(None);
        }
        if store && meta.1 > MAX_RECORDED_BYTES {
            return Err(Error::TooLarge(path.to_string()));
        }
        let file = file.to_string_lossy().into_owned();
        let args: Vec<&str> = if store {
            vec!["hash-object", "-w", "--no-filters", "--", &file]
        } else {
            vec!["hash-object", "--no-filters", "--", &file]
        };
        self.git(workspace, &args).await.map(Some)
    }

    /// Makes `path` hold `blob` through the staged writer, or removes it for `None`.
    pub async fn put(
        &self,
        store: &crate::store::Store,
        workspace: &Path,
        path: &str,
        blob: Option<&str>,
    ) -> Result<(), Error> {
        let file = workspace.join(path);
        let Some(blob) = blob else {
            return match tokio::fs::remove_file(&file).await {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(Error::Failed(error.to_string())),
                _ => Ok(()),
            };
        };
        let content = self.git_bytes(workspace, &["cat-file", "blob", blob]).await?;
        crate::tool::stage::replace(store, &file, &content)
            .await
            .map_err(|e| Error::Failed(e.to_string()))
    }

    /// Every path that differs between two trees, with its blob on each side.
    pub async fn changes_between(&self, workspace: &Path, before: &Tree, after: &Tree) -> Result<TreeChanges, Error> {
        let source = self.source(workspace).await;
        let raw = self
            .run_with(
                workspace,
                &["diff-tree", "-r", "--no-renames", "-z", &before.id, &after.id],
                None,
                source.as_ref(),
            )
            .await?;
        let oversized = |path: &str| {
            before
                .oversized
                .iter()
                .chain(&after.oversized)
                .any(|(large, _)| large == path)
        };
        let (unrecordable, changes): (Vec<FileChange>, Vec<FileChange>) = parse_raw_diff(&raw)
            .into_iter()
            .partition(|change| oversized(&change.path));
        let changes = if source.is_some() {
            unconverted_changes(workspace, changes).await
        } else {
            changes
        };
        let mut unrecorded: Vec<String> = unrecordable.into_iter().map(|change| change.path).collect();
        let untouched = |entry: &(String, Stamp)| before.oversized.contains(entry) && after.oversized.contains(entry);
        unrecorded.extend(
            before
                .oversized
                .iter()
                .chain(&after.oversized)
                .filter(|entry| !untouched(entry))
                .map(|(path, _)| path.clone()),
        );
        unrecorded.sort();
        unrecorded.dedup();
        Ok(TreeChanges { changes, unrecorded })
    }
}

/// Workspace files over the limit, relative with `/`, found the way git would (ignore rules apply,
/// in a directory that is not a git repository too).
/// Files over the size limit, or `None` once the walk passes `limit` files.
fn large_files(root: &Path, limit: usize) -> Option<Vec<(String, Stamp)>> {
    let mut files = 0;
    let mut large = Vec::new();
    for entry in ignore::WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build()
        .flatten()
    {
        let Some(meta) = entry.metadata().ok().filter(std::fs::Metadata::is_file) else {
            continue;
        };
        files += 1;
        if files > limit {
            return None;
        }
        if meta.len() > MAX_RECORDED_BYTES {
            let Ok(path) = entry.path().strip_prefix(root) else {
                continue;
            };
            large.push((
                path.to_string_lossy().replace('\\', "/"),
                (meta.len(), meta.modified().ok()),
            ));
        }
    }
    Some(large)
}

/// The repository `workspace` is the top of, with its index and object stores; `None` otherwise.
async fn find_source(workspace: &Path) -> Option<Source> {
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
    let chained = std::fs::read_to_string(objects.join("info").join("alternates")).unwrap_or_default();
    let more = chained
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| objects.join(line));
    let stores: Vec<PathBuf> = std::iter::once(objects.clone())
        .chain(more)
        .filter(|dir| dir.is_dir())
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

/// Git in the workspace's own repository, never the shadow one; `None` when it fails.
async fn plain_git(workspace: &Path, args: &[&str], input: Option<&[u8]>) -> Option<Vec<u8>> {
    use tokio::io::AsyncWriteExt;
    let mut command = Command::new("git");
    let stdin = if input.is_some() { Stdio::piped() } else { Stdio::null() };
    command
        .current_dir(workspace)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().ok()?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(input).await.ok()?;
    }
    let output = child.wait_with_output().await.ok()?;
    output.status.success().then_some(output.stdout)
}

/// Drops changes whose file the repository still holds as the before blob through its filters: only the timestamp moved.
async fn unconverted_changes(workspace: &Path, changes: Vec<FileChange>) -> Vec<FileChange> {
    let modified: Vec<&FileChange> = changes
        .iter()
        .filter(|change| change.before.is_some() && change.after.is_some())
        .collect();
    if modified.is_empty() {
        return changes;
    }
    let paths: String = modified.iter().map(|change| format!("{}\n", change.path)).collect();
    let Some(hashed) = plain_git(workspace, &["hash-object", "--stdin-paths"], Some(paths.as_bytes())).await else {
        return changes;
    };
    let hashed = String::from_utf8_lossy(&hashed);
    let same: HashSet<String> = modified
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

/// The path an exclude pattern written by `escape_pattern` matches.
fn unescape_pattern(pattern: &str) -> String {
    let mut out = String::new();
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        out.push(if c == '\\' { chars.next().unwrap_or(c) } else { c });
    }
    out
}

/// An exclude pattern matching exactly this path.
fn escape_pattern(path: &str) -> String {
    path.chars()
        .flat_map(|c| {
            if matches!(c, '*' | '?' | '[' | '\\' | '!' | '#' | ' ') {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect()
}

/// `git diff-tree -z` raw records: `:<mode> <mode> <sha> <sha> <status>\0<path>\0`; all-zero shas mean no file.
fn parse_raw_diff(raw: &[u8]) -> Vec<FileChange> {
    let text = String::from_utf8_lossy(raw);
    let mut fields = text.split('\0');
    let mut changes = Vec::new();
    while let (Some(header), Some(path)) = (fields.next(), fields.next()) {
        let parts: Vec<&str> = header.trim_start_matches(':').split(' ').collect();
        let [_, _, before, after, _] = parts[..] else { continue };
        let blob = |sha: &str| (!sha.chars().all(|c| c == '0')).then(|| sha.to_string());
        changes.push(FileChange {
            path: path.to_string(),
            before: blob(before),
            after: blob(after),
            observed: false,
        });
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs() -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("drift-snap-{}", crate::random_hex(4)));
        let workspace = base.join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        (base, workspace)
    }

    #[tokio::test]
    async fn blobs_round_trip_and_tree_diffs_name_each_changed_path() {
        let (base, workspace) = dirs();
        let snapshots = Snapshots::new(&base.join("data"));
        std::fs::write(workspace.join("a.txt"), "one\n").unwrap();
        std::fs::write(workspace.join("gone.txt"), "g\n").unwrap();
        let one = snapshots.record(&workspace, "a.txt").await.unwrap().unwrap();
        assert_eq!(snapshots.record(&workspace, "missing.txt").await.unwrap(), None);
        let before = snapshots.take(&workspace).await.unwrap();
        std::fs::write(workspace.join("a.txt"), "two\n").unwrap();
        std::fs::remove_file(workspace.join("gone.txt")).unwrap();
        std::fs::write(workspace.join("new.txt"), "n\n").unwrap();
        let after = snapshots.take(&workspace).await.unwrap();
        let mut changes = snapshots
            .changes_between(&workspace, &before, &after)
            .await
            .unwrap()
            .changes;
        changes.sort_by(|a, b| a.path.cmp(&b.path));
        let summary: Vec<(&str, bool, bool)> = changes
            .iter()
            .map(|c| (c.path.as_str(), c.before.is_some(), c.after.is_some()))
            .collect();
        assert_eq!(
            summary,
            [
                ("a.txt", true, true),
                ("gone.txt", true, false),
                ("new.txt", false, true)
            ]
        );
        assert_eq!(changes[0].before.as_deref(), Some(one.as_str()));

        assert_ne!(snapshots.current(&workspace, "a.txt").await.unwrap(), Some(one.clone()));
        let store = crate::store::tests::store();
        snapshots.put(&store, &workspace, "a.txt", Some(&one)).await.unwrap();
        snapshots.put(&store, &workspace, "new.txt", None).await.unwrap();
        assert_eq!(std::fs::read_to_string(workspace.join("a.txt")).unwrap(), "one\n");
        assert!(!workspace.join("new.txt").exists());
        assert_eq!(snapshots.current(&workspace, "a.txt").await.unwrap(), Some(one));
        assert!(
            !workspace.join(".git").exists(),
            "shadow repo must not touch the workspace"
        );
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn git_runs_from_the_workspace_root_whatever_the_engines_own_directory() {
        let (base, workspace) = dirs();
        let snapshots = Snapshots::new(&base.join("data"));
        let command = snapshots.command(&workspace, &["add", "-A", "--", "."], false);
        assert_eq!(
            command.as_std().get_current_dir(),
            Some(workspace.as_path()),
            "`.` must mean the workspace, not where the app was started"
        );
        std::fs::remove_dir_all(base).ok();
    }

    /// Git in the test repository itself, as its user would run it.
    fn repo_git(workspace: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .current_dir(workspace)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[tokio::test]
    async fn a_repository_is_captured_from_its_own_index_whatever_its_size_and_conversions() {
        let (base, workspace) = dirs();
        let workspace = crate::tool::canonical(&workspace);
        repo_git(&workspace, &["init", "-q"]);
        for (key, value) in [
            ("core.autocrlf", "true"),
            ("user.email", "dev@example.com"),
            ("user.name", "Dev"),
        ] {
            repo_git(&workspace, &["config", key, value]);
        }
        std::fs::write(workspace.join(".gitattributes"), "* text=auto\n").unwrap();
        for name in ["a", "b", "c", "d", "e"] {
            std::fs::write(
                workspace.join(format!("{name}.txt")),
                format!("{name} one\r\n{name} two\r\n"),
            )
            .unwrap();
        }
        repo_git(&workspace, &["add", "-A"]);
        repo_git(&workspace, &["commit", "-qm", "start"]);
        // As in a repository used for a while: its index is newer than its files, so git trusts their timestamps.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        repo_git(&workspace, &["update-index", "--refresh"]);
        let mut snapshots = Snapshots::new(&base.join("data"));
        snapshots.max_tree_files = 3;
        let before = snapshots
            .take(&workspace)
            .await
            .expect("a repository has no file limit");
        let counted = snapshots.git(&workspace, &["count-objects"]).await.unwrap();
        assert!(
            counted.starts_with("0 objects"),
            "the repository's files were not copied in: {counted}"
        );

        std::fs::write(workspace.join("a.txt"), "a changed\r\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        // Same bytes, new timestamp: its seeded id is the converted content, so only the repository can tell.
        std::fs::write(workspace.join("b.txt"), "b one\r\nb two\r\n").unwrap();
        std::fs::write(workspace.join("new.txt"), "n\r\n").unwrap();
        let after = snapshots.take(&workspace).await.unwrap();
        let mut changed: Vec<String> = snapshots
            .changes_between(&workspace, &before, &after)
            .await
            .unwrap()
            .changes
            .into_iter()
            .map(|change| change.path)
            .collect();
        changed.sort();
        assert_eq!(changed, ["a.txt", "new.txt"]);

        let kept = snapshots.record(&workspace, "c.txt").await.unwrap().unwrap();
        assert!(
            stored(&snapshots, &workspace, &kept),
            "a blob kept for undo lives in the shadow store, not only in the repository"
        );
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn a_workspace_with_too_many_files_is_not_captured_and_not_walked_again() {
        let (base, workspace) = dirs();
        let mut snapshots = Snapshots::new(&base.join("data"));
        snapshots.max_tree_files = 3;
        for name in ["a", "b", "c", "d"] {
            std::fs::write(workspace.join(name), name).unwrap();
        }
        assert_eq!(snapshots.take(&workspace).await, Err(Error::TooManyFiles));
        assert!(
            snapshots
                .git(&workspace, &["count-objects"])
                .await
                .unwrap()
                .starts_with("0 objects"),
            "git never ran over the tree"
        );
        std::fs::remove_file(workspace.join("d")).unwrap();
        assert_eq!(
            snapshots.take(&workspace).await,
            Err(Error::TooManyFiles),
            "the verdict lasts while the engine runs"
        );
    }

    #[tokio::test]
    async fn a_capture_stopped_part_way_does_not_block_the_next() {
        let (base, workspace) = dirs();
        let snapshots = Snapshots::new(&base.join("data"));
        std::fs::write(workspace.join("a.txt"), "a\n").unwrap();
        let before = snapshots.take(&workspace).await.unwrap();
        std::fs::write(snapshots.git_dir(&workspace).join("index.lock"), "").unwrap();
        std::fs::write(workspace.join("a.txt"), "changed\n").unwrap();
        let after = snapshots.take(&workspace).await.unwrap();
        assert_ne!(
            before.id, after.id,
            "the change was captured despite the stopped capture's lock file"
        );
    }

    #[tokio::test]
    async fn large_files_are_never_copied_into_the_store() {
        let (base, workspace) = dirs();
        let snapshots = Snapshots::new(&base.join("data"));
        std::fs::write(workspace.join("small.txt"), "s\n").unwrap();
        let before = snapshots.take(&workspace).await.unwrap();
        std::fs::write(
            workspace.join("huge [1].bin"),
            vec![b'x'; MAX_RECORDED_BYTES as usize + 1],
        )
        .unwrap();
        std::fs::write(workspace.join("small.txt"), "changed\n").unwrap();
        assert!(matches!(
            snapshots.record(&workspace, "huge [1].bin").await,
            Err(Error::TooLarge(_))
        ));
        let after = snapshots.take(&workspace).await.unwrap();
        let diff = snapshots.changes_between(&workspace, &before, &after).await.unwrap();
        let changed: Vec<String> = diff.changes.into_iter().map(|c| c.path).collect();
        assert_eq!(changed, ["small.txt"], "the large file is left out of the tree");
        assert_eq!(diff.unrecorded, ["huge [1].bin"]);
        let again = snapshots.take(&workspace).await.unwrap();
        assert_eq!(
            snapshots.changes_between(&workspace, &after, &again).await.unwrap(),
            TreeChanges::default(),
            "an untouched large file is not news"
        );
        std::fs::remove_dir_all(base).ok();
    }

    fn stored(snapshots: &Snapshots, workspace: &Path, blob: &str) -> bool {
        let git_dir = snapshots.git_dir(workspace);
        std::process::Command::new("git")
            .arg("--git-dir")
            .arg(git_dir)
            .args(["cat-file", "-e", blob])
            .status()
            .unwrap()
            .success()
    }

    #[tokio::test]
    async fn a_tracked_file_that_grows_past_the_limit_leaves_the_store_without_reading_as_deleted() {
        let (base, workspace) = dirs();
        let snapshots = Snapshots::new(&base.join("data"));
        std::fs::write(workspace.join("grows.log"), "small\n").unwrap();
        let small = snapshots.take(&workspace).await.unwrap();
        let big = vec![b'y'; MAX_RECORDED_BYTES as usize + 1];
        std::fs::write(workspace.join("grows.log"), &big).unwrap();
        let large = snapshots.take(&workspace).await.unwrap();
        let grew = snapshots.changes_between(&workspace, &small, &large).await.unwrap();
        assert_eq!(
            grew,
            TreeChanges {
                changes: vec![],
                unrecorded: vec!["grows.log".into()]
            },
            "not a deletion"
        );
        let big_blob = snapshots.current(&workspace, "grows.log").await.unwrap().unwrap();
        assert!(
            !stored(&snapshots, &workspace, &big_blob),
            "the large content never enters the store"
        );

        std::fs::write(workspace.join("grows.log"), "small again\n").unwrap();
        let shrunk = snapshots.take(&workspace).await.unwrap();
        let back = snapshots.changes_between(&workspace, &large, &shrunk).await.unwrap();
        assert_eq!(
            back,
            TreeChanges {
                changes: vec![],
                unrecorded: vec!["grows.log".into()]
            },
            "not a creation"
        );
        std::fs::write(workspace.join("grows.log"), "edited\n").unwrap();
        let edited = snapshots.take(&workspace).await.unwrap();
        let recorded = snapshots.changes_between(&workspace, &shrunk, &edited).await.unwrap();
        assert_eq!(recorded.changes.len(), 1, "once small it is recorded again");
        std::fs::remove_dir_all(base).ok();
    }

    /// Attributes that would change bytes on the way into git, and a filter the shadow repo can see.
    fn hostile_attributes(snapshots: &Snapshots, workspace: &Path, attributes: &str) {
        std::fs::write(workspace.join(".gitattributes"), attributes).unwrap();
        let git_dir = snapshots.git_dir(workspace);
        let config = |key: &str, value: &str| {
            std::process::Command::new("git")
                .arg("--git-dir")
                .arg(&git_dir)
                .args(["config", key, value])
                .status()
                .unwrap()
        };
        config("filter.upper.clean", "tr a-z A-Z");
        config("filter.upper.smudge", "cat");
    }

    #[tokio::test]
    async fn trees_hold_the_bytes_on_disk_whatever_the_attributes_say() {
        let (base, workspace) = dirs();
        let snapshots = Snapshots::new(&base.join("data"));
        std::fs::write(workspace.join("seed"), "x").unwrap();
        snapshots.take(&workspace).await.unwrap();
        hostile_attributes(
            &snapshots,
            &workspace,
            "* text=auto\n*.txt eol=lf ident filter=upper working-tree-encoding=UTF-16\n",
        );
        std::fs::write(workspace.join("a.txt"), "one $Id$\r\ntwo\r\n").unwrap();
        std::fs::write(workspace.join("b.md"), "crlf\r\n").unwrap();
        let before = snapshots.take(&workspace).await.unwrap();
        std::fs::write(workspace.join("a.txt"), "three\r\n").unwrap();
        std::fs::write(workspace.join("b.md"), "changed\r\n").unwrap();
        let after = snapshots.take(&workspace).await.unwrap();
        for change in snapshots
            .changes_between(&workspace, &before, &after)
            .await
            .unwrap()
            .changes
        {
            assert_eq!(
                snapshots.current(&workspace, &change.path).await.unwrap(),
                change.after,
                "{} after is the file's exact bytes",
                change.path
            );
            snapshots
                .put(
                    &crate::store::tests::store(),
                    &workspace,
                    &change.path,
                    change.before.as_deref(),
                )
                .await
                .unwrap();
        }
        assert_eq!(std::fs::read(workspace.join("a.txt")).unwrap(), b"one $Id$\r\ntwo\r\n");
        assert_eq!(std::fs::read(workspace.join("b.md")).unwrap(), b"crlf\r\n");
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn a_shadow_repo_made_before_the_rule_is_brought_up_to_it() {
        let (base, workspace) = dirs();
        let snapshots = Snapshots::new(&base.join("data"));
        std::fs::write(workspace.join("seed"), "x").unwrap();
        snapshots.take(&workspace).await.unwrap();
        hostile_attributes(&snapshots, &workspace, "* text=auto\n*.txt filter=upper\n");
        std::fs::write(workspace.join("a.txt"), "lower\r\n").unwrap();
        std::fs::remove_file(snapshots.git_dir(&workspace).join("info/attributes")).unwrap();
        snapshots.git(&workspace, &["add", "-A"]).await.unwrap();
        let converted = snapshots
            .git(&workspace, &["ls-files", "-s", "--", "a.txt"])
            .await
            .unwrap();

        let tree = snapshots.take(&workspace).await.unwrap();
        let entry = snapshots
            .git(&workspace, &["ls-tree", &tree.id, "--", "a.txt"])
            .await
            .unwrap();
        let exact = snapshots.current(&workspace, "a.txt").await.unwrap().unwrap();
        assert!(
            entry.contains(&exact),
            "the stale converted entry is replaced: {converted} vs {entry}"
        );
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn a_prune_keeps_what_history_refers_to_and_drops_the_rest() {
        let (base, workspace) = dirs();
        let snapshots = Snapshots::new(&base.join("data"));
        std::fs::write(workspace.join("kept.txt"), "kept\n").unwrap();
        std::fs::write(workspace.join("dropped.txt"), "dropped\n").unwrap();
        let kept = snapshots.record(&workspace, "kept.txt").await.unwrap().unwrap();
        let dropped = snapshots.record(&workspace, "dropped.txt").await.unwrap().unwrap();
        snapshots
            .prune_older_than(&workspace, std::slice::from_ref(&kept), "now")
            .await
            .unwrap();
        assert!(snapshots.git(&workspace, &["cat-file", "-e", &kept]).await.is_ok());
        assert!(snapshots.git(&workspace, &["cat-file", "-e", &dropped]).await.is_err());
        snapshots.prune(&workspace, &[]).await.unwrap();
        assert!(
            snapshots.git(&workspace, &["cat-file", "-e", &kept]).await.is_ok(),
            "the grace period protects recent objects"
        );
        std::fs::remove_dir_all(base).ok();
    }

    /// `cargo test -p drift-engine --release -- --ignored capture_cost --nocapture`; set
    /// `DRIFT_MEASURE_WORKSPACE` to time a real tree instead of 5,000 generated files.
    #[tokio::test]
    #[ignore]
    async fn capture_cost() {
        let (base, generated) = dirs();
        for i in 0..5_000 {
            let dir = generated.join(format!("d{}", i % 50));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("f{i}.txt")), vec![b'a' + (i % 26) as u8; 1024]).unwrap();
        }
        let workspace = std::env::var("DRIFT_MEASURE_WORKSPACE")
            .map(PathBuf::from)
            .unwrap_or(generated.clone());
        let snapshots = Snapshots::new(&base.join("data"));
        let time = |label: &'static str, started: std::time::Instant| eprintln!("{label}: {:?}", started.elapsed());
        let started = std::time::Instant::now();
        snapshots.take(&workspace).await.unwrap();
        time("first capture", started);
        for round in 0..3 {
            let started = std::time::Instant::now();
            snapshots.take(&workspace).await.unwrap();
            time(
                ["unchanged capture 1", "unchanged capture 2", "unchanged capture 3"][round],
                started,
            );
        }
        let started = std::time::Instant::now();
        let root = workspace.clone();
        tokio::task::spawn_blocking(move || large_files(&root, MAX_TREE_FILES))
            .await
            .unwrap();
        time("size walk alone", started);
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn a_bound_workspace_keeps_its_history_under_its_owner_wherever_its_directory_is() {
        let (base, workspace) = dirs();
        let snapshots = Snapshots::new(&base.join("data"));
        std::fs::write(workspace.join("a.txt"), "one\n").unwrap();
        let old = snapshots.record(&workspace, "a.txt").await.unwrap().unwrap();
        let legacy = snapshots.path_dir(&workspace);
        assert!(legacy.join("HEAD").exists());

        snapshots.bind("ws_1", &workspace);
        assert!(!legacy.exists(), "the path-named repo was taken over");
        std::fs::write(workspace.join("a.txt"), "two\n").unwrap();
        let store = crate::store::tests::store();
        snapshots.put(&store, &workspace, "a.txt", Some(&old)).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
            "one\n",
            "history from before the binding is kept"
        );

        let moved = base.join("moved");
        std::fs::rename(&workspace, &moved).unwrap();
        snapshots.bind("ws_1", &moved);
        std::fs::write(moved.join("a.txt"), "three\n").unwrap();
        snapshots.put(&store, &moved, "a.txt", Some(&old)).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(moved.join("a.txt")).unwrap(),
            "one\n",
            "a workspace pointed elsewhere keeps its history"
        );
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn concurrent_captures_in_one_workspace_do_not_collide() {
        let (base, workspace) = dirs();
        let snapshots = Arc::new(Snapshots::new(&base.join("data")));
        for i in 0..8 {
            std::fs::write(workspace.join(format!("f{i}.txt")), format!("{i}\n")).unwrap();
        }
        let captures = (0..8).map(|i| {
            let (snapshots, workspace) = (snapshots.clone(), workspace.clone());
            tokio::spawn(async move {
                let tree = snapshots.take(&workspace).await?;
                snapshots.record(&workspace, &format!("f{i}.txt")).await?;
                Ok::<_, Error>(tree)
            })
        });
        for capture in futures_util::future::join_all(captures).await {
            capture.unwrap().unwrap();
        }
        std::fs::remove_dir_all(base).ok();
    }
}
