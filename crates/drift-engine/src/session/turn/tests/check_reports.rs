use super::*;

#[tokio::test]
async fn checks_run_once_per_step_say_unchanged_problems_briefly_and_announce_files_they_change() {
    let h = harness().await;
    allow_edits_and_project_commands(&h);
    let log = h._dir.join("whole.log");
    let (shell, flag, whole, fix) = if cfg!(windows) {
        (
            "cmd",
            "/c",
            format!(
                "echo ran>> {} & echo 3 type errors in the workspace & exit 1",
                log.display()
            ),
            "echo fixed> $FILE",
        )
    } else {
        (
            "sh",
            "-c",
            format!(
                "echo ran >> '{}'; echo 3 type errors in the workspace; exit 1",
                log.display()
            ),
            "echo fixed > $FILE",
        )
    };
    let config = json!({ "checks": {
        "types": { "command": [shell, flag, whole], "extensions": [".ts"] },
        "fixer": { "command": [shell, flag, fix], "extensions": [".md"] },
    } });
    std::fs::write(h._dir.join("ws/drift.json"), config.to_string()).unwrap();
    h.provider
        .push(two_writes(("a.ts", "let a = 1\n"), ("b.ts", "let b = 2\n")))
        .push(text("written"));
    h.engine.submit(&h.session.id, prompt("write two")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(
        std::fs::read_to_string(&log).unwrap().lines().count(),
        1,
        "one run for the step, not one per write"
    );
    let calls = call_outputs(&h, 1);
    assert!(
        !calls[0].0.contains("Checks reported"),
        "the step's report goes on its last write: {:?}",
        calls[0].0
    );
    assert!(
        calls[1].0.contains("[types]\n3 type errors in the workspace"),
        "{:?}",
        calls[1].0
    );
    assert_eq!(calls[1].1["checks"][0]["status"], "problems");

    h.provider
        .push(tool_call("write", r#"{"path": "c.ts", "content": "let c = 3\n"}"#))
        .push(text("again"));
    h.engine
        .submit(&h.session.id, prompt("write one more"))
        .await
        .await_ok();
    until_idle(&h).await;
    let again = call_outputs(&h, transcript(&h).len() - 2);
    assert!(
        again[0].0.contains("[types] the same problems as reported before") && !again[0].0.contains("3 type errors"),
        "{:?}",
        again[0].0
    );

    h.provider
        .push(tool_call("write", r#"{"path": "notes.md", "content": "draft\n"}"#))
        .push(text("noted"));
    h.engine.submit(&h.session.id, prompt("write notes")).await.await_ok();
    until_idle(&h).await;
    let fixed = call_outputs(&h, transcript(&h).len() - 2);
    assert!(
        std::fs::read_to_string(h._dir.join("ws/notes.md"))
            .unwrap()
            .starts_with("fixed")
    );
    assert!(fixed[0].0.contains("A check then changed notes.md"), "{:?}", fixed[0].0);
    assert_eq!(fixed[0].1["checkChanged"][0], "notes.md");
}

#[tokio::test]
async fn configured_checks_report_problems_with_the_write_and_stop_cuts_them_off() {
    let h = harness().await;
    allow_edits_and_project_commands(&h);
    let (shell, flag, fail, hang) = if cfg!(windows) {
        (
            "cmd",
            "/c",
            "echo unused import in $FILE&& exit 1",
            "ping -n 30 127.0.0.1",
        )
    } else {
        ("sh", "-c", "echo unused import in $FILE; exit 1", "sleep 30")
    };
    let config = json!({ "checks": {
        "lint": { "command": [shell, flag, fail], "extensions": [".ts"] },
        "slow": { "command": [shell, flag, hang], "extensions": [".rs"] },
    } });
    std::fs::write(h._dir.join("ws/drift.json"), config.to_string()).unwrap();
    h.provider
        .push(tool_call("write", r#"{"path": "a.ts", "content": "import x\n"}"#))
        .push(text("written"));
    h.engine.submit(&h.session.id, prompt("write")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    let call = tool(&messages[1].parts[0]);
    assert_eq!(call.status, ToolStatus::Done, "the write itself succeeded");
    let output = call.output.unwrap();
    assert!(
        output.contains("Checks reported problems after this step's changes")
            && output.contains("[lint: a.ts]")
            && output.contains("unused import in"),
        "{output}"
    );
    assert_eq!(
        call.metadata.unwrap().checks.as_ref().unwrap()[0].status,
        CheckStatus::Problems
    );

    h.provider
        .push(tool_call("write", r#"{"path": "b.rs", "content": "fn main() {}\n"}"#));
    let started = std::time::Instant::now();
    h.engine.submit(&h.session.id, prompt("write rust")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(h.engine.abort(&h.session.id));
    until_idle(&h).await;
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "Stop does not wait out a slow check"
    );
    assert!(h._dir.join("ws/b.rs").exists());
}
