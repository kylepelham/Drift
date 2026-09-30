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

    h.provider.push(text("four"));
    turn(&h, "fourth").await;
    let next = requests(&h).last().unwrap().clone();
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
    assert_eq!(requests(&h).last().unwrap().model, pinned);
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
