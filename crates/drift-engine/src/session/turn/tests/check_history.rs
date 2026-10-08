use super::*;

#[tokio::test]
async fn undo_puts_back_what_a_fixing_check_rewrote_and_forgets_what_checks_said() {
    let h = harness().await;
    allow_edits_and_project_commands(&h);
    let fix = if cfg!(windows) {
        ["cmd", "/c", "echo fixed> $FILE"]
    } else {
        ["sh", "-c", "echo fixed > $FILE"]
    };
    std::fs::write(
        h._dir.join("ws/drift.json"),
        json!({ "checks": { "fixer": { "command": fix, "extensions": [".md"] } } }).to_string(),
    )
    .unwrap();
    std::fs::write(h._dir.join("ws/notes.md"), "original\n").unwrap();
    h.provider
        .push(tool_call("read", r#"{"path": "notes.md"}"#))
        .push(text("read it"));
    h.engine.submit(&h.session.id, prompt("read notes")).await.await_ok();
    until_idle(&h).await;
    h.provider
        .push(two_writes(("notes.md", "draft\n"), ("new.md", "new\n")))
        .push(text("written"));
    h.engine.submit(&h.session.id, prompt("write notes")).await.await_ok();
    until_idle(&h).await;

    assert!(
        std::fs::read_to_string(h._dir.join("ws/notes.md"))
            .unwrap()
            .starts_with("fixed")
    );
    assert!(
        std::fs::read_to_string(h._dir.join("ws/new.md"))
            .unwrap()
            .starts_with("fixed")
    );
    h.engine.turns.repeated(&h.session.id, "fixer", Some("seen"));
    let messages = transcript(&h);
    let prompt_id = messages
        .iter()
        .filter(|message| message.info.role == Role::User)
        .nth(1)
        .unwrap()
        .info
        .id
        .clone();
    let undone = h.engine.revert(&h.session.id, &prompt_id).await.unwrap();
    assert!(
        undone.kept.is_empty(),
        "the check's rewrite is the session's own, so nothing is kept: {:?}",
        undone.kept
    );
    assert_eq!(
        std::fs::read_to_string(h._dir.join("ws/notes.md")).unwrap(),
        "original\n"
    );
    assert!(
        !h._dir.join("ws/new.md").exists(),
        "a file the step created and a check rewrote is gone again"
    );
    assert!(
        !h.engine.turns.repeated(&h.session.id, "fixer", Some("seen")),
        "undone, what checks said is forgotten"
    );
}

#[tokio::test]
async fn a_change_no_check_covers_is_left_to_whoever_made_it_and_undo_keeps_it() {
    let h = harness().await;
    allow_edits_and_project_commands(&h);
    let other = h._dir.join("ws/b.txt");
    let fix = if cfg!(windows) {
        [
            "cmd".to_string(),
            "/c".to_string(),
            format!("echo fixed> $FILE & echo user edit> {}", other.display()),
        ]
    } else {
        [
            "sh".to_string(),
            "-c".to_string(),
            format!("echo fixed > $FILE; echo user edit > '{}'", other.display()),
        ]
    };
    std::fs::write(
        h._dir.join("ws/drift.json"),
        json!({ "checks": { "fixer": { "command": fix, "extensions": [".md"] } } }).to_string(),
    )
    .unwrap();
    h.provider
        .push(two_writes(("a.md", "draft\n"), ("b.txt", "bee\n")))
        .push(text("written"));
    h.engine.submit(&h.session.id, prompt("write both")).await.await_ok();
    until_idle(&h).await;

    let calls = call_outputs(&h, 1);
    assert_eq!(
        calls[1].1["checkChanged"],
        json!(["a.md"]),
        "only the file a check covers is the check's: {:?}",
        calls[1].1
    );
    h.engine
        .revert(&h.session.id, &transcript(&h)[0].info.id)
        .await
        .unwrap();
    assert!(
        !h._dir.join("ws/a.md").exists(),
        "the check's rewrite is undone with the write"
    );
    assert!(
        std::fs::read_to_string(&other).unwrap().starts_with("user edit"),
        "someone else's edit is never undone as the session's"
    );
}

#[tokio::test]
async fn a_stop_while_a_fixer_runs_still_records_what_it_rewrote() {
    let h = harness().await;
    allow_edits_and_project_commands(&h);
    let fix = if cfg!(windows) {
        ["cmd", "/c", "echo fixed> $FILE & ping -n 30 127.0.0.1 > nul"]
    } else {
        ["sh", "-c", "echo fixed > $FILE; sleep 30"]
    };
    std::fs::write(
        h._dir.join("ws/drift.json"),
        json!({ "checks": { "fixer": { "command": fix, "extensions": [".md"] } } }).to_string(),
    )
    .unwrap();
    h.provider
        .push(tool_call("write", r#"{"path": "a.md", "content": "draft\n"}"#))
        .push(text("written"));
    h.engine.submit(&h.session.id, prompt("write a")).await.await_ok();
    for _ in 0..500 {
        if std::fs::read_to_string(h._dir.join("ws/a.md")).is_ok_and(|text| text.starts_with("fixed")) {
            break;
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    h.engine.abort(&h.session.id);
    until_idle(&h).await;

    let calls = call_outputs(&h, 1);
    assert_eq!(calls[0].1["checkChanged"], json!(["a.md"]), "{:?}", calls[0].1);
    h.engine
        .revert(&h.session.id, &transcript(&h)[0].info.id)
        .await
        .unwrap();
    assert!(
        !h._dir.join("ws/a.md").exists(),
        "undo puts back the fixer's rewrite too"
    );
}

#[tokio::test]
async fn a_whole_workspace_fixer_is_captured_whole_and_what_it_changed_elsewhere_is_said_and_left_to_stand() {
    let h = harness().await;
    allow_edits_and_project_commands(&h);
    let fix = if cfg!(windows) {
        ["cmd", "/c", "echo fixed> a.md & echo fixed> other.md"]
    } else {
        ["sh", "-c", "echo fixed > a.md; echo fixed > other.md"]
    };
    std::fs::write(
        h._dir.join("ws/drift.json"),
        json!({ "checks": { "fix-all": { "command": fix, "extensions": [".md"] } } }).to_string(),
    )
    .unwrap();
    std::fs::write(h._dir.join("ws/other.md"), "untouched\n").unwrap();
    h.provider
        .push(tool_call("write", r#"{"path": "a.md", "content": "draft\n"}"#))
        .push(text("written"));
    h.engine.submit(&h.session.id, prompt("write a")).await.await_ok();
    until_idle(&h).await;

    let calls = call_outputs(&h, 1);
    let (output, metadata) = &calls[0];
    assert_eq!(
        (&metadata["checkChanged"], &metadata["checkObserved"]),
        (&json!(["a.md"]), &json!(["other.md"])),
        "{metadata:?}"
    );
    assert!(
        output.contains("files this step did not write changed too (other.md)"),
        "{output}"
    );
    h.engine
        .revert(&h.session.id, &transcript(&h)[0].info.id)
        .await
        .unwrap();
    assert!(
        !h._dir.join("ws/a.md").exists(),
        "the step's own file goes back, check's rewrite and all"
    );
    assert!(
        std::fs::read_to_string(h._dir.join("ws/other.md"))
            .unwrap()
            .starts_with("fixed"),
        "a file the step never wrote is not the session's to undo"
    );
}

#[tokio::test]
async fn a_check_rewrite_without_a_capture_is_still_announced_and_said_to_be_unrecorded() {
    let h = harness().await;
    let allow = |kind: &str| Rule {
        kind: kind.into(),
        pattern: "*".into(),
        decision: Decision::Allow,
    };
    h.engine.permissions.set_policy(Policy {
        rules: vec![allow("edit"), allow("bash"), allow("project-commands")],
    });
    let fix = if cfg!(windows) {
        ["cmd", "/c", "echo fixed> $FILE"]
    } else {
        ["sh", "-c", "echo fixed > $FILE"]
    };
    std::fs::write(
        h._dir.join("ws/drift.json"),
        json!({ "checks": { "fixer": { "command": fix, "extensions": [".md"] } } }).to_string(),
    )
    .unwrap();
    let store = h._dir.join("data/snapshots").to_string_lossy().replace('\\', "/");
    let breaks = match crate::tool::bash::Bash::detect().dialect() {
        crate::tool::command::Dialect::Bash => format!("rm -rf '{store}' && printf x > '{store}'"),
        crate::tool::command::Dialect::PowerShell => {
            format!("Remove-Item -Recurse -Force '{store}'; Set-Content -Path '{store}' -Value x")
        }
    };
    let calls = [
        call_block(
            "toolu_write",
            "write",
            &json!({ "path": "new.md", "content": "draft\n" }).to_string(),
        ),
        call_block("toolu_bash", "bash", &json!({ "command": breaks }).to_string()),
        vec![Chunk::Stop(StopReason::ToolUse)],
    ];
    let mut events = h.engine.hub.attach(None).rx;
    h.provider.push(calls.concat()).push(text("done"));
    h.engine
        .submit(&h.session.id, prompt("write and break"))
        .await
        .await_ok();
    let ask = next_ask(&mut events).await;
    reply_permission(&h, &ask.id, Reply::Once);
    until_idle(&h).await;

    assert!(
        std::fs::read_to_string(h._dir.join("ws/new.md"))
            .unwrap()
            .starts_with("fixed")
    );
    let calls = call_outputs(&h, 1);
    let write = &calls[0];
    assert!(
        write.0.contains("A check then changed new.md")
            && write.0.contains("undo cannot put back what the checks rewrote"),
        "{}",
        write.0
    );
    assert_eq!(write.1["unrecorded"], json!(["new.md"]));
}

#[tokio::test]
async fn compaction_forgets_what_checks_said() {
    let h = harness().await;
    h.provider.push(text("hello")).push(text("the summary"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_idle(&h).await;

    h.engine.turns.repeated(&h.session.id, "types", Some("3 errors"));

    h.engine
        .compact(
            &h.session.id,
            crate::session::compaction::Trigger::Manual,
            &Default::default(),
        )
        .await
        .unwrap();

    assert!(
        !h.engine.turns.repeated(&h.session.id, "types", Some("3 errors")),
        "the summary may not hold the full report, so it is sent again"
    );
}
