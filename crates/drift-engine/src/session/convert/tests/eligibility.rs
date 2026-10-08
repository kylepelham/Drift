use super::*;

#[test]
fn failed_and_streaming_attempts_are_left_out_but_aborted_partials_stay() {
    let transcript = vec![
        message_with(Role::User, MessageStatus::Done, vec![Part::Text { text: "q".into() }]),
        message_with(
            Role::Assistant,
            MessageStatus::Error,
            vec![Part::Text { text: "half".into() }],
        ),
        message_with(
            Role::Assistant,
            MessageStatus::Streaming,
            vec![Part::Text { text: "ghost".into() }],
        ),
        message_with(
            Role::Assistant,
            MessageStatus::Aborted,
            vec![Part::Text { text: "partial".into() }],
        ),
        message_with(
            Role::Assistant,
            MessageStatus::Done,
            vec![Part::Text { text: "final".into() }],
        ),
    ];
    let output = messages(&transcript, &target());
    let texts: Vec<_> = output
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| match block {
            Block::Text(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["q", "partial", "final"]);
}

#[test]
fn aborted_rows_replay_only_their_valid_blocks() {
    let broken = Part::ToolCall {
        call_id: "c_bad".into(),
        name: "read".into(),
        input: serde_json::Value::String("{\"path".into()),
        status: ToolStatus::Pending,
        title: None,
        output: None,
        metadata: None,
        started_at: None,
        finished_at: None,
    };
    let transcript = vec![message_with(
        Role::Assistant,
        MessageStatus::Aborted,
        vec![
            Part::Reasoning {
                text: "cut off".into(),
                signature: None,
                redacted: None,
            },
            Part::Text { text: "partial".into() },
            broken,
        ],
    )];
    let output = messages(&transcript, &target());
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].blocks, vec![Block::Text("partial".into())]);
}

#[test]
fn a_settled_call_with_broken_arguments_goes_back_with_its_parse_error() {
    let refused = Part::ToolCall {
        call_id: "c_bad".into(),
        name: "read".into(),
        input: serde_json::Value::String("{\"path".into()),
        status: ToolStatus::Error,
        title: None,
        output: Some("The arguments were not valid JSON (EOF while parsing)".into()),
        metadata: None,
        started_at: None,
        finished_at: None,
    };
    let output = messages(
        &[message_with(Role::Assistant, MessageStatus::Done, vec![refused])],
        &target(),
    );
    assert_eq!(
        output[0].blocks,
        vec![Block::ToolUse {
            id: "c_bad".into(),
            name: "read".into(),
            input: json!({})
        }]
    );
    assert!(
        matches!(&output[1].blocks[0], Block::ToolResult { call_id, content, is_error: true } if call_id == "c_bad" && content.contains("not valid JSON"))
    );
}
