use super::*;

#[tokio::test]
async fn tool_calls_run_and_feed_the_next_request() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "alpha\n").unwrap();
    h.provider
        .push(tool_call("read", r#"{"path": "a.txt"}"#))
        .push(text("It says alpha"));
    h.engine
        .submit(&h.session.id, prompt("what is in a.txt"))
        .await
        .await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    assert_eq!(messages.len(), 3);
    let read = tool(&messages[1].parts[0]);
    assert_eq!(read.status, ToolStatus::Done);
    assert_eq!(read.output, Some("1: alpha"));
    assert_eq!(read.title, Some("a.txt"));
    assert_eq!(
        messages[2].parts[0].part,
        Part::Text {
            text: "It says alpha".into()
        }
    );
    let requests = h.provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        matches!(&requests[1].messages[2].blocks[0], llm::Block::ToolResult { content, .. } if content == "1: alpha")
    );
}

#[tokio::test]
async fn calls_keep_the_models_order_across_a_write() {
    let h = harness().await;
    rule(&h, "edit", "*", Decision::Allow);
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    h.provider
        .push(
            [
                call_block("t1", "read", r#"{"path": "a.txt"}"#),
                call_block("t2", "write", r#"{"path": "c.txt", "content": "c\n"}"#),
                call_block("t3", "read", r#"{"path": "c.txt"}"#),
                vec![Chunk::Stop(StopReason::ToolUse)],
            ]
            .concat(),
        )
        .push(text("done"));
    h.engine.submit(&h.session.id, prompt("go")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    let outputs: Vec<_> = messages[1]
        .parts
        .iter()
        .filter_map(|row| match &row.part {
            Part::ToolCall {
                name, status, output, ..
            } => Some((name.clone(), *status, output.clone().unwrap_or_default())),
            _ => None,
        })
        .collect();
    assert_eq!(outputs[0].0, "read");
    assert_eq!(
        outputs[2],
        ("read".into(), ToolStatus::Done, "1: c".into()),
        "a read issued after a write must see the write"
    );
}

#[tokio::test]
async fn a_call_id_the_provider_repeats_is_renamed_so_every_call_keeps_its_own() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    std::fs::write(h._dir.join("ws/b.txt"), "b\n").unwrap();
    let call = |path| {
        let mut chunks = call_block("functions.read:0", "read", &format!(r#"{{"path": "{path}"}}"#));
        chunks.push(Chunk::Stop(StopReason::ToolUse));
        chunks
    };
    h.provider.push(call("a.txt")).push(call("b.txt")).push(text("done"));
    h.engine.submit(&h.session.id, prompt("read both")).await.await_ok();
    until_idle(&h).await;

    let ids: Vec<_> = transcript(&h)
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|row| match &row.part {
            Part::ToolCall { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], "functions.read:0", "a fresh id is kept as the provider sent it");
    assert!(ids[1] != ids[0] && ids[1].starts_with("call_"), "{ids:?}");

    let last = h.provider.requests.lock().unwrap().last().unwrap().clone();
    let results: Vec<_> = last
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| match block {
            llm::Block::ToolResult { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        results,
        [ids[0].as_str(), ids[1].as_str()],
        "each result answers its own call"
    );
}
