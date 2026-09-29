//! Working-tree snapshots in a shadow git dir, taken before any tool writes, so a turn can be undone.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

/// One shadow repository per workspace, kept out of the workspace so it never shows up in git status.
pub struct Snapshots {
    root: PathBuf,
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
        Self { root: data_dir.join("snapshots") }
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
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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

    /// Records the tree as it is now and returns a ref that `restore` and `diff` accept.
    pub async fn take(&self, workspace: &Path) -> Result<String, Error> {
        self.ensure(workspace).await?;
        self.git(workspace, &["add", "-A", "--", "."]).await?;
        self.git(workspace, &["write-tree"]).await
    }

    pub async fn diff(&self, workspace: &Path, from: &str) -> Result<String, Error> {
        self.git(workspace, &["add", "-A", "--", "."]).await?;
        let now = self.git(workspace, &["write-tree"]).await?;
        self.git(workspace, &["diff", "--no-color", from, &now]).await
    }

    /// Puts tracked files back and removes files created since; untracked-at-snapshot files are untouched.
    pub async fn restore(&self, workspace: &Path, tree: &str) -> Result<(), Error> {
        self.git(workspace, &["add", "-A", "--", "."]).await?;
        let now = self.git(workspace, &["write-tree"]).await?;
        let added = self.git(workspace, &["diff", "--name-only", "--diff-filter=A", tree, &now]).await?;
        self.git(workspace, &["read-tree", tree]).await?;
        self.git(workspace, &["checkout-index", "-a", "-f"]).await?;
        for file in added.lines().filter(|line| !line.is_empty()) {
            let _ = tokio::fs::remove_file(workspace.join(file)).await;
        }
        Ok(())
    }
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
    async fn take_diff_and_restore() {
        let (base, workspace) = dirs();
        let snapshots = Snapshots::new(&base.join("data"));
        std::fs::write(workspace.join("a.txt"), "one\n").unwrap();
        let before = snapshots.take(&workspace).await.unwrap();
        std::fs::write(workspace.join("a.txt"), "two\n").unwrap();
        std::fs::write(workspace.join("new.txt"), "n\n").unwrap();
        let diff = snapshots.diff(&workspace, &before).await.unwrap();
        assert!(diff.contains("-one"), "{diff}");
        assert!(diff.contains("+two"));
        assert!(diff.contains("new.txt"));
        snapshots.restore(&workspace, &before).await.unwrap();
        assert_eq!(std::fs::read_to_string(workspace.join("a.txt")).unwrap(), "one\n");
        assert!(!workspace.join("new.txt").exists());
        assert!(!workspace.join(".git").exists(), "shadow repo must not touch the workspace");
        std::fs::remove_dir_all(base).ok();
    }
}
