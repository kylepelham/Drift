use super::*;

fn too_long() -> llm::Error {
    llm::Error::api(400, "invalid_request_error", "prompt is too long: fixture overflow")
}

/// A child step that says something and then calls a tool, with usage past the compaction threshold.
fn progress_then(tool: &str, input: &str) -> Vec<Chunk> {
    let mut chunks = vec![
        Chunk::Usage(Usage {
            input: 980_000,
            ..Usage::default()
        }),
        Chunk::TextStart,
        Chunk::TextDelta("PROGRESS_TEXT".into()),
        Chunk::BlockStop,
    ];
    chunks.extend(call_block("toolu_progress", tool, input));
    chunks.push(Chunk::Stop(StopReason::ToolUse));
    chunks
}

/// Waits until the scripted queue is down to `left`, then stops only the parent's subagent.
async fn stop_the_child_when(h: &Harness, left: usize) -> String {
    for _ in 0..300 {
        if h.provider.responses_left() == left {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    let child = child_id(h);
    assert!(h.engine.abort(&child), "the child is running");
    child
}

#[tokio::test]
async fn a_subagent_that_fails_after_compacting_reports_the_failure_not_the_summary() {
    let h = harness().await;
    h.provider
        .push(tool_call("task", r#"{"description": "Doomed", "prompt": "go"}"#))
        .push_error(too_long())
        .push(text("SUBAGENT_FAIL_MARK_SUMMARY"))
        .push_error(too_long())
        .push(text("parent carries on"));
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    until_idle(&h).await;

    let (status, output, metadata) = task_call(&transcript(&h));
    assert_eq!(status, ToolStatus::Error);
    assert!(
        output.contains("prompt is too long") && !output.contains("SUBAGENT_FAIL_MARK_SUMMARY"),
        "{output}"
    );
    assert_eq!(metadata["outcome"], "failed");
    assert!(
        metadata["sessionId"].is_string(),
        "the card still opens the failed subagent"
    );
}

#[tokio::test]
async fn a_subagent_stopped_after_compacting_is_not_answered_by_its_summary() {
    let h = harness().await;
    h.provider
        .push(tool_call("task", r#"{"description": "Stalls", "prompt": "go"}"#))
        .push_error(too_long())
        .push(text("SUBAGENT_STOP_MARK_SUMMARY"))
        .push_stall();
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    for _ in 0..200 {
        if h.provider.responses_left() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(h.engine.abort(&h.session.id));
    until_idle(&h).await;

    let (status, output, _) = task_call(&transcript(&h));
    assert_eq!(status, ToolStatus::Error);
    assert!(!output.contains("SUBAGENT_STOP_MARK_SUMMARY"), "{output}");
}

#[tokio::test]
async fn stopping_only_the_subagent_while_it_compacts_reports_stopped_not_its_progress() {
    let h = harness().await;
    h.provider
        .push(tool_call("task", r#"{"description": "Compacting", "prompt": "go"}"#))
        .push(progress_then("glob", r#"{"pattern": "*.txt"}"#))
        .push_stall()
        .push(text("parent carries on"));
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    let child = stop_the_child_when(&h, 1).await;
    until_idle(&h).await;

    let messages = h.engine.store.transcript(&child).unwrap();
    let child_last = messages.last().unwrap();
    assert!(
        child_last.info.summary && child_last.info.status == MessageStatus::Aborted,
        "the stop landed in compaction"
    );
    let (status, output, metadata) = task_call(&transcript(&h));
    assert_eq!(status, ToolStatus::Error);
    assert_eq!(metadata["outcome"], "stopped");
    assert_eq!(
        metadata["sessionId"],
        child.as_str(),
        "the card still opens the stopped subagent"
    );
    assert!(!output.contains("PROGRESS_TEXT"), "{output}");
}

#[tokio::test]
async fn stopping_only_the_subagent_while_its_tool_runs_reports_stopped() {
    let h = harness().await;
    rule(&h, "bash", "*", Decision::Allow);
    let sleep = if cfg!(windows) {
        "ping -n 10 127.0.0.1"
    } else {
        "sleep 10"
    };
    let mut step = progress_then("bash", &json!({ "command": sleep }).to_string());
    step[0] = Chunk::Usage(Usage {
        input: 10,
        ..Usage::default()
    });
    h.provider
        .push(tool_call("task", r#"{"description": "Sleeps", "prompt": "go"}"#))
        .push(step)
        .push(text("parent carries on"));
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    stop_the_child_when(&h, 1).await;
    until_idle(&h).await;

    let (status, output, metadata) = task_call(&transcript(&h));
    assert_eq!(
        (status, metadata["outcome"].as_str()),
        (ToolStatus::Error, Some("stopped"))
    );
    assert!(!output.contains("PROGRESS_TEXT"), "{output}");
}
