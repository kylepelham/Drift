//! What each writing call changed, recorded around it so undo can put back exactly those files.

use std::path::{Path, PathBuf};

use super::snapshot::FileChange;
use crate::Engine;

/// The state taken before a writing call runs.
pub(super) enum Capture {
    /// The files the call names, each with its content before it ran.
    Paths(Vec<(String, Option<String>)>),
    /// The whole tree, for a call that could change anything (a shell command). Its changes are
    /// whatever differs afterwards, which can include someone else's edits made while it ran.
    Tree(String),
}

impl Engine {
    pub(super) async fn capture_before(&self, workspace: &Path, touched: Option<Vec<PathBuf>>) -> Result<Capture, String> {
        let Some(paths) = touched else {
            return self.snapshots.take(workspace).await.map(Capture::Tree).map_err(|e| e.to_string());
        };
        let mut before = Vec::new();
        for path in paths {
            let path = relative(workspace, &path);
            let blob = self.snapshots.record(workspace, &path).await.map_err(|e| e.to_string())?;
            before.push((path, blob));
        }
        Ok(Capture::Paths(before))
    }

    /// Only paths whose content actually changed; an untouched file is never part of an undo.
    pub(super) async fn capture_after(&self, workspace: &Path, capture: Capture) -> Result<Vec<FileChange>, String> {
        match capture {
            Capture::Tree(before) => {
                let after = self.snapshots.take(workspace).await.map_err(|e| e.to_string())?;
                self.snapshots.changes_between(workspace, &before, &after).await.map_err(|e| e.to_string())
            }
            Capture::Paths(paths) => {
                let mut changes = Vec::new();
                for (path, before) in paths {
                    let after = self.snapshots.record(workspace, &path).await.map_err(|e| e.to_string())?;
                    if after != before && !changes.iter().any(|c: &FileChange| c.path == path) {
                        changes.push(FileChange { path, before, after });
                    }
                }
                Ok(changes)
            }
        }
    }
}

/// Workspace-relative with `/`, matching the shadow repo's paths; absolute outside the workspace.
fn relative(workspace: &Path, path: &Path) -> String {
    match path.strip_prefix(workspace) {
        Ok(inside) => inside.to_string_lossy().replace('\\', "/"),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}
