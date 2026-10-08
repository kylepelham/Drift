use std::time::Duration;

use super::*;

mod limits;
mod measure;
mod repository;
mod trees;

fn dirs() -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("drift-snap-{}", crate::random_hex(4)));
    let workspace = base.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();

    (base, workspace)
}

fn stored(snapshots: &Snapshots, workspace: &Path, blob: &str) -> bool {
    std::process::Command::new("git")
        .arg("--git-dir")
        .arg(snapshots.git_dir(workspace))
        .args(["cat-file", "-e", blob])
        .status()
        .unwrap()
        .success()
}
