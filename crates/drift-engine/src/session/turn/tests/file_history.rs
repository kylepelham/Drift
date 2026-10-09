use super::*;

#[tokio::test]
async fn a_patch_in_a_real_turn_keeps_its_display_diff_beside_undos_record() {
    let h = harness().await;
    mutate_model(&h, |model| model.profile = crate::llm::catalog::ToolProfile::ApplyPatch);
    let patch = json!({ "patch": "*** Begin Patch\n*** Add File: new.txt\n+fresh\n*** End Patch\n" }).to_string();
    h.provider.push(tool_call("apply_patch", &patch)).push(text("patched"));
    h.engine.submit(&h.session.id, prompt("add new.txt")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    let call = tool(&messages[1].parts[0]);
    assert_eq!(call.status, ToolStatus::Done, "{:?}", call.output);
    let metadata = call.metadata.unwrap();
    assert!(
        !metadata.changes.as_ref().unwrap()[0].path.is_empty(),
        "undo's record: {metadata:?}"
    );
    assert_eq!(
        metadata.file_changes.as_ref().unwrap()[0].relative_path,
        "new.txt",
        "the display record survives undo's merge: {metadata:?}"
    );
    assert!(metadata.file_changes.as_ref().unwrap()[0].patch.contains("+fresh"));
}

#[tokio::test]
async fn an_edit_waits_while_another_writer_holds_the_file() {
    let h = harness().await;
    let file = h._dir.join("ws/a.txt");
    std::fs::write(&file, "one\ntwo\n").unwrap();
    rule(&h, "edit", "*", Decision::Allow);
    h.provider
        .push(tool_call("read", r#"{"path": "a.txt"}"#))
        .push(tool_call(
            "edit",
            r#"{"path": "a.txt", "old_string": "one", "new_string": "ONE"}"#,
        ))
        .push(text("done"));
    let held = crate::tool::lock::files(std::slice::from_ref(&file)).await;
    h.engine.submit(&h.session.id, prompt("edit a")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(300)).await;
    std::fs::write(&file, "one\nTWO\n").unwrap();

    assert!(h.engine.turns.is_running(&h.session.id), "the edit waits its turn");

    drop(held);
    until_idle(&h).await;

    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "ONE\nTWO\n",
        "it starts from the other writer's bytes, so neither change is lost"
    );
}

#[tokio::test]
async fn a_file_read_before_a_restart_may_be_edited_after_it() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "one\n").unwrap();
    h.provider
        .push(tool_call("read", r#"{"path": "a.txt"}"#))
        .push(text("read it"));
    h.engine.submit(&h.session.id, prompt("read a")).await.await_ok();
    until_idle(&h).await;

    let reopened = reopen(&h);
    reopened.permissions.set_policy(Policy {
        rules: vec![Rule {
            kind: "edit".into(),
            pattern: "*".into(),
            decision: Decision::Allow,
        }],
    });
    h.provider
        .push(tool_call(
            "edit",
            r#"{"path": "a.txt", "old_string": "one", "new_string": "two"}"#,
        ))
        .push(text("edited"));
    reopened.submit(&h.session.id, prompt("edit a")).await.await_ok();
    for _ in 0..500 {
        if !reopened.turns.is_running(&h.session.id) {
            break;
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(
        std::fs::read_to_string(h._dir.join("ws/a.txt")).unwrap(),
        "two\n",
        "the read was kept across the restart"
    );
}

#[tokio::test]
async fn a_write_is_refused_when_its_files_cannot_be_recorded() {
    let h = harness().await;
    rule(&h, "edit", "*", Decision::Allow);
    std::fs::write(h._dir.join("data/snapshots"), "not a directory").unwrap();
    h.provider
        .push(tool_call("write", r#"{"path": "new.txt", "content": "x\n"}"#))
        .push(text("noted"));
    h.engine.submit(&h.session.id, prompt("write")).await.await_ok();
    until_idle(&h).await;

    assert!(
        !h._dir.join("ws/new.txt").exists(),
        "nothing may be written that could not be undone"
    );
    let messages = transcript(&h);
    let call = tool(&messages[1].parts[0]);
    assert_eq!(call.status, ToolStatus::Error);
    assert!(call.output.unwrap().contains("could not record the files"));
}

#[tokio::test]
async fn a_command_whose_tree_cannot_be_captured_still_runs_and_says_so() {
    let h = harness().await;
    rule(&h, "bash", "*", Decision::Allow);
    std::fs::write(h._dir.join("data/snapshots"), "not a directory").unwrap();
    h.provider
        .push(tool_call("bash", &json!({ "command": "touch made.txt" }).to_string()))
        .push(text("noted"));
    h.engine.submit(&h.session.id, prompt("write")).await.await_ok();
    until_idle(&h).await;

    assert!(
        h._dir.join("ws/made.txt").exists(),
        "a shell's tree is only observed, so a missing record does not stop it"
    );
    let messages = transcript(&h);
    let call = tool(&messages[1].parts[0]);
    assert_eq!(call.status, ToolStatus::Done);
    assert!(
        call.output
            .unwrap()
            .contains("could not record what this command changed"),
        "{:?}",
        call.output
    );
    let metadata = call.metadata.unwrap();
    assert!(metadata.history_error.is_some());
    let note = metadata.history_error.as_deref().unwrap();
    assert_eq!(
        metadata.notes.as_ref(),
        Some(&vec![note.to_string()]),
        "listed apart, so the UI shows it under the call"
    );
}

#[tokio::test]
async fn stop_ends_a_capture_that_has_not_finished() {
    let h = harness().await;
    rule(&h, "bash", "*", Decision::Allow);
    let workspace = h._dir.join("ws");
    h.engine.snapshots.bind(&h.session.workspace_id, &workspace);
    let lock = h.engine.snapshots.lock_for(&workspace);
    let held = lock.lock().await;
    h.provider
        .push(tool_call("bash", &json!({ "command": "touch made.txt" }).to_string()))
        .push(text("unused"));
    h.engine.submit(&h.session.id, prompt("write")).await.await_ok();
    for _ in 0..400 {
        if transcript(&h)
            .iter()
            .flat_map(|message| &message.parts)
            .any(|row| matches!(row.part, Part::ToolCall { .. }))
        {
            break;
        }

        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    tokio::time::sleep(Duration::from_millis(50)).await;

    assert!(h.engine.abort(&h.session.id));

    until_idle(&h).await;
    drop(held);

    assert!(!h._dir.join("ws/made.txt").exists());
    let messages = transcript(&h);
    let call = tool(&messages[1].parts[0]);
    assert_eq!(
        (call.status, call.output),
        (ToolStatus::Error, Some("Aborted while recording the files first."))
    );
}

#[tokio::test]
async fn a_call_that_cannot_be_recorded_does_not_run() {
    let h = harness().await;
    rule(&h, "edit", "*", Decision::Allow);
    h.provider
        .push(tool_call("write", r#"{"path": "new.txt", "content": "x\n"}"#))
        .push(text("noted"));
    h.engine
        .store
        .lock()
        .execute(
            "CREATE TRIGGER block BEFORE UPDATE ON part BEGIN SELECT RAISE(ABORT, 'disk full'); END",
            [],
        )
        .unwrap();
    h.engine.submit(&h.session.id, prompt("write")).await.await_ok();
    until_idle(&h).await;
    h.engine.store.lock().execute("DROP TRIGGER block", []).unwrap();

    assert!(
        !h._dir.join("ws/new.txt").exists(),
        "a write whose start could not be recorded must not happen"
    );
}

#[tokio::test]
async fn an_edit_that_would_grow_a_file_past_what_undo_keeps_is_refused_before_it_writes() {
    let h = harness().await;
    rule(&h, "edit", "*", Decision::Allow);
    let original = "x".repeat(50_000);
    std::fs::write(h._dir.join("ws/big.txt"), &original).unwrap();
    let edit =
        json!({ "path": "big.txt", "old_string": "x", "new_string": "y".repeat(256), "replace_all": true }).to_string();
    h.provider
        .push(tool_call("read", r#"{"path": "big.txt"}"#))
        .push(tool_call("edit", &edit))
        .push(text("tried"));
    h.engine.submit(&h.session.id, prompt("expand it")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(
        std::fs::read_to_string(h._dir.join("ws/big.txt")).unwrap(),
        original,
        "nothing was written"
    );
    let messages = transcript(&h);
    let call = tool(&messages[2].parts[0]);
    assert_eq!(call.status, ToolStatus::Error);
    assert!(
        call.output.unwrap().contains("over the 10 MB undo can keep"),
        "{:?}",
        call.output
    );
}

#[tokio::test]
async fn a_command_that_only_reads_is_not_captured_and_one_that_writes_is() {
    let h = harness().await;
    rule(&h, "bash", "*", Decision::Allow);
    h.provider
        .push(tool_call("bash", &json!({ "command": "echo looking" }).to_string()))
        .push(tool_call("bash", &json!({ "command": "touch made.txt" }).to_string()))
        .push(text("done"));
    h.engine
        .submit(&h.session.id, prompt("look then write"))
        .await
        .await_ok();
    until_idle(&h).await;

    let recorded: Vec<_> = transcript(&h)
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|row| match &row.part {
            Part::ToolCall { metadata, .. } => {
                Some(metadata.as_ref().is_some_and(|metadata| metadata.changes.is_some()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        recorded,
        [false, true],
        "only the writing command carries a change record"
    );
    assert!(h._dir.join("ws/made.txt").exists());
}

#[tokio::test]
async fn whole_tree_calls_in_a_step_chain_their_captures_and_a_file_tool_write_breaks_the_chain() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy {
        rules: vec![
            Rule {
                kind: "bash".into(),
                pattern: "*".into(),
                decision: Decision::Allow,
            },
            Rule {
                kind: "edit".into(),
                pattern: "*".into(),
                decision: Decision::Allow,
            },
        ],
    });
    let call = |id, name, input: serde_json::Value| call_block(id, name, &input.to_string());
    h.provider
        .push(
            [
                call("t1", "bash", json!({ "command": "touch one.txt" })),
                call("t2", "bash", json!({ "command": "touch two.txt" })),
                call("t3", "write", json!({ "path": "three.txt", "content": "3\n" })),
                call("t4", "bash", json!({ "command": "touch four.txt" })),
                vec![Chunk::Stop(StopReason::ToolUse)],
            ]
            .concat(),
        )
        .push(text("done"));
    h.engine.submit(&h.session.id, prompt("make files")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    let changed: Vec<Vec<String>> = messages[1]
        .parts
        .iter()
        .filter_map(|row| match &row.part {
            Part::ToolCall {
                metadata: Some(metadata),
                ..
            } => Some(
                metadata
                    .changes
                    .iter()
                    .flatten()
                    .map(|change| change.path.clone())
                    .collect(),
            ),
            _ => None,
        })
        .collect();
    assert_eq!(
        changed,
        [vec!["one.txt"], vec!["two.txt"], vec!["three.txt"], vec!["four.txt"]],
        "each call records only its own change; the write in between is not taken for the next command's"
    );
}
