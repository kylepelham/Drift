//! Compaction: when a conversation outgrows its context, a summary stands in for its older history.
//! Nothing is deleted; the transcript keeps every message and the request view skips what was summarised.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::convert;
use super::oneshot::{Fallback, OneShot, Resolved};
use super::turn::TurnError;
use super::types::{Message, MessageStatus, MessageWithParts, ModelRef, Part, Role};
use crate::event::Event;
use crate::id;
use crate::llm::catalog::Model;
use crate::llm::{self, Block, ChatMessage};
use crate::Engine;

/// Reply room kept free in the context window, capped like the UI's context meter (`src/engine/store.ts`).
const MAX_REPLY_TOKENS: u64 = 32_000;
/// The recent history kept verbatim: at most this many turns, and at most this many estimated tokens.
const TAIL_TURNS: usize = 2;
const TAIL_TOKENS: usize = 15_000;
const SUMMARY_MAX_TOKENS: u32 = 8_192;
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(300);
/// A summary request that is itself too long drops the oldest fifth of its turns, this many times at most.
const TRIM_ATTEMPTS: usize = 3;
/// Automatic compaction stops for a session after this many failures in a row.
pub(super) const MAX_AUTO_FAILURES: u32 = 3;
pub const AUTO_COMPACT_KEY: &str = "autoCompact";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Trigger {
    Manual,
    /// The last request left too little room for a reply.
    Auto,
    /// The provider rejected a request as too long.
    Overflow,
}

/// What the model sees: the latest finished summary, then the messages it kept and everything after.
pub(super) struct View<'a> {
    pub summary: Option<String>,
    pub messages: Vec<&'a MessageWithParts>,
}

pub(super) fn view(transcript: &[MessageWithParts]) -> View<'_> {
    // A summary without text replaces nothing; the view before it stands.
    let latest = transcript.iter().rposition(|m| m.info.summary && m.info.status == MessageStatus::Done && !text_of(m).trim().is_empty());
    let Some(index) = latest else {
        return View { summary: None, messages: transcript.iter().filter(|m| !is_marker(m)).collect() };
    };
    let tail_from = transcript[..index].iter().rev().find_map(boundary).flatten();
    let messages = transcript
        .iter()
        .enumerate()
        .filter(|(i, m)| !is_marker(m) && tail_from.as_ref().map_or(*i > index, |tail| m.info.id >= *tail))
        .map(|(_, m)| m)
        .collect();
    View { summary: Some(text_of(&transcript[index])), messages }
}

/// The request history for `target`: the summary as the opening user turn, then the kept messages.
pub(super) fn request_messages(transcript: &[MessageWithParts], target: &ModelRef) -> Vec<ChatMessage> {
    let view = view(transcript);
    let mut out = Vec::new();
    if let Some(summary) = &view.summary {
        convert::push(&mut out, llm::Role::User, vec![Block::Text(wrap(summary))]);
    }
    convert::append(&mut out, view.messages, target);
    out
}

/// True once the last reply left less room than a full reply needs; the UI's meter uses the same sum.
pub(super) fn overflowing(model: &Model, transcript: &[MessageWithParts]) -> bool {
    if model.limit.context == 0 {
        return false;
    }
    let reply = match model.limit.output {
        0 => MAX_REPLY_TOKENS,
        output => output.min(MAX_REPLY_TOKENS),
    };
    last_usage(transcript) >= model.limit.context.saturating_sub(reply)
}

/// Tokens the most recent finished reply used, counting only what came after the latest summary.
fn last_usage(transcript: &[MessageWithParts]) -> u64 {
    for message in transcript.iter().rev() {
        if message.info.summary {
            return 0;
        }
        let usage = message.info.usage;
        let total = usage.input + usage.output + usage.cache_read + usage.cache_write;
        if message.info.role == Role::Assistant && message.info.status == MessageStatus::Done && total > 0 {
            return total;
        }
    }
    0
}

impl Engine {
    pub fn auto_compact(&self) -> bool {
        self.store.setting(AUTO_COMPACT_KEY).ok().flatten().unwrap_or(true)
    }

    /// Compacts an idle session as its own job, so prompts wait for it and Stop cancels it. Refused while
    /// undone: the summary would land after the hidden messages the next prompt deletes.
    pub fn start_compaction(self: &Arc<Self>, session_id: &str) -> Result<(), TurnError> {
        let abort = CancellationToken::new();
        if !self.turns.claim(session_id, &abort) {
            return Err(TurnError::Busy);
        }
        let session = match self.store.session(session_id) {
            Ok(Some(session)) => session,
            Ok(None) => return self.refuse_compaction(session_id, TurnError::NoSession),
            Err(error) => return self.refuse_compaction(session_id, error.into()),
        };
        if session.revert.is_some() {
            return self.refuse_compaction(session_id, TurnError::Reverted);
        }
        let engine = self.clone();
        let id = session_id.to_string();
        self.spawn_job(session_id, async move {
            let _ = engine.compact(&id, Trigger::Manual, &abort).await;
        });
        Ok(())
    }

    fn refuse_compaction(&self, session_id: &str, error: TurnError) -> Result<(), TurnError> {
        self.turns.release(session_id);
        Err(error)
    }

    /// Whether the turn should compact before its next request; three automatic failures in a row stop it.
    pub(super) fn wants_compaction(&self, session_id: &str, model: &Model, transcript: &[MessageWithParts]) -> bool {
        let failures = self.turns.compaction_failures.lock().unwrap().get(session_id).copied().unwrap_or(0);
        self.auto_compact() && failures < MAX_AUTO_FAILURES && overflowing(model, transcript)
    }

    /// Writes a boundary and a summary of everything before the recent turns. The summary message
    /// records failure or abort; the caller decides what that means for the turn.
    pub(super) async fn compact(self: &Arc<Self>, session_id: &str, trigger: Trigger, abort: &CancellationToken) -> Result<(), String> {
        let result = self.compact_once(session_id, trigger, abort).await;
        if trigger != Trigger::Manual {
            let mut failures = self.turns.compaction_failures.lock().unwrap();
            match &result {
                Ok(()) => {
                    failures.remove(session_id);
                }
                Err(_) => *failures.entry(session_id.into()).or_default() += 1,
            }
        }
        result
    }

    async fn compact_once(&self, session_id: &str, trigger: Trigger, abort: &CancellationToken) -> Result<(), String> {
        let (resolved, config) = self.action_model(session_id, "compaction", Fallback::Conversation).await.map_err(|e| e.to_string())?;
        let instructions = config.agent("compaction").map(|agent| agent.prompt.clone()).unwrap_or_default();
        let transcript = self.store.transcript(session_id).map_err(|e| e.to_string())?;
        let view = view(&transcript);
        let tail = tail_start(&view.messages);
        let head = &view.messages[..tail.unwrap_or(view.messages.len())];
        if head.is_empty() && view.summary.is_none() {
            return Err("there is nothing to compact yet".into());
        }
        let tail_from = tail.map(|i| view.messages[i].info.id.clone());
        let mut summary = self.open_compaction(session_id, &resolved.model_ref, trigger, tail_from).map_err(|e| e.to_string())?;
        let outcome = tokio::select! {
            outcome = self.summarise(&resolved, &instructions, view.summary.as_deref(), head) => outcome,
            () = abort.cancelled() => Err("aborted".to_string()),
        };
        self.close_compaction(&mut summary, outcome, abort.is_cancelled())
    }

    /// The boundary the UI draws and the streaming summary message it fills.
    fn open_compaction(&self, session_id: &str, model: &ModelRef, trigger: Trigger, tail_from: Option<String>) -> rusqlite::Result<Message> {
        let boundary = self.store.create_message(session_id, Role::User, Some(model))?;
        self.hub.publish(Event::MessageCreated { message: boundary.clone() });
        let part = self.store.add_part(&boundary.id, session_id, Part::Compaction { auto: trigger != Trigger::Manual, tail_from })?;
        self.hub.publish(Event::PartCreated { part });
        let summary = self.store.create_summary_message(session_id, model)?;
        self.hub.publish(Event::MessageCreated { message: summary.clone() });
        Ok(summary)
    }

    /// Publishes a finished summary only once its text and state are stored together. Anything less
    /// leaves the summary failed, the previous request view in use, and the failure with the caller.
    fn close_compaction(&self, summary: &mut Message, outcome: Result<String, String>, aborted: bool) -> Result<(), String> {
        summary.finished_at = Some(id::now_ms());
        let stored = outcome.and_then(|text| {
            summary.status = MessageStatus::Done;
            self.store.complete_summary(summary, text.trim()).map_err(|e| format!("the summary was not saved ({e})"))
        });
        let error = match stored {
            Ok(part) => {
                self.hub.publish(Event::PartCreated { part });
                self.hub.publish(Event::MessageUpdated { message: summary.clone() });
                return Ok(());
            }
            Err(error) => error,
        };
        summary.status = if aborted { MessageStatus::Aborted } else { MessageStatus::Error };
        summary.error = Some(error.clone());
        let _ = self.store.save_message(summary);
        self.hub.publish(Event::MessageUpdated { message: summary.clone() });
        Err(error)
    }

    /// One summary request; when it is itself too long, the oldest turns are dropped and it is retried.
    async fn summarise(&self, resolved: &Resolved, instructions: &str, previous: Option<&str>, head: &[&MessageWithParts]) -> Result<String, String> {
        let starts = turn_starts(head);
        let mut dropped = 0;
        for attempt in 0..=TRIM_ATTEMPTS {
            let from = starts.get(dropped).copied().unwrap_or(head.len());
            let mut messages = Vec::new();
            if let Some(previous) = previous {
                convert::push(&mut messages, llm::Role::User, vec![Block::Text(wrap(previous))]);
            }
            if dropped > 0 {
                convert::push(&mut messages, llm::Role::User, vec![Block::Text("(The oldest part of the conversation was left out to fit.)".into())]);
            }
            convert::append(&mut messages, head[from..].iter().copied(), &resolved.model_ref);
            convert::push(&mut messages, llm::Role::User, vec![Block::Text(instructions.into())]);
            let shot = OneShot { system: String::new(), messages, tools: self.tool_specs(resolved.model.profile), max_tokens: SUMMARY_MAX_TOKENS, timeout: SUMMARY_TIMEOUT };
            match self.complete(resolved, shot).await {
                Err(error) if attempt < TRIM_ATTEMPTS && llm::mentions_context_overflow(&error) && dropped < starts.len() => {
                    dropped += (starts.len() / 5).max(1);
                }
                other => return other,
            }
        }
        Err("the conversation is too long to summarise".into())
    }
}

/// The summary as the model's opening context.
fn wrap(summary: &str) -> String {
    format!(
        "This conversation was compacted to fit the context window. Summary of the earlier part:\n\n{summary}\n\n\
         The messages that follow are the most recent ones, verbatim. If the work is unfinished, continue it."
    )
}

fn is_marker(message: &MessageWithParts) -> bool {
    message.info.summary || boundary(message).is_some()
}

/// `Some(tail_from)` for a compaction boundary.
fn boundary(message: &MessageWithParts) -> Option<Option<String>> {
    message.parts.iter().find_map(|row| match &row.part {
        Part::Compaction { tail_from, .. } => Some(tail_from.clone()),
        _ => None,
    })
}

fn text_of(message: &MessageWithParts) -> String {
    message.parts.iter().filter_map(|row| match &row.part { Part::Text { text } => Some(text.as_str()), _ => None }).collect::<Vec<_>>().join("\n")
}

fn turn_starts(messages: &[&MessageWithParts]) -> Vec<usize> {
    messages.iter().enumerate().filter(|(_, m)| m.info.role == Role::User).map(|(i, _)| i).collect()
}

/// Where the verbatim tail begins: whole turns from the end, within both limits, always leaving
/// something before it to summarise. `None` summarises everything.
fn tail_start(messages: &[&MessageWithParts]) -> Option<usize> {
    let mut chosen = None;
    for start in turn_starts(messages).into_iter().rev().take(TAIL_TURNS) {
        if start == 0 || estimate(&messages[start..]) > TAIL_TOKENS {
            break;
        }
        chosen = Some(start);
    }
    chosen
}

/// Roughly four characters a token; close enough to budget a tail.
fn estimate(messages: &[&MessageWithParts]) -> usize {
    let chars: usize = messages
        .iter()
        .flat_map(|m| m.parts.iter())
        .map(|row| match &row.part {
            Part::Text { text } | Part::Reasoning { text, .. } | Part::TaskResult { text, .. } => text.len(),
            Part::ToolCall { input, output, .. } => input.to_string().len() + output.as_ref().map_or(0, String::len),
            Part::File { url, .. } => url.len(),
            Part::Compaction { .. } => 0,
        })
        .sum();
    chars / 4
}

#[cfg(test)]
mod tests;
