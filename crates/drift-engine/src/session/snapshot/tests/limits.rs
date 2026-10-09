use super::*;

#[tokio::test]
async fn a_workspace_with_too_many_files_is_not_captured_and_not_walked_again() {
    let (base, workspace) = dirs();
    let mut snapshots = Snapshots::new(&base.join("data"));
    snapshots.max_tree_files = 3;

    for name in ["a", "b", "c", "d"] {
        std::fs::write(workspace.join(name), name).unwrap();
    }

    assert_eq!(snapshots.take(&workspace).await, Err(Error::TooManyFiles));
    assert!(
        snapshots
            .git(&workspace, &["count-objects"])
            .await
            .unwrap()
            .starts_with("0 objects"),
        "git never ran over the tree"
    );

    std::fs::remove_file(workspace.join("d")).unwrap();

    assert_eq!(
        snapshots.take(&workspace).await,
        Err(Error::TooManyFiles),
        "the verdict lasts while the engine runs"
    );
}

#[tokio::test]
async fn large_files_are_never_copied_into_the_store() {
    let (base, workspace) = dirs();
    let snapshots = Snapshots::new(&base.join("data"));
    std::fs::write(workspace.join("small.txt"), "s\n").unwrap();
    let before = snapshots.take(&workspace).await.unwrap();

    std::fs::write(
        workspace.join("huge [1].bin"),
        vec![b'x'; MAX_RECORDED_BYTES as usize + 1],
    )
    .unwrap();
    std::fs::write(workspace.join("small.txt"), "changed\n").unwrap();

    assert!(matches!(
        snapshots.record(&workspace, "huge [1].bin").await,
        Err(Error::TooLarge(_))
    ));

    let after = snapshots.take(&workspace).await.unwrap();
    let diff = snapshots.changes_between(&workspace, &before, &after).await.unwrap();
    let changed: Vec<_> = diff.changes.into_iter().map(|change| change.path).collect();
    assert_eq!(changed, ["small.txt"], "the large file is left out of the tree");
    assert_eq!(diff.unrecorded, ["huge [1].bin"]);

    let again = snapshots.take(&workspace).await.unwrap();
    assert_eq!(
        snapshots.changes_between(&workspace, &after, &again).await.unwrap(),
        TreeChanges::default(),
        "an untouched large file is not news"
    );

    std::fs::remove_dir_all(base).ok();
}

#[tokio::test]
async fn a_tracked_file_that_grows_past_the_limit_leaves_the_store_without_reading_as_deleted() {
    let (base, workspace) = dirs();
    let snapshots = Snapshots::new(&base.join("data"));
    std::fs::write(workspace.join("grows.log"), "small\n").unwrap();
    let small = snapshots.take(&workspace).await.unwrap();

    let big = vec![b'y'; MAX_RECORDED_BYTES as usize + 1];
    std::fs::write(workspace.join("grows.log"), &big).unwrap();

    let large = snapshots.take(&workspace).await.unwrap();
    let grew = snapshots.changes_between(&workspace, &small, &large).await.unwrap();
    assert_eq!(
        grew,
        TreeChanges {
            changes: vec![],
            unrecorded: vec!["grows.log".into()]
        },
        "not a deletion"
    );

    let big_blob = snapshots.current(&workspace, "grows.log").await.unwrap().unwrap();
    assert!(
        !stored(&snapshots, &workspace, &big_blob),
        "the large content never enters the store"
    );

    std::fs::write(workspace.join("grows.log"), "small again\n").unwrap();

    let shrunk = snapshots.take(&workspace).await.unwrap();
    let back = snapshots.changes_between(&workspace, &large, &shrunk).await.unwrap();
    assert_eq!(
        back,
        TreeChanges {
            changes: vec![],
            unrecorded: vec!["grows.log".into()]
        },
        "not a creation"
    );

    std::fs::write(workspace.join("grows.log"), "edited\n").unwrap();

    let edited = snapshots.take(&workspace).await.unwrap();
    let recorded = snapshots.changes_between(&workspace, &shrunk, &edited).await.unwrap();
    assert_eq!(recorded.changes.len(), 1, "once small it is recorded again");

    std::fs::remove_dir_all(base).ok();
}
