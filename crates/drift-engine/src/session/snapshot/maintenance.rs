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
            let present = self.existing(workspace, keep).await?;
            if present.is_empty() {
                let _ = self.git(workspace, &["update-ref", "-d", KEEP_REF]).await;
            } else {
                let mut listing = String::new();
                for (index, blob) in present.iter().enumerate() {
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
        }

        self.pack_kept(workspace).await;
        self.git(workspace, &["prune", &format!("--expire={expire}")])
            .await
            .map(|_| ())
    }

    /// Packs the history the keep reference pins, but only once loose objects pile up. Incremental:
    /// existing packs are never rewritten, and loose objects nothing pins are left for the prune.
    /// A workspace that is its own git repository has a shadow index naming its objects, which this
    /// cannot borrow; there the attempt quietly changes nothing, as before.
    async fn pack_kept(&self, workspace: &Path) {
        let Ok(counted) = self.git(workspace, &["count-objects"]).await else {
            return;
        };
        let loose = counted
            .split_whitespace()
            .next()
            .and_then(|count| count.parse::<usize>().ok());
        if loose.is_none_or(|count| count < self.pack_at_loose) {
            return;
        }

        let _ = self.git(workspace, &["repack", "-d", "-q", "-l"]).await;
    }

    /// Which of `keep` this repository holds. Recorded history names blobs wherever the change was
    /// made, so a moved or imported session can pin content that lives in another repository; a
    /// reference naming objects that are not there would stop git packing and pruning this one.
    async fn existing(&self, workspace: &Path, keep: &[String]) -> Result<Vec<String>, Error> {
        let ids = keep.iter().map(String::as_str).collect::<Vec<_>>().join("\n");
        let checked = self
            .run(
                workspace,
                &["cat-file", "--batch-check", "--buffer"],
                Some(ids.as_bytes()),
            )
            .await?;
        let mut present = Vec::with_capacity(keep.len());
        for (blob, line) in keep.iter().zip(String::from_utf8_lossy(&checked).lines()) {
            if !line.trim_end().ends_with("missing") {
                present.push(blob.clone());
            }
        }

        Ok(present)
    }
}
