use super::*;

#[tokio::test]
async fn a_tool_named_in_the_wrong_case_runs_as_the_offered_tool() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "alpha\n").unwrap();
    h.provider
        .push(tool_call("Read", r#"{"path": "a.txt"}"#))
        .push(text("ok"));
    h.engine.submit(&h.session.id, prompt("read")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    let call = tool(&messages[1].parts[0]);
    assert_eq!(
        (call.name, call.status),
        ("read", ToolStatus::Done),
        "{:?}",
        call.output
    );
}

#[tokio::test]
async fn malformed_call_arguments_and_max_tokens_stop_dispatch() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    let mut malformed = call_block("t1", "read", r#"{"path": "a.tx"#);
    malformed.push(Chunk::Stop(StopReason::ToolUse));
    h.provider.push(malformed).push(text("ok"));
    h.engine.submit(&h.session.id, prompt("read")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    let call = tool(&messages[1].parts[0]);
    assert_eq!(call.status, ToolStatus::Error);
    assert!(
        call.output.unwrap().contains("not valid JSON (EOF while parsing"),
        "the parser's complaint is quoted: {:?}",
        call.output
    );
    let next = h.provider.requests.lock().unwrap()[1].clone();
    let replayed: Vec<_> = next
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter(|block| matches!(block, llm::Block::ToolUse { .. } | llm::Block::ToolResult { .. }))
        .collect();
    assert!(
        matches!(replayed[..], [llm::Block::ToolUse { input, .. }, llm::Block::ToolResult { is_error: true, .. }]
        if *input == json!({})),
        "the model sees its broken call and why: {replayed:?}"
    );

    let mut cut = call_block("t2", "read", r#"{"path": "a.txt"}"#);
    cut.push(Chunk::Stop(StopReason::MaxTokens));
    h.provider.push(cut);
    h.engine.submit(&h.session.id, prompt("again")).await.await_ok();
    until_idle(&h).await;
    let messages = transcript(&h);
    let last = messages.last().unwrap();
    let call = tool(&last.parts[0]);
    assert_eq!(
        call.status,
        ToolStatus::Error,
        "a max_tokens stop dispatches nothing, and says so"
    );
    assert!(
        call.output
            .unwrap()
            .starts_with("Not run: the reply hit its output limit"),
        "{:?}",
        call.output
    );
    assert_eq!(last.info.status, MessageStatus::Done);
    assert!(
        last.info.error.as_deref().unwrap().starts_with(OUTPUT_LIMIT_ENDING),
        "the ending is visible: {:?}",
        last.info.error
    );
}

#[tokio::test]
async fn arguments_that_do_not_fit_the_schema_are_refused_before_the_call_runs() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "a\n").unwrap();
    h.provider
        .push(tool_call("read", r#"{"path": "a.txt", "limit": "20"}"#))
        .push(text("ok"));
    h.engine.submit(&h.session.id, prompt("read")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    let call = tool(&messages[1].parts[0]);
    assert_eq!(call.status, ToolStatus::Error);
    assert!(
        call.output.unwrap().contains("`limit` must be integer, not a string"),
        "{:?}",
        call.output
    );
}

#[tokio::test]
async fn any_tool_result_past_the_bound_is_cut_to_its_ends_with_the_whole_on_disk() {
    let h = harness().await;
    let skill = h._dir.join("ws/.drift/skills/huge");
    std::fs::create_dir_all(&skill).unwrap();
    let body = format!("FIRST\n{}\nLAST", "guidance line\n".repeat(20_000));
    std::fs::write(skill.join("SKILL.md"), format!("---\ndescription: Huge\n---\n{body}")).unwrap();
    h.provider
        .push(tool_call("skill", r#"{"name": "huge"}"#))
        .push(text("read it"));
    h.engine.submit(&h.session.id, prompt("use the skill")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    let call = tool(&messages[1].parts[0]);
    let output = call.output.unwrap();
    assert!(
        output.len() <= crate::tool::spool::MAX_RESULT_BYTES
            && output.contains("FIRST")
            && output.trim_end().ends_with("LAST"),
        "{}",
        output.len()
    );
    let file = call
        .metadata
        .unwrap()
        .result_file
        .as_deref()
        .expect("the whole result is kept");
    assert!(std::fs::read_to_string(file).unwrap().contains(&body));
    let sent = h.provider.requests.lock().unwrap()[1].clone();
    let result = sent
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .find_map(|block| match block {
            llm::Block::ToolResult { content, .. } => Some(content.len()),
            _ => None,
        })
        .unwrap();
    assert!(
        result <= crate::tool::spool::MAX_RESULT_BYTES,
        "the model got the bounded text"
    );
}

#[tokio::test]
async fn a_call_to_a_tool_the_run_did_not_offer_is_refused_before_anything_happens() {
    let h = harness().await;
    std::fs::create_dir_all(h._dir.join("ws/.drift/agents")).unwrap();
    std::fs::write(
        h._dir.join("ws/.drift/agents/reader.md"),
        "---\ndescription: Reads\ntools: read\n---\nRead only.",
    )
    .unwrap();
    h.engine
        .store
        .update_session(&h.session.id, None, None, Some("reader"))
        .unwrap();
    h.provider
        .push(tool_call("write", r#"{"path": "mutated.txt", "content": "x\n"}"#))
        .push(text("noted"));
    h.engine
        .submit(&h.session.id, prompt("write it anyway"))
        .await
        .await_ok();
    until_idle(&h).await;

    assert!(!h._dir.join("ws/mutated.txt").exists());
    let messages = transcript(&h);
    let call = tool(&messages[1].parts[0]);
    assert_eq!(call.status, ToolStatus::Error);
    assert!(call.output.unwrap().contains("not available in this session"));
    assert!(
        call.metadata.is_none(),
        "nothing was recorded for a call that never ran"
    );
}
