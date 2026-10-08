use super::*;

#[tokio::test]
async fn undo_follows_the_order_writes_finished_not_the_order_their_messages_began() {
    let h = harness().await;
    let workspace = crate::tool::canonical(&h._dir.join("ws"));
    h.engine.snapshots.bind(&h.session.workspace_id, &workspace);
    let mut blobs = Vec::new();
    for content in ["ORIGINAL", "FROM_B", "FROM_A"] {
        std::fs::write(workspace.join("shared.txt"), content).unwrap();
        blobs.push(h.engine.snapshots.record(&workspace, "shared.txt").await.unwrap());
    }
    let model = crate::session::turn::tests::model();
    let prompt = h
        .engine
        .store
        .admit_prompt(&h.session.id, &model, vec![Part::Text { text: "both".into() }], None)
        .unwrap()
        .message
        .id;
    let (a, b) = (
        h.engine.store.create_reply(&h.session.id, &model, "build").unwrap(),
        h.engine.store.create_reply(&h.session.id, &model, "build").unwrap(),
    );
    let (b_at, a_at) = (crate::id::new("chg"), crate::id::new("chg"));
    let call = |at: &str, before: &Option<String>, after: &Option<String>| {
        let changes = json!([{ "path": "shared.txt", "before": before, "after": after }]);
        let metadata = json!({ "changes": changes, "owner": h.session.workspace_id, "at": at });
        Part::ToolCall {
            call_id: crate::id::new("call"),
            name: "write".into(),
            input: json!({}),
            status: crate::session::types::ToolStatus::Done,
            title: None,
            output: None,
            metadata: Some(Box::new(metadata.into())),
            started_at: None,
            finished_at: None,
        }
    };
    h.engine
        .store
        .add_part(&a.id, &h.session.id, call(&a_at, &blobs[1], &blobs[2]))
        .unwrap();
    h.engine
        .store
        .add_part(&b.id, &h.session.id, call(&b_at, &blobs[0], &blobs[1]))
        .unwrap();

    let undone = h.engine.revert(&h.session.id, &prompt).await.unwrap();
    assert!(
        undone.kept.is_empty(),
        "an unbroken chain of the session's own writes: {:?}",
        undone.kept
    );
    assert_eq!(read(&h, "shared.txt").as_deref(), Some("ORIGINAL"));
    h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(read(&h, "shared.txt").as_deref(), Some("FROM_A"));
}

#[tokio::test]
async fn undo_and_redo_merge_one_files_history_across_nested_workspace_moves() {
    let (h, first, second, file) = overlapping_writes(false).await;
    let undone = h.engine.revert(&h.session.id, &first).await.unwrap();
    assert!(
        undone.kept.is_empty(),
        "the uninterrupted A -> B -> C chain belongs to this session"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "A");
    h.engine.prune_snapshots().await;
    let redone = h.engine.unrevert(&h.session.id).await.unwrap();
    assert!(redone.kept.is_empty());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "C");
    h.engine.revert(&h.session.id, &second).await.unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "B");
    h.engine.revert(&h.session.id, &first).await.unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "A");
    h.engine.revert(&h.session.id, &second).await.unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "B");
    h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "C");
}

#[tokio::test]
async fn a_broken_cross_workspace_chain_preserves_the_entire_file() {
    let (h, first, _, file) = overlapping_writes(true).await;
    let undone = h.engine.revert(&h.session.id, &first).await.unwrap();
    assert_eq!(undone.kept.len(), 1);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "C",
        "A -> B then external X -> C is not partially undone to X"
    );
    let redone = h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(redone.kept.len(), 1);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "C");
}

#[tokio::test]
async fn undo_deduplicates_paths_across_overlapping_historical_workspaces() {
    let h = harness().await;
    let root = crate::tool::canonical(&h._dir.join("ws"));
    std::fs::create_dir_all(root.join("sub")).unwrap();
    let nested = h
        .engine
        .store
        .add_workspace(&root.join("sub").to_string_lossy(), "nested", "")
        .unwrap();
    let change = |owner: String, path: &str| {
        let file = crate::tool::canonical(&h.engine.root_of(&owner).unwrap().join(path));
        Net::new(
            owner,
            FileChange {
                path: path.into(),
                before: None,
                after: None,
                observed: false,
            },
            Some(file),
        )
    };
    let nets = [
        change(h.session.workspace_id.clone(), "sub/a.txt"),
        change(nested.id, "a.txt"),
    ];
    let held = tokio::time::timeout(Duration::from_secs(1), h.engine.turns_for(&nets))
        .await
        .expect("one reservation for the same physical file");
    assert!(
        tokio::time::timeout(
            Duration::from_millis(50),
            crate::tool::lock::files(&[root.join("sub/a.txt")])
        )
        .await
        .is_err()
    );
    drop(held);
}

#[tokio::test]
async fn undo_leaves_the_users_own_work_alone() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/notes.txt"), "mine\n").unwrap();
    let untouched = std::fs::metadata(h._dir.join("ws/notes.txt"))
        .unwrap()
        .modified()
        .unwrap();
    let (_, second) = two_writing_turns(&h).await;
    std::fs::write(h._dir.join("ws/notes.txt"), "mine, edited\n").unwrap();
    std::fs::write(h._dir.join("ws/fresh.txt"), "new idea\n").unwrap();
    let edited = std::fs::metadata(h._dir.join("ws/notes.txt"))
        .unwrap()
        .modified()
        .unwrap();
    assert_ne!(untouched, edited);

    h.engine.revert(&h.session.id, &second).await.unwrap();
    assert_eq!(
        read(&h, "notes.txt").as_deref(),
        Some("mine, edited\n"),
        "an unrelated edit survives the undo"
    );
    assert_eq!(
        read(&h, "fresh.txt").as_deref(),
        Some("new idea\n"),
        "a file the user created survives the undo"
    );
    assert_eq!(
        std::fs::metadata(h._dir.join("ws/notes.txt"))
            .unwrap()
            .modified()
            .unwrap(),
        edited,
        "an unrelated file is not even rewritten"
    );
    assert_eq!(
        read(&h, "a.txt").as_deref(),
        Some("one"),
        "the session's own change is undone"
    );
    h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(
        (read(&h, "notes.txt").as_deref(), read(&h, "fresh.txt").as_deref()),
        (Some("mine, edited\n"), Some("new idea\n"))
    );
}

#[tokio::test]
async fn a_file_edited_after_the_session_wrote_it_is_kept_and_reported() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    std::fs::write(h._dir.join("ws/a.txt"), "the user's fix\n").unwrap();
    let undone = h.engine.revert(&h.session.id, &second).await.unwrap();
    assert_eq!(undone.kept, ["a.txt"]);
    assert_eq!(
        undone.session.revert.as_ref().unwrap().kept,
        ["a.txt"],
        "the marker carries it for the UI"
    );
    assert_eq!(
        read(&h, "a.txt").as_deref(),
        Some("the user's fix\n"),
        "never overwritten"
    );
    assert_eq!(read(&h, "b.txt"), None, "the rest of the turn is still undone");

    std::fs::write(h._dir.join("ws/b.txt"), "user recreated it\n").unwrap();
    let redone = h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(
        redone.kept.len(),
        2,
        "redo keeps both files that differ from what the undo left"
    );
    assert_eq!(read(&h, "b.txt").as_deref(), Some("user recreated it\n"));
}

#[tokio::test]
async fn a_shell_commands_changes_are_reported_but_never_undone() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy {
        rules: vec![Rule {
            kind: "bash".into(),
            pattern: "echo made > made.txt".into(),
            decision: Decision::Allow,
        }],
    });
    std::fs::write(h._dir.join("ws/existing.txt"), "before\n").unwrap();
    h.provider
        .push(tool_call("bash", r#"{"command": "echo made > made.txt"}"#))
        .push(text("made it"));
    turn(&h, "make a file").await;
    assert!(read(&h, "made.txt").is_some_and(|text| text.contains("made")));
    let prompt_id = h.engine.store.transcript(&h.session.id).unwrap()[0].info.id.clone();
    let undone = h.engine.revert(&h.session.id, &prompt_id).await.unwrap();
    assert_eq!(
        undone.unattributed,
        ["made.txt"],
        "seen changing while the command ran, so not provably the session's"
    );
    assert!(read(&h, "made.txt").is_some(), "left as it is");
    assert_eq!(read(&h, "existing.txt").as_deref(), Some("before\n"));
    let redone = h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(redone.unattributed, ["made.txt"]);
}

#[tokio::test]
async fn a_user_edit_made_while_a_command_runs_survives_undo() {
    let h = harness().await;
    allow_shell(&h);
    std::fs::write(h._dir.join("ws/mine.txt"), "draft\n").unwrap();
    h.provider
        .push(tool_call("bash", r#"{"command": "sleep 1"}"#))
        .push(text("waited"));
    h.engine.submit(&h.session.id, prompt("wait a second")).await.unwrap();
    until_running_call(&h).await;
    std::fs::write(h._dir.join("ws/mine.txt"), "the user's edit\n").unwrap();
    until_idle(&h).await;
    let prompt_id = h.engine.store.transcript(&h.session.id).unwrap()[0].info.id.clone();
    let undone = h.engine.revert(&h.session.id, &prompt_id).await.unwrap();

    assert_eq!(
        read(&h, "mine.txt").as_deref(),
        Some("the user's edit\n"),
        "an edit made during the command is not the session's to undo"
    );
    assert_eq!(undone.unattributed, ["mine.txt"]);
    assert!(undone.kept.is_empty());
}

#[tokio::test]
async fn a_file_the_session_wrote_and_a_command_then_touched_is_left_alone() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy {
        rules: vec![
            Rule {
                kind: "edit".into(),
                pattern: "*".into(),
                decision: Decision::Allow,
            },
            Rule {
                kind: "bash".into(),
                pattern: "echo more >> a.txt".into(),
                decision: Decision::Allow,
            },
        ],
    });
    h.provider
        .push(write("a.txt", "one"))
        .push(tool_call("bash", r#"{"command": "echo more >> a.txt"}"#))
        .push(text("done"));
    turn(&h, "write then append").await;
    let after = read(&h, "a.txt").unwrap();
    let prompt_id = h.engine.store.transcript(&h.session.id).unwrap()[0].info.id.clone();
    let undone = h.engine.revert(&h.session.id, &prompt_id).await.unwrap();

    assert_eq!(
        read(&h, "a.txt"),
        Some(after),
        "part of its history is unattributed, so none of it is applied"
    );
    assert_eq!(undone.unattributed, ["a.txt"]);
}

#[tokio::test]
async fn a_user_edit_between_two_session_writes_survives_undo_and_redo() {
    let h = harness().await;
    allow_writes(&h);
    h.provider.push(write("a.txt", "one")).push(text("wrote one"));
    turn(&h, "first").await;
    std::fs::write(h._dir.join("ws/a.txt"), "the user's line\n").unwrap();
    h.provider
        .push(tool_call("read", r#"{"path": "a.txt"}"#))
        .push(write("a.txt", "two"))
        .push(text("wrote two"));
    turn(&h, "second").await;
    assert_eq!(read(&h, "a.txt").as_deref(), Some("two"));

    let first = h.engine.store.transcript(&h.session.id).unwrap()[0].info.id.clone();
    let undone = h.engine.revert(&h.session.id, &first).await.unwrap();
    assert_eq!(undone.kept, ["a.txt"], "the chain broke at the user's edit");
    assert_eq!(
        read(&h, "a.txt").as_deref(),
        Some("two"),
        "not rolled back past the user's edit"
    );
    let redone = h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(redone.kept, ["a.txt"]);
    assert_eq!(read(&h, "a.txt").as_deref(), Some("two"));
    let second = h
        .engine
        .store
        .transcript(&h.session.id)
        .unwrap()
        .iter()
        .filter(|message| message.info.role == Role::User)
        .nth(1)
        .unwrap()
        .info
        .id
        .clone();
    let undone = h.engine.revert(&h.session.id, &second).await.unwrap();
    assert!(undone.kept.is_empty(), "within one unbroken change, undo still works");
    assert_eq!(
        read(&h, "a.txt").as_deref(),
        Some("the user's line\n"),
        "back to where the user left it"
    );
}

#[tokio::test]
async fn undo_after_a_move_changes_the_files_where_they_were_written() {
    let h = harness().await;
    allow_writes(&h);
    h.provider.push(write("a.txt", "written in A")).push(text("done"));
    turn(&h, "write it").await;
    let elsewhere = h._dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("a.txt"), "B's own file\n").unwrap();
    let workspace = h
        .engine
        .store
        .add_workspace(&elsewhere.to_string_lossy(), "B", "")
        .unwrap();
    h.engine.move_session(&h.session.id, &workspace.id).unwrap();
    h.engine.prune_snapshots().await;
    let first = h.engine.store.transcript(&h.session.id).unwrap()[0].info.id.clone();
    let undone = h
        .engine
        .revert(&h.session.id, &first)
        .await
        .expect("no error after the move");

    assert!(undone.kept.is_empty(), "{:?}", undone.kept);
    assert_eq!(read(&h, "a.txt"), None, "undone in A, where it was written");
    assert_eq!(
        std::fs::read_to_string(elsewhere.join("a.txt")).unwrap(),
        "B's own file\n",
        "B is untouched"
    );
    h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(
        read(&h, "a.txt").as_deref(),
        Some("written in A"),
        "the blob survived the prune after the move"
    );
}
