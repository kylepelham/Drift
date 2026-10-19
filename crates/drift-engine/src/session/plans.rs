//! Plan files: Markdown the plan agent writes under Drift's own data, one folder per workspace, which every
//! agent reads without asking and compaction keeps naming.

use std::path::{Path, PathBuf};

use crate::Engine;

/// The most plan files a request names after a summary.
const LISTED: usize = 10;

impl Engine {
    /// `<data>/plans/<workspace id>`: the one place a read-only primary agent may write.
    pub(crate) fn plans_dir(&self, workspace_id: &str) -> PathBuf {
        crate::tool::canonical(&self.data_dir.join("plans").join(workspace_id))
    }

    /// The workspace's plan files by name, so editing one never changes what a request after a summary says.
    pub(crate) fn plan_files(&self, workspace_id: &str) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(self.plans_dir(workspace_id)) else {
            return Vec::new();
        };
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| is_markdown(path) && path.is_file())
            .collect();
        found.sort();
        found.truncate(LISTED);

        found
    }

    /// What a request after a summary is told about the workspace's plans, when it has any.
    pub(crate) fn plans_note(&self, workspace_id: &str) -> Option<String> {
        let files = self.plan_files(workspace_id);
        if files.is_empty() {
            return None;
        }
        let listed: Vec<String> = files.iter().map(|path| format!("- {}", path.display())).collect();

        Some(format!(
            "<system-reminder>\nPlan files in this workspace; read one before acting on a plan it holds:\n{}\n</system-reminder>",
            listed.join("\n")
        ))
    }
}

/// A Markdown file inside `plans`.
pub(crate) fn is_plan_file(plans: &Path, path: &Path) -> bool {
    path.starts_with(plans) && path != plans && is_markdown(path)
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}
