use super::*;

fn old_output(h: &Harness, name: &str) -> PathBuf {
    let path = h.engine.data_dir.join("tool-output").join("ses_old").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(&path).unwrap();
    file.set_modified(std::time::SystemTime::now() - Duration::from_secs(8 * 24 * 60 * 60))
        .unwrap();

    path
}

/// Bounded by the wall clock: the paused clock races ahead while the prune waits on its git child.
async fn until_gone(path: &Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);

    while path.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "{} was never pruned",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn every_blob_undo_needs_is_kept_through_a_prune() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;

    let kept = h
        .engine
        .store
        .recorded_blobs()
        .unwrap()
        .remove(&h.session.workspace_id)
        .unwrap();
    let workspace = crate::tool::canonical(&h._dir.join("ws"));
    let current = h.engine.snapshots.current(&workspace, "a.txt").await.unwrap().unwrap();
    assert!(kept.contains(&current), "the blob a redo would restore is referenced");
    assert_eq!(
        kept.len(),
        3,
        "one, two and bee, each once; a file that did not exist has no blob"
    );

    h.engine.prune_snapshots().await;
    h.engine.revert(&h.session.id, &second).await.unwrap();
    h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(read(&h, "a.txt").as_deref(), Some("two"));
}

#[tokio::test]
async fn housekeeping_runs_again_and_again_and_keeps_what_undo_needs() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    let first_old = old_output(&h, "first.log");
    let fresh = h.engine.data_dir.join("tool-output").join("ses_new").join("fresh.log");
    std::fs::create_dir_all(fresh.parent().unwrap()).unwrap();
    std::fs::write(&fresh, "recent").unwrap();
    tokio::time::pause();
    let maintaining = tokio::spawn(h.engine.clone().maintain());
    until_gone(&first_old).await;
    assert!(fresh.exists(), "recent output is kept");
    let second_old = old_output(&h, "second.log");
    tokio::time::sleep(crate::MAINTENANCE_INTERVAL).await;
    until_gone(&second_old).await;
    tokio::time::resume();
    maintaining.abort();

    h.engine.revert(&h.session.id, &second).await.unwrap();
    h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(
        read(&h, "a.txt").as_deref(),
        Some("two"),
        "pruning kept every blob undo and redo need"
    );
}
