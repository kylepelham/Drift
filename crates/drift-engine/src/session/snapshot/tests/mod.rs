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

#[tokio::test]
async fn recording_bytes_again_keeps_their_blob_through_the_next_prune() {
    let (base, workspace) = dirs();
    let snapshots = Snapshots::new(&base.join("data"));
    let again = snapshots.store_bytes(&workspace, b"recorded twice").await.unwrap();
    let once = snapshots.store_bytes(&workspace, b"recorded once").await.unwrap();
    let objects = snapshots.git_dir(&workspace).join("objects");
    let hours_ago = std::time::SystemTime::now() - Duration::from_secs(3 * 60 * 60);
    for id in [&again, &once] {
        let path = objects.join(&id[..2]).join(&id[2..]);
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(hours_ago).unwrap();
        drop(file);
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
    }

    assert_eq!(
        snapshots.store_bytes(&workspace, b"recorded twice").await.unwrap(),
        again
    );
    snapshots.prune(&workspace, &[]).await.unwrap();

    assert!(stored(&snapshots, &workspace, &again), "a blob recorded again is fresh");
    assert!(!stored(&snapshots, &workspace, &once), "an old unpinned blob is pruned");
    std::fs::remove_dir_all(base).ok();
}
