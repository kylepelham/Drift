use super::*;

/// The shadow repo stores bytes exactly as they are on disk, without line-ending or filter conversion.
/// These attributes override every in-tree attributes file, including encoding and ident rules.
/// Existing converted index entries are discarded when the attributes are installed.
const RAW_ATTRIBUTES: &str = "* -text -eol -filter -ident -working-tree-encoding\n";

impl Snapshots {
    /// Records the whole tree with the workspace's ignore rules applied.
    /// Captures serialize per workspace because sessions and subagents share its shadow index, so one
    /// that began after this call asked already holds everything it would see, and is shared instead.
    pub async fn take(&self, workspace: &Path) -> Result<Tree, Error> {
        let asked = std::time::Instant::now();
        let source = self.source(workspace).await;
        if source.is_none() && self.too_many.lock().unwrap().contains(workspace) {
            return Err(Error::TooManyFiles);
        }

        self.ensure(workspace).await?;
        let lock = self.lock_for(workspace);
        let _held = lock.lock().await;
        let git_dir = self.git_dir(workspace);
        if let Some((began, tree)) = self.latest.lock().unwrap().get(&git_dir)
            && *began > asked
        {
            return Ok(tree.clone());
        }

        let began = std::time::Instant::now();
        let tree = self.capture(workspace, source.as_ref()).await?;
        self.latest.lock().unwrap().insert(git_dir, (began, tree.clone()));

        Ok(tree)
    }

    async fn capture(&self, workspace: &Path, source: Option<&Source>) -> Result<Tree, Error> {
        // Every index writer holds the lock, so a remaining lock file belongs to a stopped capture.
        let _ = tokio::fs::remove_file(self.git_dir(workspace).join("index.lock")).await;
        self.store_raw_bytes(workspace).await?;
        let (oversized, pending) = match source {
            Some(source) => {
                self.seed(workspace, source).await;
                self.leave_out_large_changes(workspace, source).await?
            }
            None => (self.leave_out_large_files(workspace).await?, true),
        };
        let id = match self.unchanged_tree(workspace).await.filter(|_| !pending) {
            Some(id) => id,
            None => self.write_tree(workspace, source).await?,
        };

        Ok(Tree { id, oversized })
    }

    /// Adds every change to the shadow index and writes it as a tree, remembered against the index it came from.
    async fn write_tree(&self, workspace: &Path, source: Option<&Source>) -> Result<String, Error> {
        // --ignore-errors can exit nonzero after indexing usable files; write-tree decides capture success.
        let _ = self
            .run_with(workspace, &["add", "-A", "--ignore-errors", "--", "."], None, source)
            .await;
        let write: &[&str] = if source.is_some() {
            &["write-tree", "--missing-ok"]
        } else {
            &["write-tree"]
        };
        let raw = self.run_with(workspace, write, None, source).await?;
        let id = String::from_utf8_lossy(&raw).trim().to_string();
        if let Some(stamp) = self.index_stamp(workspace).await {
            self.trees
                .lock()
                .unwrap()
                .insert(self.git_dir(workspace), (stamp, id.clone()));
        }

        Ok(id)
    }

    /// The tree last written from this shadow index, if nothing has written the index since.
    async fn unchanged_tree(&self, workspace: &Path) -> Option<String> {
        let stamp = self.index_stamp(workspace).await?;
        let trees = self.trees.lock().unwrap();
        let (written, id) = trees.get(&self.git_dir(workspace))?;
        (*written == stamp).then(|| id.clone())
    }

    async fn index_stamp(&self, workspace: &Path) -> Option<Stamp> {
        let metadata = tokio::fs::metadata(self.git_dir(workspace).join("index")).await.ok()?;
        Some((metadata.len(), metadata.modified().ok()))
    }

    /// The repository at the workspace root, cached for this engine run; none for plain or nested folders.
    pub(super) async fn source(&self, workspace: &Path) -> Option<Source> {
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

    /// Sizes only changed, untracked or previously excluded files in a repository, without walking its tree.
    /// Also says whether anything is pending: with nothing changed or untracked, adding would change nothing.
    async fn leave_out_large_changes(
        &self,
        workspace: &Path,
        source: &Source,
    ) -> Result<(Vec<(String, Stamp)>, bool), Error> {
        let (modified, untracked) = tokio::try_join!(
            self.run_with(workspace, &["diff-files", "--name-only", "-z"], None, Some(source)),
            self.run_with(
                workspace,
                &["ls-files", "--others", "--exclude-standard", "-z"],
                None,
                Some(source),
            ),
        )?;
        let changed = !modified.is_empty() || !untracked.is_empty();

        let excluded = tokio::fs::read_to_string(self.git_dir(workspace).join("info/exclude"))
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
        let sized = move || {
            candidates
                .into_iter()
                .filter_map(|path| {
                    let metadata = std::fs::metadata(root.join(&path))
                        .ok()
                        .filter(|metadata| metadata.is_file() && metadata.len() > MAX_RECORDED_BYTES)?;
                    Some((path, (metadata.len(), metadata.modified().ok())))
                })
                .collect::<Vec<_>>()
        };
        let large = tokio::task::spawn_blocking(sized)
            .await
            .map_err(|error| Error::Failed(error.to_string()))?;
        // A changed exclusion list adds what left it; otherwise the index stamp tells whether removals changed anything.
        let pending = self.exclude(workspace, &large).await? || changed;

        Ok((large, pending))
    }

    /// Installs exact-byte attributes even for shadow repositories created before this rule.
    /// A new rule discards cached index entries that may contain converted bytes.
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
            .map_err(|error| Error::Failed(error.to_string()))?;
        tokio::fs::write(&path, RAW_ATTRIBUTES)
            .await
            .map_err(|error| Error::Failed(error.to_string()))?;

        match tokio::fs::remove_file(self.git_dir(workspace).join("index")).await {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(Error::Failed(error.to_string())),
            _ => Ok(()),
        }
    }

    /// Excludes large files from new captures and removes files indexed before they grew too large.
    /// The workspace's ignore rules apply during the walk.
    /// Only the shadow repository's exclude file is changed, never the workspace's .gitignore.
    async fn leave_out_large_files(&self, workspace: &Path) -> Result<Vec<(String, Stamp)>, Error> {
        let (root, limit) = (workspace.to_path_buf(), self.max_tree_files);
        let walked = tokio::task::spawn_blocking(move || large_files(&root, limit))
            .await
            .map_err(|error| Error::Failed(error.to_string()))?;
        let Some(large) = walked else {
            self.too_many.lock().unwrap().insert(workspace.to_path_buf());
            return Err(Error::TooManyFiles);
        };

        self.exclude(workspace, &large).await?;

        Ok(large)
    }

    /// Keeps `large` out of the shadow index from now on, and drops any already in it; says whether the list changed.
    async fn exclude(&self, workspace: &Path, large: &[(String, Stamp)]) -> Result<bool, Error> {
        let info = self.git_dir(workspace).join("info");
        tokio::fs::create_dir_all(&info)
            .await
            .map_err(|error| Error::Failed(error.to_string()))?;
        let mut lines = String::new();
        for (path, _) in large {
            let _ = writeln!(lines, "/{}", escape_pattern(path));
        }
        let file = info.join("exclude");
        let changed = tokio::fs::read_to_string(&file).await.ok().as_deref() != Some(lines.as_str());
        if changed {
            tokio::fs::write(file, lines)
                .await
                .map_err(|error| Error::Failed(error.to_string()))?;
        }

        if !large.is_empty() {
            let mut paths = String::new();
            for (path, _) in large {
                paths.push_str(path);
                paths.push('\0');
            }
            self.run(
                workspace,
                &["update-index", "--force-remove", "-z", "--stdin"],
                Some(paths.as_bytes()),
            )
            .await?;
        }

        Ok(changed)
    }
}

/// Finds oversized workspace files using git-style ignore rules, including outside git repositories.
/// Paths are relative with forward slashes; timestamps identify later changes to excluded files.
/// Returns `None` once the walk passes `limit` files.
pub(super) fn large_files(root: &Path, limit: usize) -> Option<Vec<(String, Stamp)>> {
    let mut files = 0;
    let mut large = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    for entry in walker.flatten() {
        let Some(metadata) = entry.metadata().ok().filter(std::fs::Metadata::is_file) else {
            continue;
        };

        files += 1;
        if files > limit {
            return None;
        }
        if metadata.len() > MAX_RECORDED_BYTES {
            let Ok(path) = entry.path().strip_prefix(root) else {
                continue;
            };

            large.push((
                path.to_string_lossy().replace('\\', "/"),
                (metadata.len(), metadata.modified().ok()),
            ));
        }
    }

    Some(large)
}

/// The path an exclude pattern written by `escape_pattern` matches.
fn unescape_pattern(pattern: &str) -> String {
    let mut output = String::new();
    let mut characters = pattern.chars();
    while let Some(character) = characters.next() {
        output.push(if character == '\\' {
            characters.next().unwrap_or(character)
        } else {
            character
        });
    }

    output
}

/// An exclude pattern matching exactly this path.
fn escape_pattern(path: &str) -> String {
    path.chars()
        .flat_map(|character| {
            if matches!(character, '*' | '?' | '[' | '\\' | '!' | '#' | ' ') {
                vec!['\\', character]
            } else {
                vec![character]
            }
        })
        .collect()
}
