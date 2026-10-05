use std::collections::HashMap;
use std::time::Duration;

use serde_json::json;

use super::*;
use crate::llm::{Chunk, StopReason};
use crate::session::turn::tests::{harness, prompt, text, until_idle, Harness};
use crate::session::types::Usage;

/// A reply whose request used `input` tokens, enough to trip the threshold when large.
fn reply_using(input: u64, words: &str) -> Vec<Chunk> {
    vec![Chunk::Usage(Usage { input, ..Usage::default() }), Chunk::TextStart, Chunk::TextDelta(words.into()), Chunk::BlockStop, Chunk::Stop(StopReason::EndTurn)]
}

async fn turn(h: &Harness, ask: &str) {
    h.engine.submit(&h.session.id, prompt(ask)).await.unwrap();
    until_idle(h).await;
}

fn texts(message: &MessageWithParts) -> String {
    text_of(message)
}

fn requests(h: &Harness) -> Vec<crate::llm::Request> {
    h.provider.requests.lock().unwrap().clone()
}

fn first_text(request: &crate::llm::Request) -> String {
    match &request.messages[0].blocks[0] {
        Block::Text(text) => text.clone(),
        other => panic!("{other:?}"),
    }
}

fn mentions(request: &crate::llm::Request, needle: &str) -> bool {
    request.messages.iter().flat_map(|m| m.blocks.iter()).any(|b| matches!(b, Block::Text(t) if t.contains(needle)))
}

/// Temporary triggers that make storing a finished summary fail at each of its two writes.
const STORAGE_FAULTS: [(&str, &str); 2] = [
    ("reject_summary_text", "CREATE TEMP TRIGGER reject_summary_text BEFORE INSERT ON part WHEN (SELECT summary FROM message WHERE id = NEW.message_id) = 1 BEGIN SELECT RAISE(ABORT, 'injected'); END;"),
    ("reject_summary_done", "CREATE TEMP TRIGGER reject_summary_done BEFORE UPDATE OF status ON message WHEN OLD.summary = 1 AND NEW.status = 'done' BEGIN SELECT RAISE(ABORT, 'injected'); END;"),
];

#[test]
fn a_small_window_compacts_only_when_it_is_actually_filling() {
    let message_with_usage = |input: u64| MessageWithParts {
        info: Message {
            id: "m".into(),
            session_id: "s".into(),
            role: Role::Assistant,
            status: MessageStatus::Done,
            model: None,
            agent: None,
            usage: Usage { input, ..Usage::default() },
            cost: 0.0,
            error: None,
            created_at: 0,
            finished_at: None,
            summary: false,
            ending: None,
        },
        parts: Vec::new(),
    };
    let model = |context: u64, output: u64| {
        let mut model = crate::llm::catalog::Catalog::bundled().model("anthropic", "claude-sonnet-4-5").unwrap().clone();
        model.limit = crate::llm::catalog::Limit { context, output, input: 0 };
        model
    };
    let used = |tokens: u64| vec![message_with_usage(tokens)];
    assert!(!overflowing(&model(4_096, 0), &used(1_000)), "a 4k model with room left does not compact every step");
    assert!(overflowing(&model(4_096, 0), &used(3_200)), "it does once less than a quarter is left");
    assert!(!overflowing(&model(32_000, 0), &used(20_000)));
    assert!(overflowing(&model(200_000, 64_000), &used(170_000)), "a known output limit is the room, at most 32k");
    assert!(!overflowing(&model(0, 0), &used(1_000_000)), "an unknown window never compacts on its own");
    assert!(!overflowing(&model(32_768, 32_768), &used(8_000)), "an output limit as large as the window still leaves the prompt half");
    assert!(!overflowing(&model(16_000, 64_000), &used(4_000)));
    assert!(overflowing(&model(32_768, 32_768), &used(17_000)));
    let capped = |input: u64| {
        let mut capped = model(400_000, 128_000);
        capped.limit.input = input;
        capped
    };
    assert!(overflowing(&capped(272_000), &used(260_000)), "an input cap below the window is where it compacts, before the provider refuses");
    assert!(!overflowing(&capped(272_000), &used(240_000)));
    assert!(!overflowing(&capped(400_000), &used(260_000)), "a cap equal to the window changes nothing");
    let bundled = crate::llm::catalog::Catalog::bundled();
    let gpt = bundled.model("openai", "gpt-5.4").expect("bundled");
    assert!(gpt.limit.input > 0 && gpt.limit.input < gpt.limit.context, "models.dev's input cap is read");
}

#[tokio::test]
async fn a_step_loads_from_the_kept_tail_and_sends_what_the_whole_transcript_would() {
    let h = harness().await;
    for (ask, reply) in [("first ANCIENT", "one"), ("second", "two"), ("third", "three"), ("fourth", "four")] {
        h.provider.push(text(reply));
        turn(&h, ask).await;
    }
    h.provider.push(text("SUMMARY of the start"));
    h.engine.start_compaction(&h.session.id).unwrap();
    until_idle(&h).await;
    h.provider.push(text("five"));
    turn(&h, "fifth").await;
    let full = h.engine.store.transcript(&h.session.id).unwrap();
    let start = h.engine.store.view_start(&h.session.id).unwrap().expect("a finished summary has a view start");
    let window = h.engine.store.messages_from(&h.session.id, &start).unwrap();
    assert!(window.len() < full.len() && !window.iter().any(|m| texts(m).contains("ANCIENT")), "summarised history is not loaded");
    let target = crate::session::turn::tests::model();
    assert_eq!(request_messages(&window, &target, &[]), request_messages(&full, &target, &[]), "the request is the same either way");
    assert!(mentions(requests(&h).last().unwrap(), "SUMMARY of the start") && !mentions(requests(&h).last().unwrap(), "ANCIENT"));
}

#[tokio::test]
async fn a_single_long_turn_keeps_its_newest_steps_and_its_prompt_verbatim() {
    let h = harness().await;
    for i in 0..6 {
        std::fs::write(h._dir.join(format!("ws/big{i}.txt")), (0..1_500).map(|n| format!("BODY{i} line {n}\n")).collect::<String>()).unwrap();
        h.provider.push(crate::session::turn::tests::tool_call("read", &format!(r#"{{"path": "big{i}.txt"}}"#)));
    }
    h.provider.push(text("all read"));
    turn(&h, "PLEASE AUDIT EVERY FILE").await;
    h.provider.push(text("SUMMARY of the audit"));
    h.engine.start_compaction(&h.session.id).unwrap();
    until_idle(&h).await;
    let full = h.engine.store.transcript(&h.session.id).unwrap();
    let Some(Part::Compaction { tail_from: Some(tail), .. }) = full.iter().flat_map(|m| &m.parts).map(|row| &row.part).find(|part| matches!(part, Part::Compaction { .. })) else { panic!("a tail inside the turn") };
    let kept = full.iter().find(|m| &m.info.id == tail).unwrap();
    assert_eq!(kept.info.role, Role::Assistant, "the tail starts at a reply inside the one turn");
    let window = h.engine.request_window(&h.session.id).unwrap();
    let target = crate::session::turn::tests::model();
    let sent = request_messages(&window, &target, &[]);
    assert_eq!(sent, request_messages(&full, &target, &[]), "the loaded window sends what the whole transcript would");
    let opening = match &sent[0].blocks[..] { [Block::Text(summary), Block::Text(request), ..] => (summary.clone(), request.clone()), other => panic!("{other:?}") };
    assert!(opening.0.contains("SUMMARY of the audit") && opening.1.contains("PLEASE AUDIT EVERY FILE"), "{opening:?}");
    let shown = format!("{sent:?}");
    assert!(shown.contains("BODY5"), "the newest steps stay verbatim");
    assert!(!shown.contains("BODY0 "), "the oldest are summarised");
}

#[tokio::test]
async fn a_summary_that_cannot_be_stored_never_replaces_the_history() {
    for (name, fault) in STORAGE_FAULTS {
        let h = harness().await;
        for (ask, reply) in [("first ORIGINAL_REQUIREMENT", "one"), ("second", "two"), ("third", "three")] {
            h.provider.push(text(reply));
            turn(&h, ask).await;
        }
        h.engine.store.lock().execute_batch(fault).unwrap();
        h.provider.push(text("SUMMARY"));
        h.engine.start_compaction(&h.session.id).unwrap();
        until_idle(&h).await;
        let transcript = h.engine.store.transcript(&h.session.id).unwrap();
        let summary = transcript.last().unwrap();
        assert!(summary.info.summary && summary.info.status == MessageStatus::Error, "{name}: {:?}", summary.info.status);
        assert!(summary.info.error.as_deref().unwrap().contains("not saved"), "{name}");
        assert!(view(&transcript).summary.is_none(), "{name}: the failed summary is not active");

        h.engine.store.lock().execute_batch(&format!("DROP TRIGGER temp.{name};")).unwrap();
        h.provider.push(text("four"));
        turn(&h, "fourth").await;
        assert!(mentions(requests(&h).last().unwrap(), "ORIGINAL_REQUIREMENT"), "{name}: the history is still sent");
    }
}

#[tokio::test]
async fn unsuccessful_summary_endings_keep_the_previous_valid_context() {
    for reason in [StopReason::MaxTokens, StopReason::Refused, StopReason::ContextFull, StopReason::ToolUse, StopReason::Other] {
        let h = harness().await;
        for ask in ["first", "second", "third"] {
            h.provider.push(text("reply"));
            turn(&h, ask).await;
        }
        h.provider.push(text("VALID SUMMARY"));
        h.engine.start_compaction(&h.session.id).unwrap();
        until_idle(&h).await;
        let start = h.engine.store.view_start(&h.session.id).unwrap();
        h.provider.push(vec![Chunk::TextStart, Chunk::TextDelta("PARTIAL SUMMARY".into()), Chunk::BlockStop, Chunk::Stop(reason)]);
        h.engine.start_compaction(&h.session.id).unwrap();
        until_idle(&h).await;
        let transcript = h.engine.store.transcript(&h.session.id).unwrap();
        assert_eq!(transcript.last().unwrap().info.status, MessageStatus::Error, "{reason:?}");
        assert_eq!(h.engine.store.view_start(&h.session.id).unwrap(), start, "{reason:?}");
        assert_eq!(view(&transcript).summary.as_deref(), Some("VALID SUMMARY"), "{reason:?}");
        h.provider.push(text("continued"));
        turn(&h, "continue").await;
        assert!(mentions(requests(&h).last().unwrap(), "VALID SUMMARY"));
        assert!(!mentions(requests(&h).last().unwrap(), "PARTIAL SUMMARY"));
    }
}

#[tokio::test]
async fn a_storage_failure_counts_against_automatic_compaction() {
    let h = harness().await;
    h.provider.push(reply_using(980_000, "long"));
    turn(&h, "first ORIGINAL_REQUIREMENT").await;
    h.engine.store.lock().execute_batch(STORAGE_FAULTS[0].1).unwrap();
    h.provider.push(text("SUMMARY")).push(text("anyway"));
    turn(&h, "second").await;
    assert_eq!(h.engine.turns.compaction_failures.lock().unwrap()[&h.session.id], 1);
    assert!(mentions(requests(&h).last().unwrap(), "ORIGINAL_REQUIREMENT"), "the turn ran on the uncompacted history");
}

#[tokio::test]
async fn a_manual_compaction_summarises_older_turns_and_keeps_the_recent_ones() {
    let h = harness().await;
    for (ask, reply) in [("first", "one"), ("second", "two"), ("third", "three")] {
        h.provider.push(text(reply));
        turn(&h, ask).await;
    }
    h.provider.push(text("SUMMARY"));
    h.engine.start_compaction(&h.session.id).unwrap();
    assert!(matches!(h.engine.start_compaction(&h.session.id), Err(TurnError::Busy)), "one job at a time");
    until_idle(&h).await;

    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let (boundary, summary) = (&transcript[6], &transcript[7]);
    let Part::Compaction { auto, tail_from } = &boundary.parts[0].part else { panic!("{:?}", boundary.parts) };
    assert!(!auto);
    assert_eq!(tail_from.as_deref(), Some(transcript[2].info.id.as_str()), "the last two turns stay verbatim");
    assert!(summary.info.summary && summary.info.status == MessageStatus::Done);
    assert_eq!(texts(summary), "SUMMARY");

    let summarised = requests(&h).last().unwrap().clone();
    assert_eq!(summarised.messages.len(), 3, "the first turn, then the instructions");
    assert!(summarised.messages[2].blocks.iter().any(|b| matches!(b, Block::Text(t) if t.contains("do not call tools"))));
    assert!(summarised.no_tool_calls && !summarised.tools.is_empty(), "tools stay defined for the history but cannot be called");

    h.provider.push(text("four"));
    turn(&h, "fourth").await;
    let next = requests(&h).last().unwrap().clone();
    assert!(!next.no_tool_calls);
    assert!(first_text(&next).contains("SUMMARY") && first_text(&next).starts_with("This conversation was compacted"));
    let rest: Vec<String> = next.messages.iter().flat_map(|m| m.blocks.iter()).filter_map(|b| match b { Block::Text(t) => Some(t.clone()), _ => None }).skip(1).collect();
    assert_eq!(rest, ["second", "two", "third", "three", "fourth"], "the summary replaces the first turn only");
}

#[tokio::test]
async fn a_turn_compacts_itself_when_the_last_reply_left_too_little_room() {
    let h = harness().await;
    h.provider.push(reply_using(980_000, "long answer"));
    turn(&h, "read the whole repo").await;
    h.provider.push(text("SUMMARY")).push(text("done"));
    turn(&h, "now fix it").await;

    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let boundary = transcript.iter().find(|m| matches!(m.parts.first().map(|p| &p.part), Some(Part::Compaction { .. }))).unwrap();
    let Part::Compaction { auto, tail_from } = &boundary.parts[0].part else { unreachable!() };
    assert!(auto);
    assert_eq!(tail_from.as_deref(), Some(transcript[2].info.id.as_str()), "the prompt being answered is kept verbatim");
    let last = requests(&h).last().unwrap().clone();
    assert!(first_text(&last).contains("SUMMARY"));
    assert!(last.messages[0].blocks.iter().any(|b| matches!(b, Block::Text(t) if t == "now fix it")));
    assert_eq!(texts(transcript.last().unwrap()), "done");
}

#[tokio::test]
async fn a_request_the_provider_rejects_as_too_long_is_compacted_and_retried_once() {
    let h = harness().await;
    h.provider.push(text("one"));
    turn(&h, "first").await;
    let too_long = || crate::llm::Error::api(400, "invalid_request_error", "prompt is too long: 210000 tokens > 200000 maximum");
    h.provider.push_error(too_long()).push(text("SUMMARY")).push(text("two"));
    turn(&h, "second").await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let statuses: Vec<(bool, MessageStatus)> = transcript.iter().map(|m| (m.info.summary, m.info.status)).collect();
    assert!(statuses.contains(&(false, MessageStatus::Error)), "the rejected attempt stays as history");
    assert!(statuses.contains(&(true, MessageStatus::Done)));
    assert_eq!(texts(transcript.last().unwrap()), "two");

    h.provider.push_error(too_long()).push(text("SUMMARY 2")).push_error(too_long());
    turn(&h, "third").await;
    let last = h.engine.store.transcript(&h.session.id).unwrap().last().unwrap().clone();
    assert_eq!(last.info.status, MessageStatus::Error, "a second overflow in the same turn gives up");
    assert!(h.provider.responses_left() == 0);
}

#[tokio::test]
async fn automatic_compaction_can_be_switched_off_and_stops_after_repeated_failures() {
    let h = harness().await;
    h.provider.push(reply_using(980_000, "long"));
    turn(&h, "first").await;
    let model = h.engine.catalog.read().unwrap().providers["anthropic"].models["claude-sonnet-4-5"].clone();
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert!(h.engine.wants_compaction(&h.session.id, &model, &transcript));

    h.engine.store.set_setting(AUTO_COMPACT_KEY, &false).unwrap();
    assert!(!h.engine.wants_compaction(&h.session.id, &model, &transcript));
    h.engine.store.set_setting(AUTO_COMPACT_KEY, &true).unwrap();

    h.provider.push_error(crate::llm::Error::Api { status: 500, kind: "api_error".into(), message: "down".into(), retryable: false, retry_after: None }).push(text("anyway"));
    turn(&h, "second").await;
    assert_eq!(h.engine.turns.compaction_failures.lock().unwrap()[&h.session.id], 1, "a failed compaction still lets the turn run");
    assert_eq!(texts(h.engine.store.transcript(&h.session.id).unwrap().last().unwrap()), "anyway");
    h.engine.turns.compaction_failures.lock().unwrap().insert(h.session.id.clone(), MAX_AUTO_FAILURES);
    assert!(!h.engine.wants_compaction(&h.session.id, &model, &transcript));
}

#[tokio::test]
async fn the_compaction_model_pinned_in_settings_writes_the_summary() {
    let h = harness().await;
    for (ask, reply) in [("first", "one"), ("second", "two"), ("third", "three")] {
        h.provider.push(text(reply));
        turn(&h, ask).await;
    }
    let pinned = h.engine.catalog.read().unwrap().providers["anthropic"].models.keys().find(|id| id.as_str() != "claude-sonnet-4-5").unwrap().clone();
    let pin = crate::config::AgentOverride::from_json(&json!({ "model": format!("anthropic/{pinned}") }));
    h.engine.set_agent_overrides(HashMap::from([("compaction".to_string(), pin)]));
    h.provider.push(text("SUMMARY"));
    h.engine.start_compaction(&h.session.id).unwrap();
    until_idle(&h).await;
    let summary = requests(&h).last().unwrap().clone();
    assert_eq!(summary.model, pinned);
    assert!(summary.system.is_empty() && summary.cache_key.is_none(), "another model has no cached prefix of this conversation to reuse");
}

#[tokio::test]
async fn nothing_is_written_when_there_is_nothing_to_compact_and_stop_cancels_a_summary() {
    let h = harness().await;
    h.engine.start_compaction(&h.session.id).unwrap();
    until_idle(&h).await;
    assert!(h.engine.store.transcript(&h.session.id).unwrap().is_empty());

    for (ask, reply) in [("first", "one"), ("second", "two"), ("third", "three")] {
        h.provider.push(text(reply));
        turn(&h, ask).await;
    }
    h.provider.push_stall();
    h.engine.start_compaction(&h.session.id).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(h.engine.abort(&h.session.id));
    until_idle(&h).await;
    let summary = h.engine.store.transcript(&h.session.id).unwrap().last().unwrap().clone();
    assert!(summary.info.summary && summary.info.status == MessageStatus::Aborted);
    assert_eq!(view(&h.engine.store.transcript(&h.session.id).unwrap()).summary, None, "an aborted summary is ignored");
}

#[tokio::test]
async fn a_fork_of_a_compacted_conversation_sees_the_same_history() {
    let h = harness().await;
    for (ask, reply) in [("first", "one"), ("second", "two"), ("third", "three")] {
        h.provider.push(text(reply));
        turn(&h, ask).await;
    }
    h.provider.push(text("SUMMARY"));
    h.engine.start_compaction(&h.session.id).unwrap();
    until_idle(&h).await;
    let fork = h.engine.fork(&h.session.id, None).unwrap();
    let source = h.engine.store.transcript(&h.session.id).unwrap();
    let copy = h.engine.store.transcript(&fork.id).unwrap();
    let shape = |t: &[MessageWithParts]| {
        let view = view(t);
        (view.summary, view.messages.iter().map(|m| texts(m)).collect::<Vec<_>>())
    };
    assert_eq!(shape(&copy), shape(&source));
    assert_eq!(shape(&copy).1, ["second", "two", "third", "three"]);
}

#[test]
fn the_kept_tail_scales_with_the_conversations_model() {
    let model = |context: u64, output: u64| {
        let mut model = crate::llm::catalog::Catalog::bundled().model("anthropic", "claude-sonnet-4-5").unwrap().clone();
        model.limit = crate::llm::catalog::Limit { context, output, input: 0 };
        model
    };
    assert_eq!(tail_budget(&model(32_000, 0)), 6_000, "a 32k local model compacts at 24k and keeps a quarter of that");
    assert_eq!(tail_budget(&model(8_192, 0)), 2_000, "never less than 2k");
    assert_eq!(tail_budget(&model(1_000_000, 64_000)), 8_000, "never more than 8k");
    assert_eq!(tail_budget(&model(0, 0)), 2_000, "an unknown window keeps the least");
}

#[tokio::test]
async fn a_summary_on_the_conversations_model_shares_its_frame_drops_files_cuts_long_results_and_retries_an_overload() {
    let h = harness().await;
    let lines: String = (0..400).map(|n| format!("line {n} of a long file that the summary does not need whole\n")).collect();
    std::fs::write(h._dir.join("ws/big.txt"), &lines).unwrap();
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    let image = Part::File { mime: "image/png".into(), name: "shot.png".into(), url: format!("data:image/png;base64,{png}"), path: None };
    h.provider.push(crate::session::turn::tests::tool_call("read", r#"{"path":"big.txt"}"#)).push(text("seen"));
    h.engine.submit(&h.session.id, crate::session::turn::Prompt { parts: vec![image, Part::Text { text: "look".into() }], ..prompt("") }).await.unwrap();
    until_idle(&h).await;
    for (ask, reply) in [("second", "two"), ("third", "three")] {
        h.provider.push(text(reply));
        turn(&h, ask).await;
    }
    h.provider.push_error(crate::llm::Error::api(529, "overloaded_error", "busy")).push(text("SUMMARY"));
    h.engine.start_compaction(&h.session.id).unwrap();
    until_idle(&h).await;
    let all = requests(&h);
    let (conversation, summary) = (&all[0], all.last().unwrap());
    assert_eq!(all.len(), 6, "the overloaded summary request was sent again");
    assert_eq!(texts(h.engine.store.transcript(&h.session.id).unwrap().last().unwrap()), "SUMMARY");
    assert_eq!(summary.system, conversation.system, "the conversation's own system prompt opens it");
    assert_eq!(summary.tools.iter().map(|t| &t.name).collect::<Vec<_>>(), conversation.tools.iter().map(|t| &t.name).collect::<Vec<_>>());
    assert_eq!(summary.cache_key.as_deref(), Some(h.session.id.as_str()));
    let blocks: Vec<&Block> = summary.messages.iter().flat_map(|m| m.blocks.iter()).collect();
    assert!(!blocks.iter().any(|b| matches!(b, Block::Image { .. } | Block::Pdf { .. } | Block::Stored { .. })), "no file goes to the summary");
    assert!(blocks.iter().any(|b| matches!(b, Block::Text(t) if t.contains("image/png file was attached"))));
    let result = blocks.iter().find_map(|b| match b { Block::ToolResult { content, .. } => Some(content.clone()), _ => None }).unwrap();
    assert!(result.ends_with("[... cut for the summary]") && result.chars().count() < 2_100, "{}", result.len());
}
