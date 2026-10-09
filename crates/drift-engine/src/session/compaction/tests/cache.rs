use std::fmt::Write as _;

use super::*;

#[tokio::test]
async fn a_warm_summary_on_the_conversations_model_is_its_next_request_with_the_instructions_and_shows_its_retries() {
    let h = harness().await;
    let (png, big) = files(&h);
    h.provider
        .push(crate::session::turn::tests::tool_call("read", r#"{"path":"big.txt"}"#))
        .push(text("seen"));
    h.engine
        .submit(
            &h.session.id,
            crate::session::turn::Prompt {
                parts: vec![png, Part::Text { text: "look".into() }],
                ..prompt("")
            },
        )
        .await
        .unwrap();
    until_idle(&h).await;
    for (ask, reply) in [("second", "two"), ("third", "three")] {
        h.provider.push(text(reply));
        turn(&h, ask).await;
    }

    let mut events = h.engine.hub.attach(None).rx;
    h.provider
        .push_error(crate::llm::Error::api(529, "overloaded_error", "busy"))
        .push(text("SUMMARY"));
    h.engine.start_compaction(&h.session.id).unwrap();
    until_idle(&h).await;

    let all = requests(&h);
    let (last_turn, summary) = (&all[all.len() - 3], all.last().unwrap());
    assert_eq!(all.len(), 6, "the overloaded summary request was sent again");
    assert_warm_summary_stored(&h);
    assert_eq!(
        summary.messages[..last_turn.messages.len()],
        last_turn.messages[..],
        "the conversation's own messages, uncut, so its cached prefix is read"
    );
    let last_line = big.lines().last().unwrap();
    let blocks: Vec<_> = summary.messages.iter().flat_map(|message| &message.blocks).collect();
    assert!(
        mentions(summary, "do not call tools")
            && blocks
                .iter()
                .any(|block| matches!(block, Block::ToolResult { content, .. } if content.contains(last_line))),
        "the read's result is whole"
    );
    assert!(
        blocks.iter().any(|block| matches!(block, Block::Image { .. })),
        "files stay, as the cache holds them"
    );
    let same = (
        summary.system == last_turn.system,
        summary.tools.len() == last_turn.tools.len(),
        summary.reasoning == last_turn.reasoning,
        summary.no_tool_calls == last_turn.no_tool_calls,
        summary.cache_key == last_turn.cache_key,
    );
    assert_eq!(
        same,
        (true, true, true, true, true),
        "everything a provider keys its cache on matches the turn's"
    );
    let retried = std::iter::from_fn(|| events.try_recv().ok()).any(|envelope| {
        matches!(envelope.event, crate::event::Event::SessionRetry { ref session_id, attempt: 1, .. }
            if *session_id == h.session.id)
    });
    assert!(retried, "the user waiting on the compaction sees it retry, as a turn's");
}

fn assert_warm_summary_stored(h: &Harness) {
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let stored = transcript.last().unwrap();

    assert_eq!(texts(stored), "SUMMARY");
    assert!(
        stored.info.usage
            == Usage {
                input: 10,
                output: 3,
                cache_read: 0,
                cache_write: 0
            }
            && stored.info.cost > 0.0,
        "the summary is charged like a reply: {:?} {}",
        stored.info.usage,
        stored.info.cost
    );
}

#[tokio::test]
async fn a_cold_summary_is_lean_drops_files_and_cuts_long_results() {
    let h = harness().await;
    let (png, _) = files(&h);
    h.provider
        .push(crate::session::turn::tests::tool_call("read", r#"{"path":"big.txt"}"#))
        .push(text("seen"));
    h.engine
        .submit(
            &h.session.id,
            crate::session::turn::Prompt {
                parts: vec![png, Part::Text { text: "look".into() }],
                ..prompt("")
            },
        )
        .await
        .unwrap();
    until_idle(&h).await;
    for (ask, reply) in [("second", "two"), ("third", "three")] {
        h.provider.push(text(reply));
        turn(&h, ask).await;
    }
    h.engine
        .store
        .lock()
        .execute("UPDATE message SET finished_at = 1 WHERE role = 'assistant'", [])
        .unwrap();
    h.provider.push(text("SUMMARY"));
    h.engine.start_compaction(&h.session.id).unwrap();
    until_idle(&h).await;

    let summary = requests(&h).last().unwrap().clone();
    assert!(
        summary.system.is_empty() && summary.cache_key.is_none() && summary.no_tool_calls,
        "no cache to reuse: the smallest request"
    );
    let blocks: Vec<_> = summary.messages.iter().flat_map(|message| &message.blocks).collect();
    assert!(
        !blocks
            .iter()
            .any(|block| matches!(block, Block::Image { .. } | Block::Pdf { .. } | Block::Stored { .. })),
        "no file goes to the summary"
    );
    assert!(
        blocks
            .iter()
            .any(|block| matches!(block, Block::Text(text) if text.contains("image/png file was attached")))
    );
    let result = blocks
        .iter()
        .find_map(|block| match block {
            Block::ToolResult { content, .. } => Some(content.clone()),
            _ => None,
        })
        .unwrap();
    assert!(
        result.ends_with("[... cut for the summary]") && result.chars().count() < 2_100,
        "{}",
        result.len()
    );
}

#[tokio::test]
async fn a_warm_summary_that_calls_a_tool_is_asked_again_the_lean_way() {
    let h = harness().await;
    for (ask, reply) in [("first", "one"), ("second", "two"), ("third", "three")] {
        h.provider.push(text(reply));
        turn(&h, ask).await;
    }
    let mut calls = vec![Chunk::Usage(Usage {
        input: 1_000,
        ..Usage::default()
    })];
    calls.extend(crate::session::turn::tests::tool_call("read", r#"{"path":"x"}"#));
    h.provider.push(calls).push(text("SUMMARY"));
    h.engine.start_compaction(&h.session.id).unwrap();
    until_idle(&h).await;

    let all = requests(&h);
    assert!(
        !all[all.len() - 2].system.is_empty() && all.last().unwrap().system.is_empty(),
        "the cached request first, then the lean one"
    );
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let stored = transcript.last().unwrap();
    assert_eq!(texts(stored), "SUMMARY");
    assert_eq!(
        stored.info.usage.input, 1_010,
        "the refused cached reply is paid for too"
    );
}

/// A one-pixel image prompt part and a file whose read is far over the summary's cut.
fn files(h: &Harness) -> (Part, String) {
    let mut lines = String::new();
    for line in 0..400 {
        let _ = writeln!(lines, "line {line} of a long file that the summary does not need whole");
    }
    std::fs::write(h._dir.join("ws/big.txt"), &lines).unwrap();
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

    (
        Part::File {
            mime: "image/png".into(),
            name: "shot.png".into(),
            url: format!("data:image/png;base64,{png}"),
            path: None,
        },
        lines,
    )
}

#[tokio::test]
async fn a_lean_summary_still_defines_the_workspaces_mcp_tools_its_history_may_call() {
    let h = harness().await;
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/echo-server.cjs");
    let config = crate::mcp::ServerConfig::Stdio {
        command: "node".into(),
        args: vec![script.into()],
        env: Default::default(),
        cwd: None,
        timeout_seconds: None,
    };
    h.engine.store.save_mcp_server("echo", &config).unwrap();
    h.engine
        .connect_mcp_in("echo", Some(&crate::tool::canonical(&h._dir.join("ws"))))
        .await
        .unwrap();
    for (ask, reply) in [("first", "one"), ("second", "two"), ("third", "three")] {
        h.provider.push(text(reply));
        turn(&h, ask).await;
    }
    h.engine
        .store
        .lock()
        .execute("UPDATE message SET finished_at = 1 WHERE role = 'assistant'", [])
        .unwrap();
    h.provider.push(text("SUMMARY"));
    h.engine.start_compaction(&h.session.id).unwrap();
    until_idle(&h).await;

    let summarised = requests(&h).last().unwrap().clone();
    assert!(
        summarised.no_tool_calls && summarised.tools.iter().any(|tool| tool.name == "echo_echo"),
        "{:?}",
        summarised.tools.iter().map(|tool| &tool.name).collect::<Vec<_>>()
    );
}
