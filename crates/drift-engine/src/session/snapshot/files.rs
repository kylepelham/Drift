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
        let content = match blob {
            Some(blob) => Some(self.git_bytes(workspace, &["cat-file", "blob", blob]).await?),
            None => None,
        };

        self.write(store, &file, content.as_deref()).await
    }

    /// Writes `content` to `file` whole, or removes it for `None`.
    pub async fn write(&self, store: &crate::store::Store, file: &Path, content: Option<&[u8]>) -> Result<(), Error> {
        let Some(content) = content else {
            return match tokio::fs::remove_file(file).await {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(Error::Failed(error.to_string())),
                _ => Ok(()),
            };
        };

        crate::tool::stage::replace(store, file, content)
            .await
            .map_err(|error| Error::Failed(error.to_string()))
    }

    /// Many stored blobs' bytes from one git process, as one per blob costs about 150 ms each on Windows.
    pub async fn blobs(&self, workspace: &Path, blobs: &[&str]) -> Result<HashMap<String, Vec<u8>>, Error> {
        if blobs.is_empty() {
            return Ok(HashMap::new());
        }
        let input = blobs.join("\n") + "\n";
        let output = self
            .run(workspace, &["cat-file", "--batch"], Some(input.as_bytes()))
            .await?;

        parse_batch(&output)
    }
}

/// `git cat-file --batch` output: per blob a `<id> <type> <size>` line, its bytes and a newline.
fn parse_batch(mut output: &[u8]) -> Result<HashMap<String, Vec<u8>>, Error> {
    let mut blobs = HashMap::new();
    while let Some(end) = output.iter().position(|byte| *byte == b'\n') {
        let header = String::from_utf8_lossy(&output[..end]).into_owned();
        let mut fields = header.split(' ');
        let (Some(id), Some(kind)) = (fields.next(), fields.next()) else {
            return Err(Error::Failed(format!("git cat-file: {header}")));
        };
        output = &output[end + 1..];
        // A blob the store lacks fails alone, when its file is written.
        if kind == "missing" {
            continue;
        }
        let size: usize = fields
            .next()
            .and_then(|size| size.parse().ok())
            .ok_or_else(|| Error::Failed(format!("git cat-file: {header}")))?;
        let content = output
            .get(..size)
            .ok_or_else(|| Error::Failed("git cat-file: output cut short".into()))?;
        blobs.insert(id.to_string(), content.to_vec());
        output = output.get(size + 1..).unwrap_or_default();
    }

    Ok(blobs)
}

async fn blocking(work: impl FnOnce() -> Result<String, Error> + Send + 'static) -> Result<String, Error> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| Error::Failed(error.to_string()))?
}
