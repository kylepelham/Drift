//! What each writing call changed, recorded around it so undo can put back exactly those files.

use std::path::{Path, PathBuf};

use super::snapshot::{FileChange, Tree};
use crate::Engine;

#[derive(Debug, thiserror::Error)]
pub(super) enum CaptureError {
    #[error("{0}")]
    Snapshot(#[from] super::snapshot::Error),
    #[error("{0}")]
    Unrecorded(String),
}

/// The state taken before a writing call runs.
pub(super) enum Capture {
    /// The files the call names, each with its content before it ran.
    Paths(Vec<(String, Option<String>)>),
    /// The whole tree, for a call that could change anything (a shell command). What differs
    /// afterwards is only observed: the user, an editor or another session may have made any of it
    /// while the call ran, and nothing here can tell them apart.
    Tree(Tree),
    /// A whole tree that could not be taken, and why; tree changes are never undone, so the call still runs.
    Unrecorded(String),
    /// A plain folder too large to capture (a drive, a home folder); tree changes are never undone, so nothing is said.
    Skipped,
}

/// What a writing call changed, and the files it may have changed that are too large to record.
#[derive(Debug, Default)]
pub(super) struct Recorded {
    pub changes: Vec<FileChange>,
    pub unrecorded: Vec<String>,
    /// A time-ordered stamp taken once the call's writes are done: the order undo replays calls in.
    pub at: String,
    /// The tree taken after a whole-tree call, which the next one in the same step starts from.
    pub tree: Option<Tree>,
}

/// A call whose after state could not be recorded: what was done about it, in words for the model and the user.
#[derive(Debug)]
pub(super) struct Lost {
    pub note: String,
    /// The named files are back as they were before the call, so it changed nothing.
    pub put_back: bool,
    /// Files the call may have changed that undo cannot restore.
    pub unrecorded: Vec<String>,
}

impl Engine {
    pub(super) async fn capture_before(
        &self,
        workspace: &Path,
        touched: Option<Vec<PathBuf>>,
    ) -> Result<Capture, CaptureError> {
        let Some(paths) = touched else {
            return Ok(match self.snapshots.take(workspace).await {
                Ok(tree) => Capture::Tree(tree),
                Err(super::snapshot::Error::TooManyFiles) => Capture::Skipped,
                Err(error) => Capture::Unrecorded(error.to_string()),
            });
        };

        let mut before = Vec::new();
        for path in paths {
            let path = relative(workspace, &path);
            let blob = self.snapshots.record(workspace, &path).await?;
            before.push((path, blob));
        }

        Ok(Capture::Paths(before))
    }

    /// [`Self::capture_after`] that never loses history quietly: named files go back to their recorded
    /// before state, and anything that cannot is reported as unrecorded.
    pub(super) async fn record_call(&self, workspace: &Path, capture: Capture) -> Result<Recorded, Lost> {
        let before = match &capture {
            Capture::Paths(paths) => Some(paths.clone()),
            Capture::Tree(_) | Capture::Unrecorded(_) | Capture::Skipped => None,
        };
        // Completion stamps preserve write order when workers start in a different order.
        let at = crate::id::new("chg");
        let error = match self.capture_after(workspace, capture).await {
            Ok(recorded) => return Ok(Recorded { at, ..recorded }),
            Err(error) => error,
        };

        let Some(paths) = before else {
            let note = format!("Drift could not record what this command changed ({error}); undo cannot put it back.");
            return Err(Lost {
                note,
                put_back: false,
                unrecorded: Vec::new(),
            });
        };

        let mut stuck = Vec::new();
        for (path, blob) in paths {
            if let Err(failure) = self.snapshots.put(&self.store, workspace, &path, blob.as_deref()).await {
                stuck.push((path, failure.to_string()));
            }
        }

        if stuck.is_empty() {
            return Err(Lost {
                note: format!(
                    "Drift could not record what this call wrote ({error}), so it put the files back as they were."
                ),
                put_back: true,
                unrecorded: Vec::new(),
            });
        }

        let named: Vec<String> = stuck
            .iter()
            .map(|(path, failure)| format!("{path} ({failure})"))
            .collect();
        let note = format!(
            "Drift could not record what this call wrote ({error}) and could not put back {}; \
             undo cannot restore them.",
            named.join("; ")
        );

        Err(Lost {
            note,
            put_back: false,
            unrecorded: stuck.into_iter().map(|(path, _)| path).collect(),
        })
    }

    /// Only paths whose content actually changed; an untouched file is never part of an undo.
    async fn capture_after(&self, workspace: &Path, capture: Capture) -> Result<Recorded, CaptureError> {
        match capture {
            Capture::Unrecorded(reason) => Err(CaptureError::Unrecorded(reason)),
            Capture::Skipped => Ok(Recorded::default()),
            Capture::Tree(before) => {
                let after = self.snapshots.take(workspace).await?;
                let diff = self.snapshots.changes_between(workspace, &before, &after).await?;
                let changes = diff
                    .changes
                    .into_iter()
                    .map(|change| FileChange {
                        observed: true,
                        ..change
                    })
                    .collect();

                Ok(Recorded {
                    changes,
                    unrecorded: diff.unrecorded,
                    tree: Some(after),
                    ..Recorded::default()
                })
            }
            Capture::Paths(paths) => {
                let mut changes = Vec::new();
                for (path, before) in paths {
                    let after = self.snapshots.record(workspace, &path).await?;
                    if after != before && !changes.iter().any(|c: &FileChange| c.path == path) {
                        changes.push(FileChange {
                            path,
                            before,
                            after,
                            observed: false,
                        });
                    }
                }

                Ok(Recorded {
                    changes,
                    ..Recorded::default()
                })
            }
        }
    }
}

/// Workspace-relative with `/`, matching the shadow repo's paths; absolute outside the workspace.
pub(super) fn relative(workspace: &Path, path: &Path) -> String {
    match path.strip_prefix(workspace) {
        Ok(inside) => inside.to_string_lossy().replace('\\', "/"),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use crate::session::snapshot::MAX_RECORDED_BYTES;
    use crate::session::turn::tests::harness;

    #[tokio::test]
    async fn a_write_whose_result_cannot_be_recorded_is_put_back_and_said() {
        let h = harness().await;
        let ws = h._dir.join("ws");
        std::fs::write(ws.join("a.txt"), "before\n").unwrap();
        let capture = h
            .engine
            .capture_before(&ws, Some(vec![ws.join("a.txt"), ws.join("new.txt")]))
            .await
            .unwrap();

        // Written past what the store keeps, so the after state cannot be recorded.
        std::fs::write(ws.join("a.txt"), vec![b'x'; MAX_RECORDED_BYTES as usize + 1]).unwrap();
        std::fs::write(ws.join("new.txt"), "created\n").unwrap();
        let lost = h.engine.record_call(&ws, capture).await.unwrap_err();
        assert!(lost.put_back && lost.note.contains("put the files back"), "{lost:?}");
        assert_eq!(std::fs::read_to_string(ws.join("a.txt")).unwrap(), "before\n");
        assert!(!ws.join("new.txt").exists(), "a file the call created is removed again");
    }

    #[tokio::test]
    async fn a_command_whose_changes_cannot_be_recorded_says_undo_cannot_restore_them() {
        let h = harness().await;
        let ws = h._dir.join("ws");
        std::fs::write(ws.join("seed.txt"), "s\n").unwrap();
        let capture = h.engine.capture_before(&ws, None).await.unwrap();

        // The shadow store vanishing is an I/O failure that has nothing to do with the file sizes.
        std::fs::remove_dir_all(h._dir.join("data/snapshots")).unwrap();
        std::fs::write(h._dir.join("data/snapshots"), "not a directory").unwrap();
        let lost = h.engine.record_call(&ws, capture).await.unwrap_err();
        assert!(
            !lost.put_back && lost.note.contains("undo cannot put it back"),
            "{lost:?}"
        );
    }
}
