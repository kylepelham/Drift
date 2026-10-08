use super::*;

#[test]
fn an_empty_signed_text_part_goes_only_to_the_model_that_signed_it() {
    let mut reply = message(
        Role::Assistant,
        vec![Part::Text { text: "answer".into() }, Part::Text { text: String::new() }],
    );
    reply.parts[1].provider_signature = Some("sig".into());
    let transcript = [message(Role::User, vec![Part::Text { text: "q".into() }]), reply];

    let same = messages(&transcript, &target());
    let [Block::Text(_), Block::Signed { part, .. }] = &same[1].blocks[..] else {
        panic!("the signed empty text is replayed to the model that wrote it");
    };
    assert!(matches!(part.as_ref(), Block::Text(text) if text.is_empty()));

    let other = messages(
        &transcript,
        &ModelRef {
            provider: "openai".into(),
            model: "gpt".into(),
        },
    );
    assert_eq!(
        other[1].blocks,
        [Block::Text("answer".into())],
        "no empty text reaches a model that refuses it"
    );
}

#[test]
fn signed_reasoning_goes_back_signed_only_to_the_model_that_made_it_and_as_text_to_another() {
    let signed = Part::Reasoning {
        text: "hm".into(),
        signature: Some("sig".into()),
        redacted: None,
    };
    let transcript = vec![
        message(Role::User, vec![Part::Text { text: "a".into() }]),
        message(Role::Assistant, vec![signed, Part::Text { text: "x".into() }]),
    ];
    assert!(matches!(
        &messages(&transcript, &target())[1].blocks[0],
        Block::Reasoning { signature: Some(_), .. }
    ));
    let other = ModelRef {
        provider: "openai".into(),
        model: "gpt-5".into(),
    };
    assert_eq!(
        messages(&transcript, &other)[1].blocks,
        vec![Block::Text("hm".into()), Block::Text("x".into())],
        "read, not checked"
    );

    let cut_off = vec![message_with(
        Role::Assistant,
        MessageStatus::Aborted,
        vec![Part::Reasoning {
            text: "whole".into(),
            signature: Some("sig".into()),
            redacted: None,
        }],
    )];
    assert_eq!(
        messages(&cut_off, &other)[0].blocks,
        vec![Block::Text("whole".into())],
        "a signed thought was whole before the reply was cut"
    );
    let hidden = vec![message(
        Role::Assistant,
        vec![
            Part::Reasoning {
                text: String::new(),
                signature: None,
                redacted: Some("opaque".into()),
            },
            Part::Text { text: "x".into() },
        ],
    )];
    assert_eq!(
        messages(&hidden, &other)[0].blocks,
        vec![Block::Text("x".into())],
        "a redacted thought has nothing to read"
    );
}

#[test]
fn a_mode_and_its_base_take_each_others_signed_reasoning() {
    let catalog = Catalog::bundled();
    let base = ModelRef {
        provider: "anthropic".into(),
        model: "claude-opus-5-5".into(),
    };
    let fast = ModelRef {
        provider: "anthropic".into(),
        model: "claude-opus-5-5-fast".into(),
    };
    let mut reply = message(
        Role::Assistant,
        vec![Part::Reasoning {
            text: "hm".into(),
            signature: Some("sig".into()),
            redacted: None,
        }],
    );
    reply.info.model = Some(fast);
    let mut output = Vec::new();
    append(
        &mut output,
        [&reply],
        &OnCatalog {
            model: &base,
            catalog: &catalog,
        },
    );
    assert!(
        matches!(&output[0].blocks[0], Block::Reasoning { signature: Some(_), .. }),
        "the same model, run fast"
    );

    let mut sibling = Vec::new();
    append(
        &mut sibling,
        [&reply],
        &OnCatalog {
            model: &ModelRef {
                provider: "anthropic".into(),
                model: "claude-opus-5".into(),
            },
            catalog: &catalog,
        },
    );
    assert_eq!(
        sibling[0].blocks,
        vec![Block::Text("hm".into())],
        "another model in the family reads it as text"
    );
}

#[test]
fn unsigned_reasoning_from_turns_already_over_is_dropped_wherever_the_new_prompt_lands() {
    let thought = |id: &str| {
        let mut reply = message(
            Role::Assistant,
            vec![
                Part::Reasoning {
                    text: format!("thought {id}"),
                    signature: None,
                    redacted: None,
                },
                Part::Reasoning {
                    text: "signed".into(),
                    signature: Some("s".into()),
                    redacted: None,
                },
            ],
        );
        reply.info.id = id.into();
        reply
    };

    let mut prompt = message(
        Role::User,
        vec![Part::Text {
            text: "after a stop".into(),
        }],
    );
    prompt.info.id = "msg_2".into();
    let mut transcript = vec![thought("msg_1"), prompt, thought("msg_3")];

    drop_earlier_reasoning(&mut transcript, Some("msg_2"));

    let kept = |message: &MessageWithParts| {
        message
            .parts
            .iter()
            .filter(|row| matches!(&row.part, Part::Reasoning { signature: None, .. }))
            .count()
    };
    assert_eq!(
        (kept(&transcript[0]), kept(&transcript[2])),
        (0, 1),
        "the stopped turn's unsigned thought goes; this turn's stays"
    );
    assert_eq!(
        transcript[0].parts.len(),
        1,
        "signed reasoning is the adapter's to judge, never dropped here"
    );
}

#[test]
fn unsigned_reasoning_goes_back_only_from_a_finished_reply_and_adjacent_users_merge() {
    let thought = || Part::Reasoning {
        text: "hm".into(),
        signature: None,
        redacted: None,
    };
    let transcript = vec![
        message(Role::User, vec![Part::Text { text: "a".into() }]),
        message(Role::User, vec![Part::Text { text: "b".into() }]),
        message(Role::Assistant, vec![thought(), Part::Text { text: "x".into() }]),
    ];
    let output = messages(&transcript, &target());
    assert_eq!(output[0].blocks, vec![Block::Text("a".into()), Block::Text("b".into())]);
    assert_eq!(
        output[1].blocks,
        vec![
            Block::Reasoning {
                text: "hm".into(),
                signature: None,
                redacted: None
            },
            Block::Text("x".into())
        ],
        "for wires that take reasoning_content"
    );
    let other = ModelRef {
        provider: "openai".into(),
        model: "gpt-5".into(),
    };
    assert_eq!(
        messages(&transcript, &other)[1].blocks,
        vec![Block::Text("hm".into()), Block::Text("x".into())],
        "another model reads it as text"
    );

    let aborted = vec![message_with(
        Role::Assistant,
        MessageStatus::Aborted,
        vec![thought(), Part::Text { text: "x".into() }],
    )];
    assert_eq!(
        messages(&aborted, &target())[0].blocks,
        vec![Block::Text("x".into())],
        "a cut-off thought is not replayed"
    );
    assert_eq!(
        messages(&aborted, &other)[0].blocks,
        vec![Block::Text("x".into())],
        "not even as text"
    );
}
