//! File history in a shadow git dir, kept out of the workspace: what each writing call changed, as
//! blobs, so an undo can put back exactly those files and nothing else.

use std::collections::HashMap;
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
}

/// One path a call changed: its content before and after as shadow blobs, `None` for no file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileChange {
    /// Relative to the workspace with `/` separators, or absolute for a file outside it.
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum Error {
    NoGit,
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoGit => write!(f, "git is not installed"),
            Self::Failed(message) => write!(f, "git: {message}"),
        }
    }
}

impl Snapshots {
    pub fn new(data_dir: &Path) -> Self {
        Self { root: data_dir.join("snapshots"), locks: Mutex::default() }
    }

    fn lock_for(&self, workspace: &Path) -> Arc<tokio::sync::Mutex<()>> {
        self.locks.lock().unwrap().entry(self.git_dir(workspace)).or_default().clone()
    }

    fn git_dir(&self, workspace: &Path) -> PathBuf {
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
        let git_dir = self.git_dir(workspace);
        let mut command = Command::new("git");
        command
            .arg("--git-dir")
            .arg(&git_dir)
            .arg("--work-tree")
            .arg(workspace)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let output = command.output().await.map_err(|_| Error::NoGit)?;
        if !output.status.success() {
            return Err(Error::Failed(String::from_utf8_lossy(&output.stderr).trim().to_string()));
        }
        Ok(output.stdout)
    }

    async fn ensure(&self, workspace: &Path) -> Result<(), Error> {
        let git_dir = self.git_dir(workspace);
        if git_dir.join("HEAD").exists() {
            return Ok(());
        }
        tokio::fs::create_dir_all(&git_dir).await.map_err(|e| Error::Failed(e.to_string()))?;
        self.git(workspace, &["init", "-q"]).await?;
        self.git(workspace, &["config", "core.autocrlf", "false"]).await?;
        self.git(workspace, &["config", "user.email", "drift@localhost"]).await?;
        self.git(workspace, &["config", "user.name", "Drift"]).await?;
        Ok(())
    }

    /// Records the whole tree as it is now (the workspace's ignore rules apply) and returns its id.
    pub async fn take(&self, workspace: &Path) -> Result<String, Error> {
        let lock = self.lock_for(workspace);
        let _held = lock.lock().await;
        self.ensure(workspace).await?;
        // A file git cannot index (an unusual name, a locked file) must not stop every other file
        // being recorded; `--ignore-errors` still exits non-zero, so only the tree write decides.
        let _ = self.git(workspace, &["add", "-A", "--ignore-errors", "--", "."]).await;
        self.git(workspace, &["write-tree"]).await
    }

    /// Stores `path`'s content now and returns its blob, or `None` when there is no such file.
    pub async fn record(&self, workspace: &Path, path: &str) -> Result<Option<String>, Error> {
        self.hash(workspace, path, true).await
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
        if !file.is_file() {
            return Ok(None);
        }
        let file = file.to_string_lossy().into_owned();
        let args: Vec<&str> = if store { vec!["hash-object", "-w", "--no-filters", "--", &file] } else { vec!["hash-object", "--no-filters", "--", &file] };
        self.git(workspace, &args).await.map(Some)
    }

    /// Makes `path` hold `blob`, or removes it for `None`.
    pub async fn put(&self, workspace: &Path, path: &str, blob: Option<&str>) -> Result<(), Error> {
        let file = workspace.join(path);
        let Some(blob) = blob else {
            return match tokio::fs::remove_file(&file).await {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(Error::Failed(error.to_string())),
                _ => Ok(()),
            };
        };
        let content = self.git_bytes(workspace, &["cat-file", "blob", blob]).await?;
        if let Some(parent) = file.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| Error::Failed(e.to_string()))?;
        }
        tokio::fs::write(&file, content).await.map_err(|e| Error::Failed(e.to_string()))
    }

    /// Every path that differs between two trees, with its blob on each side.
    pub async fn changes_between(&self, workspace: &Path, before: &str, after: &str) -> Result<Vec<FileChange>, Error> {
        let raw = self.git_bytes(workspace, &["diff-tree", "-r", "--no-renames", "-z", before, after]).await?;
        Ok(parse_raw_diff(&raw))
    }
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
        changes.push(FileChange { path: path.to_string(), before: blob(before), after: blob(after) });
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
        let mut changes = snapshots.changes_between(&workspace, &before, &after).await.unwrap();
        changes.sort_by(|a, b| a.path.cmp(&b.path));
        let summary: Vec<(&str, bool, bool)> = changes.iter().map(|c| (c.path.as_str(), c.before.is_some(), c.after.is_some())).collect();
        assert_eq!(summary, [("a.txt", true, true), ("gone.txt", true, false), ("new.txt", false, true)]);
        assert_eq!(changes[0].before.as_deref(), Some(one.as_str()));

        assert_ne!(snapshots.current(&workspace, "a.txt").await.unwrap(), Some(one.clone()));
        snapshots.put(&workspace, "a.txt", Some(&one)).await.unwrap();
        snapshots.put(&workspace, "new.txt", None).await.unwrap();
        assert_eq!(std::fs::read_to_string(workspace.join("a.txt")).unwrap(), "one\n");
        assert!(!workspace.join("new.txt").exists());
        assert_eq!(snapshots.current(&workspace, "a.txt").await.unwrap(), Some(one));
        assert!(!workspace.join(".git").exists(), "shadow repo must not touch the workspace");
        std::fs::remove_dir_all(base).ok();
    }
}
