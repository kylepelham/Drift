use super::*;

#[tokio::test]
async fn blobs_round_trip_and_tree_diffs_name_each_changed_path() {
    let (base, workspace) = dirs();
    let snapshots = Snapshots::new(&base.join("data"));
    std::fs::write(workspace.join("a.txt"), "one\n").unwrap();
    std::fs::write(workspace.join("gone.txt"), "g\n").unwrap();
    let one = snapshots.record(&workspace, "a.txt").await.unwrap().unwrap();
    assert_eq!(snapshots.record(&workspace, "missing.txt").await.unwrap(), None);
    let before = snapshots.take(&workspace).await.unwrap();
    std::fs::write(workspace.join("a.txt"), "two\n").unwrap();
    std::fs::remove_file(workspace.join("gone.txt")).unwrap();
    std::fs::write(workspace.join("new.txt"), "n\n").unwrap();
    let after = snapshots.take(&workspace).await.unwrap();
    let mut changes = snapshots
        .changes_between(&workspace, &before, &after)
        .await
        .unwrap()
        .changes;
    changes.sort_by(|left, right| left.path.cmp(&right.path));
    let summary: Vec<_> = changes
        .iter()
        .map(|change| (change.path.as_str(), change.before.is_some(), change.after.is_some()))
        .collect();
    assert_eq!(
        summary,
        [
            ("a.txt", true, true),
            ("gone.txt", true, false),
            ("new.txt", false, true)
        ]
    );
    assert_eq!(changes[0].before.as_deref(), Some(one.as_str()));

    assert_ne!(snapshots.current(&workspace, "a.txt").await.unwrap(), Some(one.clone()));
    let store = crate::store::tests::store();
    snapshots.put(&store, &workspace, "a.txt", Some(&one)).await.unwrap();
    snapshots.put(&store, &workspace, "new.txt", None).await.unwrap();
    assert_eq!(std::fs::read_to_string(workspace.join("a.txt")).unwrap(), "one\n");
    assert!(!workspace.join("new.txt").exists());
    assert_eq!(snapshots.current(&workspace, "a.txt").await.unwrap(), Some(one));
    assert!(
        !workspace.join(".git").exists(),
        "shadow repo must not touch the workspace"
    );
    std::fs::remove_dir_all(base).ok();
}

#[test]
fn git_runs_from_the_workspace_root_whatever_the_engines_own_directory() {
    let (base, workspace) = dirs();
    let snapshots = Snapshots::new(&base.join("data"));
    let command = snapshots.command(&workspace, &["add", "-A", "--", "."], false);
    assert_eq!(
        command.as_std().get_current_dir(),
        Some(workspace.as_path()),
        "`.` must mean the workspace, not where the app was started"
    );
    std::fs::remove_dir_all(base).ok();
}

#[tokio::test]
async fn a_capture_stopped_part_way_does_not_block_the_next() {
    let (base, workspace) = dirs();
    let snapshots = Snapshots::new(&base.join("data"));
    std::fs::write(workspace.join("a.txt"), "a\n").unwrap();
    let before = snapshots.take(&workspace).await.unwrap();
    std::fs::write(snapshots.git_dir(&workspace).join("index.lock"), "").unwrap();
    std::fs::write(workspace.join("a.txt"), "changed\n").unwrap();
    let after = snapshots.take(&workspace).await.unwrap();
    assert_ne!(
        before.id, after.id,
        "the change was captured despite the stopped capture's lock file"
    );
}

#[tokio::test]
async fn a_prune_keeps_what_history_refers_to_and_drops_the_rest() {
    let (base, workspace) = dirs();
    let snapshots = Snapshots::new(&base.join("data"));
    std::fs::write(workspace.join("kept.txt"), "kept\n").unwrap();
    std::fs::write(workspace.join("dropped.txt"), "dropped\n").unwrap();
    let kept = snapshots.record(&workspace, "kept.txt").await.unwrap().unwrap();
    let dropped = snapshots.record(&workspace, "dropped.txt").await.unwrap().unwrap();
    snapshots
        .prune_older_than(&workspace, std::slice::from_ref(&kept), "now")
        .await
        .unwrap();
    assert!(snapshots.git(&workspace, &["cat-file", "-e", &kept]).await.is_ok());
    assert!(snapshots.git(&workspace, &["cat-file", "-e", &dropped]).await.is_err());
    snapshots.prune(&workspace, &[]).await.unwrap();
    assert!(
        snapshots.git(&workspace, &["cat-file", "-e", &kept]).await.is_ok(),
        "the grace period protects recent objects"
    );
    std::fs::remove_dir_all(base).ok();
}

#[tokio::test]
async fn a_bound_workspace_keeps_its_history_under_its_owner_wherever_its_directory_is() {
    let (base, workspace) = dirs();
    let snapshots = Snapshots::new(&base.join("data"));
    std::fs::write(workspace.join("a.txt"), "one\n").unwrap();
    let old = snapshots.record(&workspace, "a.txt").await.unwrap().unwrap();
    let legacy = snapshots.path_dir(&workspace);
    assert!(legacy.join("HEAD").exists());
    snapshots.bind("ws_1", &workspace);
    assert!(!legacy.exists(), "the path-named repo was taken over");
    std::fs::write(workspace.join("a.txt"), "two\n").unwrap();
    let store = crate::store::tests::store();
    snapshots.put(&store, &workspace, "a.txt", Some(&old)).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
        "one\n",
        "history from before the binding is kept"
    );

    let moved = base.join("moved");
    std::fs::rename(&workspace, &moved).unwrap();
    snapshots.bind("ws_1", &moved);
    std::fs::write(moved.join("a.txt"), "three\n").unwrap();
    snapshots.put(&store, &moved, "a.txt", Some(&old)).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(moved.join("a.txt")).unwrap(),
        "one\n",
        "a workspace pointed elsewhere keeps its history"
    );
    std::fs::remove_dir_all(base).ok();
}

#[tokio::test]
async fn concurrent_captures_in_one_workspace_do_not_collide() {
    let (base, workspace) = dirs();
    let snapshots = Arc::new(Snapshots::new(&base.join("data")));
    for index in 0..8 {
        std::fs::write(workspace.join(format!("f{index}.txt")), format!("{index}\n")).unwrap();
    }
    let captures = (0..8).map(|index| {
        let (snapshots, workspace) = (snapshots.clone(), workspace.clone());
        tokio::spawn(async move {
            let tree = snapshots.take(&workspace).await?;
            snapshots.record(&workspace, &format!("f{index}.txt")).await?;
            Ok::<_, Error>(tree)
        })
    });
    for capture in futures_util::future::join_all(captures).await {
        capture.unwrap().unwrap();
    }
    std::fs::remove_dir_all(base).ok();
}
