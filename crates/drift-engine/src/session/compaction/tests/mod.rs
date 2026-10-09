use crate::llm::{Chunk, StopReason};
use crate::session::turn::tests::{Harness, harness, prompt, text, until_idle};
use crate::session::types::Usage;
use serde_json::json;
use std::collections::HashMap;
use std::time::Duration;

use super::*;

mod cache;
mod lifecycle;
mod window;

/// Temporary triggers that make storing a finished summary fail at each of its two writes.
const STORAGE_FAULTS: [(&str, &str); 2] = [
    (
        "reject_summary_text",
        "CREATE TEMP TRIGGER reject_summary_text BEFORE INSERT ON part \
         WHEN (SELECT summary FROM message WHERE id = NEW.message_id) = 1 \
         BEGIN SELECT RAISE(ABORT, 'injected'); END;",
    ),
    (
        "reject_summary_done",
        "CREATE TEMP TRIGGER reject_summary_done BEFORE UPDATE OF status ON message \
         WHEN OLD.summary = 1 AND NEW.status = 'done' BEGIN SELECT RAISE(ABORT, 'injected'); END;",
    ),
];

/// A reply whose request used `input` tokens, enough to trip the threshold when large.
fn reply_using(input: u64, words: &str) -> Vec<Chunk> {
    vec![
        Chunk::Usage(Usage {
            input,
            ..Usage::default()
        }),
        Chunk::TextStart,
        Chunk::TextDelta(words.into()),
        Chunk::BlockStop,
        Chunk::Stop(StopReason::EndTurn),
    ]
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
    request
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .any(|block| matches!(block, Block::Text(text) if text.contains(needle)))
}
