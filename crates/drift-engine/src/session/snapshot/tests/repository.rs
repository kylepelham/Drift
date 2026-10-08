use super::*;

/// Git in the test repository itself, as its user would run it.
fn repo_git(workspace: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(workspace)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Attributes that would change bytes on the way into git, and a filter the shadow repo can see.
fn hostile_attributes(snapshots: &Snapshots, workspace: &Path, attributes: &str) {
    std::fs::write(workspace.join(".gitattributes"), attributes).unwrap();
    let git_dir = snapshots.git_dir(workspace);
    let config = |key: &str, value: &str| {
        std::process::Command::new("git")
            .arg("--git-dir")
            .arg(&git_dir)
            .args(["config", key, value])
            .status()
            .unwrap()
    };
    config("filter.upper.clean", "tr a-z A-Z");
    config("filter.upper.smudge", "cat");
}

#[tokio::test]
async fn a_repository_is_captured_from_its_own_index_whatever_its_size_and_conversions() {
    let (base, workspace) = dirs();
    let workspace = crate::tool::canonical(&workspace);
    repo_git(&workspace, &["init", "-q"]);
    for (key, value) in [
        ("core.autocrlf", "true"),
        ("user.email", "dev@example.com"),
        ("user.name", "Dev"),
    ] {
        repo_git(&workspace, &["config", key, value]);
    }
    std::fs::write(workspace.join(".gitattributes"), "* text=auto\n").unwrap();
    for name in ["a", "b", "c", "d", "e"] {
        std::fs::write(
            workspace.join(format!("{name}.txt")),
            format!("{name} one\r\n{name} two\r\n"),
        )
        .unwrap();
    }
    repo_git(&workspace, &["add", "-A"]);
    repo_git(&workspace, &["commit", "-qm", "start"]);
    std::thread::sleep(Duration::from_millis(1100));
    repo_git(&workspace, &["update-index", "--refresh"]);
    let mut snapshots = Snapshots::new(&base.join("data"));
    snapshots.max_tree_files = 3;
    let before = snapshots
        .take(&workspace)
        .await
        .expect("a repository has no file limit");
    let counted = snapshots.git(&workspace, &["count-objects"]).await.unwrap();
    assert!(
        counted.starts_with("0 objects"),
        "the repository's files were not copied in: {counted}"
    );

    std::fs::write(workspace.join("a.txt"), "a changed\r\n").unwrap();
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(workspace.join("b.txt"), "b one\r\nb two\r\n").unwrap();
    std::fs::write(workspace.join("new.txt"), "n\r\n").unwrap();
    let after = snapshots.take(&workspace).await.unwrap();
    let mut changed: Vec<_> = snapshots
        .changes_between(&workspace, &before, &after)
        .await
        .unwrap()
        .changes
        .into_iter()
        .map(|change| change.path)
        .collect();
    changed.sort();
    assert_eq!(changed, ["a.txt", "new.txt"]);
    let kept = snapshots.record(&workspace, "c.txt").await.unwrap().unwrap();
    assert!(
        stored(&snapshots, &workspace, &kept),
        "a blob kept for undo lives in the shadow store, not only in the repository"
    );
    std::fs::remove_dir_all(base).ok();
}

#[tokio::test]
async fn trees_hold_the_bytes_on_disk_whatever_the_attributes_say() {
    let (base, workspace) = dirs();
    let snapshots = Snapshots::new(&base.join("data"));
    std::fs::write(workspace.join("seed"), "x").unwrap();
    snapshots.take(&workspace).await.unwrap();
    hostile_attributes(
        &snapshots,
        &workspace,
        "* text=auto\n*.txt eol=lf ident filter=upper working-tree-encoding=UTF-16\n",
    );
    std::fs::write(workspace.join("a.txt"), "one $Id$\r\ntwo\r\n").unwrap();
    std::fs::write(workspace.join("b.md"), "crlf\r\n").unwrap();
    let before = snapshots.take(&workspace).await.unwrap();
    std::fs::write(workspace.join("a.txt"), "three\r\n").unwrap();
    std::fs::write(workspace.join("b.md"), "changed\r\n").unwrap();
    let after = snapshots.take(&workspace).await.unwrap();
    for change in snapshots
        .changes_between(&workspace, &before, &after)
        .await
        .unwrap()
        .changes
    {
        assert_eq!(
            snapshots.current(&workspace, &change.path).await.unwrap(),
            change.after,
            "{} after is the file's exact bytes",
            change.path
        );
        snapshots
            .put(
                &crate::store::tests::store(),
                &workspace,
                &change.path,
                change.before.as_deref(),
            )
            .await
            .unwrap();
    }
    assert_eq!(std::fs::read(workspace.join("a.txt")).unwrap(), b"one $Id$\r\ntwo\r\n");
    assert_eq!(std::fs::read(workspace.join("b.md")).unwrap(), b"crlf\r\n");
    std::fs::remove_dir_all(base).ok();
}

#[tokio::test]
async fn a_shadow_repo_made_before_the_rule_is_brought_up_to_it() {
    let (base, workspace) = dirs();
    let snapshots = Snapshots::new(&base.join("data"));
    std::fs::write(workspace.join("seed"), "x").unwrap();
    snapshots.take(&workspace).await.unwrap();
    hostile_attributes(&snapshots, &workspace, "* text=auto\n*.txt filter=upper\n");
    std::fs::write(workspace.join("a.txt"), "lower\r\n").unwrap();
    std::fs::remove_file(snapshots.git_dir(&workspace).join("info/attributes")).unwrap();
    snapshots.git(&workspace, &["add", "-A"]).await.unwrap();
    let converted = snapshots
        .git(&workspace, &["ls-files", "-s", "--", "a.txt"])
        .await
        .unwrap();

    let tree = snapshots.take(&workspace).await.unwrap();
    let entry = snapshots
        .git(&workspace, &["ls-tree", &tree.id, "--", "a.txt"])
        .await
        .unwrap();
    let exact = snapshots.current(&workspace, "a.txt").await.unwrap().unwrap();
    assert!(
        entry.contains(&exact),
        "the stale converted entry is replaced: {converted} vs {entry}"
    );
    std::fs::remove_dir_all(base).ok();
}
