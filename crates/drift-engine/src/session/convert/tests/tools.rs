use super::*;

#[test]
fn a_call_id_stored_twice_by_an_older_session_goes_out_unique_with_its_result() {
    let step = || message(Role::Assistant, vec![call(ToolStatus::Done, Some("ok"))]);
    let transcript = [
        message(Role::User, vec![Part::Text { text: "q".into() }]),
        step(),
        step(),
        step(),
    ];
    let sent = messages(&transcript, &target());
    let flat: Vec<_> = sent.iter().flat_map(|message| &message.blocks).collect();
    let uses: Vec<_> = flat
        .iter()
        .filter_map(|block| match block {
            Block::ToolUse { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    let answers: Vec<_> = flat
        .iter()
        .filter_map(|block| match block {
            Block::ToolResult { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(uses, ["c1", "c1_2", "c1_3"]);
    assert_eq!(answers, uses, "each result answers its own renamed call");
}

#[test]
fn calls_repeating_one_id_in_a_step_each_keep_their_own_result() {
    let calls = ["first", "second", "third"].map(|output| call(ToolStatus::Done, Some(output)));
    let transcript = [
        message(Role::User, vec![Part::Text { text: "q".into() }]),
        message(Role::Assistant, calls.to_vec()),
    ];
    let sent = messages(&transcript, &target());
    let answers: Vec<(&str, &str)> = sent
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| match block {
            Block::ToolResult { call_id, content, .. } => Some((call_id.as_str(), content.as_str())),
            _ => None,
        })
        .collect();

    assert_eq!(answers.len(), 3);
    for ((id, content), (want_id, want)) in answers
        .iter()
        .zip([("c1", "first"), ("c1_2", "second"), ("c1_3", "third")])
    {
        assert_eq!(*id, want_id);
        assert!(content.contains(want), "{id} answered with {content}");
    }
}

#[test]
fn tool_calls_get_results_in_the_following_user_turn() {
    let transcript = vec![
        message(Role::User, vec![Part::Text { text: "read a".into() }]),
        message(
            Role::Assistant,
            vec![Part::Text { text: "ok".into() }, call(ToolStatus::Done, Some("1: x"))],
        ),
        message(Role::Assistant, vec![Part::Text { text: "done".into() }]),
    ];
    let output = messages(&transcript, &target());

    assert_eq!(output.len(), 4);
    assert_eq!(output[1].role, LlmRole::Assistant);
    assert!(matches!(&output[1].blocks[1], Block::ToolUse { id, .. } if id == "c1"));
    assert_eq!(
        output[2].blocks,
        vec![Block::ToolResult {
            call_id: "c1".into(),
            content: "1: x".into(),
            is_error: false
        }]
    );
    assert_eq!(output[3].blocks, vec![Block::Text("done".into())]);
}

#[test]
fn returned_images_follow_every_result_of_the_turn() {
    let mut with_image = call(ToolStatus::Done, Some("an image"));
    if let Part::ToolCall { metadata, .. } = &mut with_image {
        *metadata = Some(Box::new(
            json!({ "images": [{ "mime": "image/png", "hash": "abc" }] }).into(),
        ));
    }
    let mut second = call(ToolStatus::Done, Some("text"));
    if let Part::ToolCall { call_id, .. } = &mut second {
        *call_id = "c2".into();
    }
    let output = messages(&[message(Role::Assistant, vec![with_image, second])], &target());
    let kinds: Vec<_> = output[1]
        .blocks
        .iter()
        .map(|block| match block {
            Block::ToolResult { .. } => "result",
            Block::Text(_) => "text",
            Block::Stored { .. } => "image",
            _ => "other",
        })
        .collect();

    assert_eq!(kinds, ["result", "result", "text", "image"]);
}

#[test]
fn unfinished_and_denied_calls_still_produce_results() {
    let transcript = vec![
        message(Role::Assistant, vec![call(ToolStatus::Running, None)]),
        message(Role::Assistant, vec![call(ToolStatus::Denied, None)]),
    ];
    let output = messages(&transcript, &target());
    let Block::ToolResult { is_error, content, .. } = &output[1].blocks[0] else {
        panic!()
    };

    assert!(*is_error && content.contains("interrupted"));
    assert!(matches!(&output[3].blocks[0], Block::ToolResult { content, .. } if content.contains("denied")));
}

#[test]
fn an_instruction_update_saved_between_steps_follows_that_steps_tool_results() {
    let update = Part::InstructionUpdate {
        generation: "base_1".into(),
        changes: vec![crate::session::types::InstructionChange {
            key: "file:AGENTS.md".into(),
            title: "AGENTS.md".into(),
            text: Some("# Instructions from AGENTS.md\n\nBe thorough.".into()),
        }],
    };
    let transcript = [
        message(Role::User, vec![Part::Text { text: "go".into() }]),
        message(Role::Assistant, vec![call(ToolStatus::Done, Some("ran")), update]),
    ];

    let sent = messages(&transcript, &target());
    let results = &sent.last().unwrap().blocks;
    assert!(matches!(results[0], Block::ToolResult { .. }), "{results:?}");
    assert!(
        matches!(&results[1], Block::Reminder(text) if text.contains("Be thorough.") && text.starts_with("<system-reminder>")),
        "{results:?}"
    );
}
