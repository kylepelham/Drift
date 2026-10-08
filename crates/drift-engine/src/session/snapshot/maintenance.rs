use super::*;

/// Unreferenced objects younger than this survive a prune: a capture in flight has not been saved yet.
const PRUNE_GRACE: &str = "2.hours.ago";
const KEEP_REF: &str = "refs/drift/keep";

impl Snapshots {
    /// Drops unreferenced objects older than the grace period, protecting captures not yet saved.
    /// The blobs in `keep` are pinned under a private reference.
    pub async fn prune(&self, workspace: &Path, keep: &[String]) -> Result<(), Error> {
        self.prune_older_than(workspace, keep, PRUNE_GRACE).await
    }

    pub(super) async fn prune_older_than(&self, workspace: &Path, keep: &[String], expire: &str) -> Result<(), Error> {
        if !self.git_dir(workspace).join("HEAD").exists() {
            return Ok(());
        }
        let lock = self.lock_for(workspace);
        let _held = lock.lock().await;

        if keep.is_empty() {
            let _ = self.git(workspace, &["update-ref", "-d", KEEP_REF]).await;
        } else {
            let mut listing = String::new();
            for (index, blob) in keep.iter().enumerate() {
                let _ = writeln!(listing, "100644 blob {blob}\t{index}");
            }
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
}
