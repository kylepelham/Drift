use super::*;

fn edit(path: &Path, diff: &str) -> Value {
    json!({
        "type": "tool", "tool": "edit", "callID": format!("e{}", path.display()),
        "state": {
            "status": "completed", "input": { "filePath": path, "oldString": "x", "newString": "y" },
            "output": "Edit applied successfully.", "metadata": { "filediff": { "file": path, "patch": diff } },
        },
    })
}

fn recorded(store: &Store, session: &str, kept: &Kept) -> Vec<(String, Option<String>, Option<String>)> {
    let content = |blob: &Option<Option<String>>| blob.as_ref().and_then(Option::as_ref).map(|id| kept.0[id].clone());
    let mut changes = Vec::new();

    for message in store.transcript(session).unwrap() {
        for row in message.parts {
            let Part::ToolCall {
                metadata: Some(metadata),
                ..
            } = row.part
            else {
                continue;
            };

            assert!(
                metadata.changes.is_none() || metadata.at.as_deref() == Some(message.info.id.as_str()),
                "stamped with its own message"
            );
            for change in metadata.changes.iter().flatten() {
                changes.push((change.path.clone(), content(&change.before), content(&change.after)));
            }
        }
    }

    changes.sort();
    changes
}

fn write_recent_history(conversation: &Conversation<'_>, workspace: &Path, now: i64) {
    let hour = 3_600_000;
    let write_new = json!({
        "type": "tool", "tool": "write", "callID": "w1",
        "state": {
            "status": "completed", "input": { "filePath": workspace.join("new.rs"), "content": "fresh\n" },
            "output": "Wrote.", "metadata": { "exists": false },
        },
    });
    let patch = json!({
        "type": "tool", "tool": "apply_patch", "callID": "p1",
        "state": { "status": "completed", "input": {}, "output": "Success.", "metadata": { "files": [
            {
                "filePath": workspace.join("gone.rs"), "type": "delete", "patch": "@@ -1 +0,0 @@\n-bye\n",
                "additions": 0, "deletions": 1,
            },
            {
                "filePath": workspace.join("was.rs"), "movePath": workspace.join("moved.rs"), "type": "move",
                "patch": "@@ -1 +1 @@\n-x\n+y\n", "additions": 1, "deletions": 1,
            },
        ] } },
    });

    conversation.message(
        "msg_0",
        now - 9 * 24 * hour,
        assistant(now - 9 * 24 * hour, json!({})),
        &[edit(&workspace.join("old.rs"), "@@ -1 +1 @@\n-early\n+late\n")],
    );
    conversation.message(
        "msg_1",
        now - 3 * hour,
        assistant(now - 3 * hour, json!({})),
        &[
            edit(&workspace.join("chain.rs"), "@@ -1 +1 @@\n-a\n+b\n"),
            edit(&workspace.join("touched.rs"), "@@ -1 +1 @@\n-1\n+2\n"),
        ],
    );
    conversation.message(
        "msg_2",
        now - 2 * hour,
        assistant(now - 2 * hour, json!({})),
        &[
            edit(&workspace.join("chain.rs"), "@@ -1 +1 @@\n-b\n+c\n"),
            edit(&workspace.join("touched.rs"), "@@ -1 +1 @@\n-2\n+3\n"),
        ],
    );
    conversation.message(
        "msg_3",
        now - hour,
        assistant(now - hour, json!({})),
        &[
            edit(&workspace.join("a.rs"), "@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n"),
            write_new,
            patch,
        ],
    );
}

#[test]
fn recent_edits_are_rebuilt_from_todays_files_and_anything_that_no_longer_matches_is_left_out() {
    let directory = dir();
    let workspace = directory.0.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    for (name, content) in [
        ("a.rs", "one\nTWO\nthree\n"),
        ("new.rs", "fresh\n"),
        ("moved.rs", "y\n"),
        ("chain.rs", "c\n"),
        ("touched.rs", "4\n"),
        ("old.rs", "late\n"),
    ] {
        std::fs::write(workspace.join(name), content).unwrap();
    }

    let source = directory.0.join("opencode.db");
    let conn = opencode(&source);
    session(&conn, "ses_a", None, &workspace.to_string_lossy(), None);
    write_recent_history(
        &Conversation::new(&conn, "ses_a"),
        &workspace,
        drift_engine::id::now_ms(),
    );
    drop(conn);
    let store = store_with(&directory.0, &[&workspace.to_string_lossy()]);
    let mut kept = Kept::default();

    let report = run_import(&store, &source, &HashSet::new(), &mut kept, &mut |_| {}).unwrap();

    assert_eq!(report.undoable, 5, "{report:?}");
    let some = |text: &str| Some(text.to_string());
    assert_eq!(
        recorded(&store, "ses_a", &kept),
        vec![
            ("a.rs".into(), some("one\ntwo\nthree\n"), some("one\nTWO\nthree\n")),
            ("chain.rs".into(), some("a\n"), some("b\n")),
            ("chain.rs".into(), some("b\n"), some("c\n")),
            ("gone.rs".into(), some("bye\n"), None),
            ("moved.rs".into(), None, some("y\n")),
            ("new.rs".into(), None, some("fresh\n")),
            ("was.rs".into(), some("x\n"), None),
        ],
        "touched.rs changed since, so neither of its edits is recorded; old.rs is over a week old"
    );
}

#[test]
fn only_a_conversations_newest_thirty_messages_get_undo_records() {
    let directory = dir();
    let workspace = directory.0.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("a.rs"), "b\n").unwrap();
    let source = directory.0.join("opencode.db");
    let conn = opencode(&source);
    session(&conn, "ses_a", None, &workspace.to_string_lossy(), None);
    let conversation = Conversation::new(&conn, "ses_a");
    let now = drift_engine::id::now_ms();

    conversation.message(
        "msg_000",
        now - 1000,
        assistant(now - 1000, json!({})),
        &[edit(&workspace.join("a.rs"), "@@ -1 +1 @@\n-a\n+b\n")],
    );
    for index in 1..=30 {
        conversation.message(
            &format!("msg_{index:03}"),
            now - 1000 + index,
            user(now - 1000 + index),
            &[json!({ "type": "text", "text": "more" })],
        );
    }
    drop(conn);
    let store = store_with(&directory.0, &[&workspace.to_string_lossy()]);

    let report = run_import(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| {}).unwrap();

    assert_eq!((report.imported, report.undoable), (1, 0));
}

#[test]
fn an_imported_edit_undoes_and_redoes_through_the_engine_and_the_rest_are_named() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let directory = dir();
    let workspace = directory.0.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("a.rs"), "after\n").unwrap();
    std::fs::write(workspace.join("z.rs"), "someone else's\n").unwrap();
    let source = directory.0.join("opencode.db");
    let conn = opencode(&source);
    session(&conn, "ses_a", None, &workspace.to_string_lossy(), None);
    let conversation = Conversation::new(&conn, "ses_a");
    let now = drift_engine::id::now_ms();

    conversation.message(
        "msg_1",
        now - 2000,
        user(now - 2000),
        &[json!({ "type": "text", "text": "fix" })],
    );
    conversation.message(
        "msg_2",
        now - 1000,
        assistant(now - 1000, json!({})),
        &[
            edit(&workspace.join("a.rs"), "@@ -1 +1 @@\n-before\n+after\n"),
            edit(&workspace.join("z.rs"), "@@ -1 +1 @@\n-z\n+zz\n"),
        ],
    );
    drop(conn);
    let engine = drift_engine::Engine::open_with(
        &directory.0.join("data"),
        drift_engine::Options {
            file_credentials: true,
            ..Default::default()
        },
    )
    .unwrap();
    engine
        .store
        .add_workspace(&workspace.to_string_lossy(), "ws", "")
        .unwrap();
    let mut history = History::new(&engine.snapshots).unwrap();

    let report = run_import(&engine.store, &source, &HashSet::new(), &mut history, &mut |_| {}).unwrap();
    drop(history);

    assert_eq!((report.imported, report.undoable), (1, 1), "{report:?}");
    let prompt = engine.store.transcript("ses_a").unwrap()[0].info.id.clone();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let undone = runtime.block_on(engine.revert("ses_a", &prompt)).unwrap();

    assert_eq!(std::fs::read_to_string(workspace.join("a.rs")).unwrap(), "before\n");
    assert_eq!(
        std::fs::read_to_string(workspace.join("z.rs")).unwrap(),
        "someone else's\n",
        "a file whose diff no longer matches is left alone"
    );
    assert_eq!(
        undone.unrecorded,
        [workspace.join("z.rs").to_string_lossy()],
        "and named"
    );

    runtime.block_on(engine.unrevert("ses_a")).unwrap();

    assert_eq!(
        std::fs::read_to_string(workspace.join("a.rs")).unwrap(),
        "after\n",
        "redo puts the edit back"
    );
}
