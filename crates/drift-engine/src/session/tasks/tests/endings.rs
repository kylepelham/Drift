use super::*;

#[tokio::test]
async fn a_worker_cut_off_at_its_output_limit_is_incomplete_not_an_answer() {
    let h = harness().await;
    let cut_off = vec![
        Chunk::TextStart,
        Chunk::TextDelta("The first half of an ans".into()),
        Chunk::BlockStop,
        Chunk::Stop(crate::llm::StopReason::MaxTokens),
    ];
    h.provider
        .push_for(
            "PARENT",
            launches(&[json!({ "description": "Write it up", "prompt": "CHILD write a long report" })]),
        )
        .push_for("PARENT", text("it did not finish"))
        .push_for("CHILD write", cut_off);
    h.engine.submit(&h.session.id, prompt("PARENT report")).await.unwrap();
    until_idle(&h).await;

    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let call = tool(&transcript[1].parts[0]);
    assert_eq!(call.status, ToolStatus::Error, "not a successful task");
    assert_eq!(call.metadata.unwrap().outcome.as_deref(), Some("incomplete"));
    let output = call.output.unwrap();
    assert!(
        output.contains("not a complete answer") && output.contains("The first half of an ans"),
        "the partial text is kept: {output}"
    );
    assert_eq!(tasks(&h)[0].state, TaskState::Failed);
}

#[tokio::test]
async fn a_worker_whose_reply_the_provider_refused_says_so_not_that_it_was_cut_off() {
    let h = harness().await;
    let refused = vec![
        Chunk::TextStart,
        Chunk::TextDelta("I can".into()),
        Chunk::BlockStop,
        Chunk::Stop(crate::llm::StopReason::Refused),
    ];
    h.provider
        .push_for(
            "PARENT",
            launches(&[json!({ "description": "Write it up", "prompt": "CHILD write it" })]),
        )
        .push_for("PARENT", text("it was refused"))
        .push_for("CHILD write", refused);
    h.engine.submit(&h.session.id, prompt("PARENT report")).await.unwrap();
    until_idle(&h).await;

    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let call = tool(&transcript[1].parts[0]);
    assert_eq!(call.status, ToolStatus::Error);
    assert_eq!(call.metadata.unwrap().outcome.as_deref(), Some("refused"));
    let output = call.output.unwrap();
    assert!(
        output.contains("safety filter") && !output.contains("output limit"),
        "{output}"
    );
}
