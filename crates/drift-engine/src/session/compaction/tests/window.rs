use std::fmt::Write as _;

use super::*;

fn model(context: u64, output: u64) -> Model {
    let mut model = crate::llm::catalog::Catalog::bundled()
        .model("anthropic", "claude-sonnet-4-5")
        .unwrap()
        .clone();
    model.limit = crate::llm::catalog::Limit {
        context,
        output,
        input: 0,
    };

    model
}

fn used(input: u64) -> Vec<MessageWithParts> {
    vec![MessageWithParts {
        info: Message {
            id: "m".into(),
            session_id: "s".into(),
            role: Role::Assistant,
            status: MessageStatus::Done,
            model: None,
            agent: None,
            usage: Usage {
                input,
                ..Usage::default()
            },
            cost: 0.0,
            error: None,
            created_at: 0,
            finished_at: None,
            summary: false,
            ending: None,
        },
        parts: Vec::new(),
    }]
}

#[test]
fn a_small_window_compacts_only_when_it_is_actually_filling() {
    assert!(
        !overflowing(&model(4_096, 0), &used(1_000)),
        "a 4k model with room left does not compact every step"
    );
    assert!(
        overflowing(&model(4_096, 0), &used(3_200)),
        "it does once less than a quarter is left"
    );
    assert!(!overflowing(&model(32_000, 0), &used(20_000)));
    assert!(
        overflowing(&model(200_000, 64_000), &used(170_000)),
        "a known output limit is the room, at most 32k"
    );
    assert!(
        !overflowing(&model(0, 0), &used(1_000_000)),
        "an unknown window never compacts on its own"
    );
    assert!(
        !overflowing(&model(32_768, 32_768), &used(8_000)),
        "an output limit as large as the window still leaves the prompt half"
    );
    assert!(!overflowing(&model(16_000, 64_000), &used(4_000)));
    assert!(overflowing(&model(32_768, 32_768), &used(17_000)));

    let capped = |input| {
        let mut model = model(400_000, 128_000);
        model.limit.input = input;
        model
    };
    assert!(
        overflowing(&capped(272_000), &used(260_000)),
        "an input cap below the window is where it compacts, before the provider refuses"
    );
    assert!(!overflowing(&capped(272_000), &used(240_000)));
    assert!(
        !overflowing(&capped(400_000), &used(260_000)),
        "a cap equal to the window changes nothing"
    );
    let bundled = crate::llm::catalog::Catalog::bundled();
    let gpt = bundled.model("openai", "gpt-5.4").expect("bundled");
    assert!(
        gpt.limit.input > 0 && gpt.limit.input < gpt.limit.context,
        "models.dev's input cap is read"
    );
}

#[tokio::test]
async fn a_step_loads_from_the_kept_tail_and_sends_what_the_whole_transcript_would() {
    let h = harness().await;
    for (ask, reply) in [
        ("first ANCIENT", "one"),
        ("second", "two"),
        ("third", "three"),
        ("fourth", "four"),
    ] {
        h.provider.push(text(reply));
        turn(&h, ask).await;
    }
    h.provider.push(text("SUMMARY of the start"));
    h.engine.start_compaction(&h.session.id).unwrap();
    until_idle(&h).await;
    h.provider.push(text("five"));
    turn(&h, "fifth").await;

    let full = h.engine.store.transcript(&h.session.id).unwrap();
    let start = h
        .engine
        .store
        .view_start(&h.session.id)
        .unwrap()
        .expect("a finished summary has a view start");
    let window = h.engine.store.messages_from(&h.session.id, &start).unwrap();
    assert!(
        window.len() < full.len() && !window.iter().any(|message| texts(message).contains("ANCIENT")),
        "summarised history is not loaded"
    );
    let target = crate::session::turn::tests::model();
    assert_eq!(
        request_messages(&window, &target, &[]),
        request_messages(&full, &target, &[]),
        "the request is the same either way"
    );
    let requests = requests(&h);
    let last = requests.last().unwrap();
    assert!(mentions(last, "SUMMARY of the start") && !mentions(last, "ANCIENT"));
}

#[tokio::test]
async fn a_single_long_turn_keeps_its_newest_steps_and_its_prompt_verbatim() {
    let h = harness().await;
    for index in 0..6 {
        let mut contents = String::new();
        for line in 0..1_500 {
            let _ = writeln!(contents, "BODY{index} line {line}");
        }
        std::fs::write(h._dir.join(format!("ws/big{index}.txt")), contents).unwrap();
        let input = format!(r#"{{"path": "big{index}.txt"}}"#);
        h.provider.push(crate::session::turn::tests::tool_call("read", &input));
    }
    h.provider.push(text("all read"));
    turn(&h, "PLEASE AUDIT EVERY FILE").await;
    h.provider.push(text("SUMMARY of the audit"));
    h.engine.start_compaction(&h.session.id).unwrap();
    until_idle(&h).await;

    let full = h.engine.store.transcript(&h.session.id).unwrap();
    let boundary = full
        .iter()
        .flat_map(|message| &message.parts)
        .map(|row| &row.part)
        .find(|part| matches!(part, Part::Compaction { .. }));
    let Some(Part::Compaction {
        tail_from: Some(tail), ..
    }) = boundary
    else {
        panic!("a tail inside the turn")
    };
    let kept = full.iter().find(|message| &message.info.id == tail).unwrap();
    assert_eq!(
        kept.info.role,
        Role::Assistant,
        "the tail starts at a reply inside the one turn"
    );
    let window = h.engine.request_window(&h.session.id).unwrap();
    let target = crate::session::turn::tests::model();
    let sent = request_messages(&window, &target, &[]);
    assert_eq!(
        sent,
        request_messages(&full, &target, &[]),
        "the loaded window sends what the whole transcript would"
    );
    let opening = match &sent[0].blocks[..] {
        [Block::Text(summary), Block::Text(request), ..] => (summary.clone(), request.clone()),
        other => panic!("{other:?}"),
    };
    assert!(
        opening.0.contains("SUMMARY of the audit") && opening.1.contains("PLEASE AUDIT EVERY FILE"),
        "{opening:?}"
    );
    let shown = format!("{sent:?}");
    assert!(shown.contains("BODY5"), "the newest steps stay verbatim");
    assert!(!shown.contains("BODY0 "), "the oldest are summarised");
}

#[test]
fn the_kept_tail_scales_with_the_conversations_model() {
    assert_eq!(
        tail_budget(&model(32_000, 0)),
        6_000,
        "a 32k local model compacts at 24k and keeps a quarter of that"
    );
    assert_eq!(tail_budget(&model(8_192, 0)), 2_000, "never less than 2k");
    assert_eq!(tail_budget(&model(1_000_000, 64_000)), 15_000, "never more than 15k");
    assert_eq!(tail_budget(&model(0, 0)), 2_000, "an unknown window keeps the least");
}
