use super::*;

/// A shell line that runs about two seconds, prints `marker`, then exits with `code`.
fn slow_exit(marker: &str, code: i32) -> String {
    match crate::tool::bash::Bash::detect().shell() {
        crate::tool::bash::Shell::Bash(_) => format!("sleep 2; echo {marker}; exit {code}"),
        crate::tool::bash::Shell::PowerShell(_) => format!("Start-Sleep 2; Write-Output {marker}; exit {code}"),
    }
}

fn start_in_background(h: &Harness, line: &str) {
    h.provider
        .push(tool_call(
            "bash",
            &json!({ "command": line, "background": true }).to_string(),
        ))
        .push(text("started it"));
}

/// Waits until the conversation has `count` messages and is idle.
async fn until_messages(h: &Harness, count: usize) -> Vec<MessageWithParts> {
    for _ in 0..200 {
        let messages = transcript(h);
        if messages.len() >= count && !h.engine.turns.is_running(&h.session.id) {
            return messages;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!(
        "the conversation never reached {count} messages: {:?}",
        transcript(h).len()
    );
}

#[tokio::test]
async fn a_process_that_exits_after_the_turn_wakes_its_conversation() {
    let h = harness().await;
    start_in_background(&h, &slow_exit("built", 1));
    h.provider.push(text("the build failed; fixing it"));

    h.engine.submit(&h.session.id, prompt("build it")).await.await_ok();
    let messages = until_messages(&h, 5).await;

    let woken = &messages[3];
    assert_eq!(woken.info.role, Role::User);
    let Part::ProcessExit { exit_code, output, .. } = &woken.parts[0].part else {
        panic!("{:?}", woken.parts);
    };
    assert_eq!(*exit_code, Some(1));
    assert!(output.contains("built"), "{output}");

    let requests = h.provider.requests.lock().unwrap();
    let last = requests.last().unwrap().messages.last().unwrap();
    assert!(
        last.blocks
            .iter()
            .any(|block| matches!(block, llm::Block::Text(text) if text.contains("exited with code 1"))),
        "{last:?}"
    );
}

#[tokio::test]
async fn a_process_the_model_stopped_or_a_stop_since_it_started_wakes_nothing() {
    let h = harness().await;
    start_in_background(&h, &slow_exit("never", 0));
    h.engine.submit(&h.session.id, prompt("start it")).await.await_ok();
    until_idle(&h).await;
    let process = h.engine.processes.of(&h.session.id).pop().unwrap();

    // The user pressed Stop after it started: its exit stays in the dock, not the conversation.
    h.engine.abort(&h.session.id);
    process
        .wait(
            None,
            Duration::from_secs(20),
            &tokio_util::sync::CancellationToken::new(),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(transcript(&h).len(), 3);
    assert_eq!(process.exit(), Some(crate::background::Exit::Code(0)));

    start_in_background(&h, &slow_exit("stopped", 0));
    h.engine.submit(&h.session.id, prompt("again")).await.await_ok();
    until_idle(&h).await;
    let second = h.engine.processes.of(&h.session.id).pop().unwrap();
    second.stop();
    second
        .wait(
            None,
            Duration::from_secs(20),
            &tokio_util::sync::CancellationToken::new(),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(transcript(&h).len(), 6, "a stopped process says nothing");
}

#[tokio::test]
async fn archiving_a_conversation_stops_its_processes() {
    let h = harness().await;
    let forever = match crate::tool::bash::Bash::detect().shell() {
        crate::tool::bash::Shell::Bash(_) => "sleep 60",
        crate::tool::bash::Shell::PowerShell(_) => "Start-Sleep 60",
    };
    start_in_background(&h, forever);
    h.engine.submit(&h.session.id, prompt("start it")).await.await_ok();
    until_idle(&h).await;
    let process = h.engine.processes.of(&h.session.id).pop().unwrap();

    h.engine.processes.stop_session(&h.session.id);
    process
        .wait(
            None,
            Duration::from_secs(20),
            &tokio_util::sync::CancellationToken::new(),
        )
        .await;

    assert_eq!(process.record().state, crate::background::ProcessState::Stopped);
}
