use serde_json::json;

use super::*;
use crate::tool::bash::{Bash, Shell};
use crate::tool::tests::Sandbox;

/// Starts `bash` and `powershell` in the background with the shell this machine has; returns the process id.
async fn background(sandbox: &Sandbox, bash_line: &str, powershell_line: &str) -> String {
    let shell = Bash::detect();
    let line = match shell.shell() {
        Shell::Bash(_) => bash_line,
        Shell::PowerShell(_) => powershell_line,
    };
    let started = shell
        .run(&sandbox.ctx, json!({ "command": line, "background": true }))
        .await
        .unwrap();

    started.metadata.process_id.unwrap()
}

async fn call(sandbox: &Sandbox, input: Value) -> Result<Output, ToolError> {
    Process.run(&sandbox.ctx, input).await
}

#[tokio::test]
async fn wait_until_a_line_then_read_only_what_is_new_then_stop() {
    let sandbox = Sandbox::new("process-wait");
    let id = background(
        &sandbox,
        "sleep 3; echo 'ready at http://localhost:7000'; sleep 1; echo later; sleep 30",
        "Start-Sleep 3; Write-Output 'ready at http://localhost:7000'; Start-Sleep 1; Write-Output later; Start-Sleep 30",
    )
    .await;

    let ready = call(
        &sandbox,
        json!({ "action": "wait", "id": id, "until": "ready", "wait_seconds": 20 }),
    )
    .await
    .unwrap();
    assert!(ready.output.starts_with("A line matched."), "{}", ready.output);
    assert!(ready.output.contains("http://localhost:7000"), "{}", ready.output);

    call(
        &sandbox,
        json!({ "action": "wait", "id": id, "until": "later", "wait_seconds": 20 }),
    )
    .await
    .unwrap();
    let nothing = call(&sandbox, json!({ "action": "output", "id": id })).await.unwrap();
    assert!(nothing.output.contains("Nothing new printed."), "{}", nothing.output);

    let found = call(&sandbox, json!({ "action": "output", "id": id, "search": "^ready" }))
        .await
        .unwrap();
    assert!(found.output.contains("ready at"), "{}", found.output);

    let listed = call(&sandbox, json!({ "action": "list" })).await.unwrap();
    assert!(
        listed.output.contains(&id) && listed.output.contains("is running"),
        "{}",
        listed.output
    );

    let stopped = call(&sandbox, json!({ "action": "stop", "id": id })).await.unwrap();
    assert!(stopped.output.contains("was stopped"), "{}", stopped.output);
}

#[tokio::test]
async fn a_wait_for_the_end_reports_the_exit_code_and_claims_the_report() {
    let sandbox = Sandbox::new("process-end");
    let id = background(
        &sandbox,
        "sleep 2; echo done; exit 4",
        "Start-Sleep 2; Write-Output done; exit 4",
    )
    .await;

    let ended = call(&sandbox, json!({ "action": "wait", "id": id, "wait_seconds": 20 }))
        .await
        .unwrap();

    assert!(ended.output.contains("exited with code 4"), "{}", ended.output);
    assert!(ended.output.contains("done"), "{}", ended.output);
    let handle = sandbox.ctx.engine.processes.get(&id).unwrap();
    assert!(!handle.claim_report(), "the conversation is not told again");
}

#[tokio::test]
async fn another_conversations_process_is_not_found_and_typing_asks() {
    let sandbox = Sandbox::new("process-owner");
    let id = background(&sandbox, "sleep 30", "Start-Sleep 30").await;

    let mut other = sandbox.ctx_clone();
    other.session_id = "ses_other".into();
    let refused = Process.run(&other, json!({ "action": "output", "id": id })).await;
    assert!(refused.is_err());

    let ask = Process
        .ask(&sandbox.ctx, &json!({ "action": "input", "id": id, "text": "y\n" }))
        .unwrap();
    assert_eq!((ask.kind.as_str(), ask.pattern.as_str()), ("bash", "y"));
    assert!(Process.ask(&sandbox.ctx, &json!({ "action": "list" })).is_none());

    sandbox.ctx.engine.processes.stop_session(&sandbox.ctx.session_id);
}

#[test]
fn spans_read_as_a_person_would_say_them() {
    assert_eq!(elapsed(4_500), "4 s");
    assert_eq!(elapsed(125_000), "2 min 5 s");
    assert_eq!(elapsed(3_725_000), "1 h 2 min");
}

#[tokio::test]
async fn a_wait_ends_when_a_message_arrives_in_the_conversation() {
    let mut sandbox = Sandbox::new("process-arrival");
    let store = &sandbox.ctx.engine.store;
    let workspace = store
        .add_workspace(&sandbox.ctx.workspace.to_string_lossy(), "ws", "")
        .unwrap();
    let session = store
        .create_session(crate::store::NewSession {
            workspace_id: &workspace.id,
            parent_id: None,
            visibility: crate::session::types::Visibility::Sibling,
            title: "Arrival",
            agent: "build",
            model: None,
        })
        .unwrap();
    sandbox.ctx.session_id = session.id;
    let id = background(&sandbox, "sleep 30", "Start-Sleep 30").await;
    let engine = sandbox.ctx.engine.clone();
    let session = sandbox.ctx.session_id.clone();
    let arriving = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        let message = engine
            .store
            .create_message(&session, crate::session::types::Role::User, None)
            .unwrap();
        engine.hub.publish(crate::event::Event::MessageCreated { message });
    });

    let started = std::time::Instant::now();
    let waited = call(&sandbox, json!({ "action": "wait", "id": id, "wait_seconds": 60 }))
        .await
        .unwrap();
    arriving.await.unwrap();
    // Stopped before the asserts, so a failed one never leaves the sleep running into later tests.
    sandbox.ctx.engine.processes.stop_session(&sandbox.ctx.session_id);

    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert!(waited.output.starts_with("A new message arrived"), "{}", waited.output);
}

#[tokio::test]
async fn a_process_started_before_a_stop_is_not_waited_for_since_its_end_will_not_wake_the_session() {
    let mut sandbox = Sandbox::new("process-stopped-wait");
    let store = &sandbox.ctx.engine.store;
    let workspace = store
        .add_workspace(&sandbox.ctx.workspace.to_string_lossy(), "ws", "")
        .unwrap();
    let session = store
        .create_session(crate::store::NewSession {
            workspace_id: &workspace.id,
            parent_id: None,
            visibility: crate::session::types::Visibility::Sibling,
            title: "Stopped",
            agent: "orchestrator",
            model: None,
        })
        .unwrap();
    sandbox.ctx.session_id = session.id.clone();
    background(&sandbox, "sleep 30", "Start-Sleep 30").await;
    let engine = &sandbox.ctx.engine;

    let before = engine.background_running(&session.id);
    engine.abort(&session.id);
    let after = engine.background_running(&session.id);
    engine.processes.stop_session(&session.id);

    assert!(before, "running and able to wake the session");
    assert!(!after, "after Stop its end is not told, so nothing should wait on it");
}
