use super::*;
use crate::llm::Block;
use crate::session::convert::CLEARED_RESULT;

/// A step that reads `path`, with a call id of its own.
fn read_call(index: usize, path: &str) -> Vec<Chunk> {
    vec![
        Chunk::Usage(Usage {
            input: 10,
            ..Usage::default()
        }),
        Chunk::ToolUseStart {
            id: format!("toolu_read_{index}"),
            name: "read".into(),
        },
        Chunk::ToolInputDelta(json!({ "path": path }).to_string()),
        Chunk::BlockStop,
        Chunk::Stop(StopReason::ToolUse),
    ]
}

/// Of the tool results in request `index`, how many were cleared and how many were sent whole.
fn results_in(h: &Harness, index: usize) -> (usize, usize) {
    let requests = h.provider.requests.lock().unwrap();
    let results: Vec<&String> = requests[index]
        .messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            Block::ToolResult { content, .. } => Some(content),
            _ => None,
        })
        .collect();
    let cleared = results
        .iter()
        .filter(|content| content.as_str() == CLEARED_RESULT)
        .count();

    (cleared, results.len() - cleared)
}

#[tokio::test]
async fn a_request_that_misses_the_cache_anyway_sends_old_large_results_as_a_note() {
    let h = harness().await;
    let workspace = h._dir.join("ws");
    for index in 0..14 {
        let line = format!("{index:02} the retry loop backs off before it asks the server again\n");
        std::fs::write(workspace.join(format!("notes{index}.txt")), line.repeat(500)).unwrap();
        h.provider.push(read_call(index, &format!("notes{index}.txt")));
    }
    h.provider.push(text("Read them all.")).push(text("Still here."));

    // Inside one turn the cache is warm, so every result goes whole.
    h.engine
        .submit(&h.session.id, prompt("Read the notes"))
        .await
        .await_ok();
    until_idle(&h).await;
    assert_eq!(results_in(&h, 14), (0, 14));

    let mut last = h.engine.store.last_cached_reply(&h.session.id).unwrap().unwrap();
    last.finished_at = Some(id::now_ms() - 2 * 3_600_000);
    h.engine.store.save_message(&last).unwrap();
    h.engine
        .submit(&h.session.id, prompt("Anything else?"))
        .await
        .await_ok();
    until_idle(&h).await;

    // Gone cold, the oldest results beyond the newest ten are cleared; the transcript still shows them whole.
    assert_eq!(results_in(&h, 15), (4, 10));
    let shown = transcript(&h);
    let first = shown
        .iter()
        .flat_map(|message| message.parts.iter())
        .find(|row| matches!(&row.part, Part::ToolCall { .. }))
        .unwrap();
    let view = tool(first);
    assert!(view.output.unwrap().contains("the retry loop backs off"));
    assert_eq!(view.metadata.and_then(|metadata| metadata.cleared), Some(true));
}

#[tokio::test]
async fn a_cold_request_with_little_to_clear_is_sent_as_it_was() {
    let h = harness().await;
    let workspace = h._dir.join("ws");
    for index in 0..12 {
        std::fs::write(workspace.join(format!("short{index}.txt")), format!("entry {index}\n")).unwrap();
        h.provider.push(read_call(index, &format!("short{index}.txt")));
    }
    h.provider.push(text("Read them all.")).push(text("Still here."));
    h.engine
        .submit(&h.session.id, prompt("Read the notes"))
        .await
        .await_ok();
    until_idle(&h).await;

    let mut last = h.engine.store.last_cached_reply(&h.session.id).unwrap().unwrap();
    last.finished_at = Some(id::now_ms() - 2 * 3_600_000);
    h.engine.store.save_message(&last).unwrap();
    h.engine
        .submit(&h.session.id, prompt("Anything else?"))
        .await
        .await_ok();
    until_idle(&h).await;

    assert_eq!(results_in(&h, 13), (0, 12));
}
