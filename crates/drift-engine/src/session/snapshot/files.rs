use super::*;

impl Snapshots {
    /// Stores `path`'s content now and returns its blob, or `None` when there is no such file.
    pub async fn record(&self, workspace: &Path, path: &str) -> Result<Option<String>, Error> {
        self.hash(workspace, path, true).await
    }

    /// Stores bytes in the workspace's history even when they are not currently on disk.
    /// Imported edits use this for their earlier versions.
    pub async fn store_bytes(&self, workspace: &Path, bytes: &[u8]) -> Result<String, Error> {
        self.ensure(workspace).await?;
        let objects = self.git_dir(workspace).join("objects");
        let bytes = bytes.to_vec();

        blocking(move || blob::store(&objects, &bytes)).await
    }

    /// `path`'s blob id now without storing it, to compare against a recorded one.
    pub async fn current(&self, workspace: &Path, path: &str) -> Result<Option<String>, Error> {
        self.hash(workspace, path, false).await
    }

    async fn hash(&self, workspace: &Path, path: &str, store: bool) -> Result<Option<String>, Error> {
        // A missing file's after state still needs an initialized history store.
        if store {
            self.ensure(workspace).await?;
        }
        let file = workspace.join(path);
        let Ok(metadata) = std::fs::metadata(&file).map(|metadata| (metadata.is_file(), metadata.len())) else {
            return Ok(None);
        };
        if !metadata.0 {
            return Ok(None);
        }
        if store && metadata.1 > MAX_RECORDED_BYTES {
            return Err(Error::TooLarge(path.to_string()));
        }

        let objects = store.then(|| self.git_dir(workspace).join("objects"));

        blocking(move || blob::of_file(&file, objects.as_deref()))
            .await
            .map(Some)
    }

    /// A stored blob's bytes.
    pub async fn blob(&self, workspace: &Path, blob: &str) -> Result<Vec<u8>, Error> {
        self.git_bytes(workspace, &["cat-file", "blob", blob]).await
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
            .map_err(|error| Error::Failed(error.to_string()))
    }
}

async fn blocking(work: impl FnOnce() -> Result<String, Error> + Send + 'static) -> Result<String, Error> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| Error::Failed(error.to_string()))?
}
