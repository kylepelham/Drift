use std::time::Duration;

use super::*;

mod limits;
mod measure;
mod repository;
mod trees;

fn dirs() -> (PathBuf, PathBuf) {
    let base = crate::platform::temp::test_run().join(format!("drift-snap-{}", crate::random_hex(4)));
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

#[tokio::test]
async fn many_blobs_come_back_byte_for_byte_from_one_read() {
    let (base, workspace) = dirs();
    let snapshots = Snapshots::new(&base.join("data"));
    let binary = b"line\nnext\r\n\0\xff end".to_vec();
    let text = b"plain".to_vec();
    let empty = Vec::new();
    let mut ids = Vec::new();
    for bytes in [&binary, &text, &empty] {
        ids.push(snapshots.store_bytes(&workspace, bytes).await.unwrap());
    }
    let missing = "0123456789abcdef0123456789abcdef01234567";

    let mut asked: Vec<&str> = ids.iter().map(String::as_str).collect();
    asked.insert(1, missing);
    let blobs = snapshots.blobs(&workspace, &asked).await.unwrap();

    assert_eq!(blobs[&ids[0]], binary);
    assert_eq!(blobs[&ids[1]], text);
    assert_eq!(blobs[&ids[2]], empty);
    assert!(!blobs.contains_key(missing), "a blob the store lacks is left out");
}
