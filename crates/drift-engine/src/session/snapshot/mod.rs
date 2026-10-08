//! File history in a shadow git directory outside the workspace, stored as blobs per writing call.
//! Undo uses those blobs to restore exactly the files a call changed and nothing else.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::process::Command;

mod capture;
mod files;
mod git;
mod maintenance;
mod repository;

#[cfg(test)]
use capture::large_files;
use repository::{find_source, unconverted_changes};

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
    /// Object stores borrowed for tree comparisons; undo blobs live in the shadow store, safe from source gc.
    objects: std::ffi::OsString,
}

/// One path a call changed: its content before and after as shadow blobs, `None` for no file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileChange {
    /// Relative to the workspace with `/` separators, or absolute for a file outside it.
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
    /// Seen changing while the call ran rather than written by it.
    /// Another writer may have made the change, so undo and redo leave it alone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub observed: bool,
}

/// A whole-tree capture and the files left out because they were over the size limit.
/// Each oversized file keeps its size and modification time so a later capture can tell whether it changed.
#[derive(Clone, Debug, PartialEq)]
pub struct Tree {
    pub id: String,
    pub oversized: Vec<(String, Stamp)>,
}

pub type Stamp = (u64, Option<std::time::SystemTime>);

/// What differs between two trees.
/// A path over the size limit on either side is `unrecorded`, because its absence from a tree says nothing.
#[derive(Debug, Default, PartialEq)]
pub struct TreeChanges {
    pub changes: Vec<FileChange>,
    pub unrecorded: Vec<String>,
}

/// Files past this size are never copied into the store, and whole-tree captures leave them out.
/// A write to such a file is refused because it could not be undone.
pub const MAX_RECORDED_BYTES: u64 = 10 * 1024 * 1024;
/// Whole-tree captures are skipped above this many files, such as a drive or home folder, as they take minutes.
/// A git repository's top folder has no limit because its capture starts from the repository's index.
pub const MAX_TREE_FILES: usize = 50_000;

#[derive(Debug, PartialEq)]
pub enum Error {
    NoGit,
    TooLarge(String),
    TooManyFiles,
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoGit => write!(formatter, "git is not installed"),
            Self::TooLarge(path) => write!(
                formatter,
                "{path} is larger than {} MB, too large to keep for undo",
                MAX_RECORDED_BYTES / 1024 / 1024
            ),
            Self::TooManyFiles => write!(
                formatter,
                "the workspace holds more than {MAX_TREE_FILES} files, too many to record"
            ),
            Self::Failed(message) => write!(formatter, "git: {message}"),
        }
    }
}

impl std::error::Error for Error {}

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

    /// Binds a workspace directory to the owner id whose history survives moves and path changes.
    /// A legacy repository named from the old directory path is taken over once.
    /// Sessions moved to a different workspace still find each history through its original owner.
    pub fn bind(&self, owner: &str, root: &Path) {
        let owned = self.owned_dir(owner);
        let legacy = self.path_dir(root);
        if !owned.exists() && legacy.join("HEAD").exists() {
            let _ = std::fs::rename(&legacy, &owned);
        }
        self.owners.lock().unwrap().insert(root.to_path_buf(), owned);
    }

    fn owned_dir(&self, owner: &str) -> PathBuf {
        let owner = owner.replace(
            |character: char| !character.is_ascii_alphanumeric() && character != '_',
            "-",
        );
        self.root.join(format!("ws-{owner}"))
    }

    /// Deletes a workspace's whole history, once nothing of it is kept: its conversations are gone.
    pub fn forget(&self, owner: &str) {
        let owned = self.owned_dir(owner);
        self.owners.lock().unwrap().retain(|_, directory| *directory != owned);
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
        let (unrecordable, changes): (Vec<_>, Vec<_>) = parse_raw_diff(&raw)
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

/// Parses git diff-tree's NUL-delimited raw records; all-zero blob ids mean the file does not exist.
fn parse_raw_diff(raw: &[u8]) -> Vec<FileChange> {
    let text = String::from_utf8_lossy(raw);
    let mut fields = text.split('\0');
    let mut changes = Vec::new();
    while let (Some(header), Some(path)) = (fields.next(), fields.next()) {
        let parts: Vec<&str> = header.trim_start_matches(':').split(' ').collect();
        let [_, _, before, after, _] = parts[..] else { continue };
        let blob = |hash: &str| (!hash.chars().all(|character| character == '0')).then(|| hash.to_string());
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
mod tests;
