//! Compaction: when a conversation outgrows its context, a summary stands in for its older history.
//! Nothing is deleted; the transcript keeps every message and the request view skips what was summarised.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::convert;
use super::oneshot::{Action, Answer, Failure, Fallback, OneShot};
use super::turn::Plan;
use super::turn::TurnError;
use super::types::{Message, MessageStatus, MessageWithParts, ModelRef, Part, Role, Session};
use crate::Engine;
use crate::event::Event;
use crate::id;
use crate::llm::catalog::Model;
use crate::llm::{self, Block, ChatMessage};

/// The recent history kept verbatim: at most this many turns, within [`tail_budget`] estimated tokens.
const TAIL_TURNS: usize = 2;
/// The tail budget is a quarter of the model's compaction point, clamped to these bounds.
/// The minimum stops a small local model from compacting again on the next step.
/// opencode caps the tail at 8k tokens; Drift keeps its earlier 15k cap for large windows.
const TAIL_MIN_TOKENS: u64 = 2_000;
const TAIL_MAX_TOKENS: u64 = 15_000;
/// Character limit for each tool result in a summary request; images and PDFs are replaced by a mention.
const SUMMARY_TOOL_CHARS: usize = 2_000;
const SUMMARY_MAX_TOKENS: u32 = 8_192;
const CACHE_WARM_MS: i64 = 5 * 60 * 1000;
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(300);
/// Retries for an overlong summary request, each dropping the oldest fifth of its turns.
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

#[derive(Debug, thiserror::Error)]
pub(super) enum CompactionError {
    #[error("{0}")]
    Turn(#[from] TurnError),
    #[error("{0}")]
    Store(#[from] rusqlite::Error),
    #[error("the conversation could not be read")]
    Unreadable,
    #[error("there is nothing to compact yet")]
    Empty,
    #[error("aborted")]
    Aborted,
    #[error("the summary was not saved ({0})")]
    Unsaved(rusqlite::Error),
    #[error("{0}")]
    Request(String),
    #[error("the conversation is too long to summarise")]
    TooLong,
}

struct SummaryInput<'a> {
    session_id: &'a str,
    action: &'a Action,
    instructions: &'a str,
    window: &'a [MessageWithParts],
    trigger: Trigger,
    previous: Option<&'a str>,
    head: &'a [&'a MessageWithParts],
}

/// What the model sees: the latest finished summary, then the messages it kept and everything after.
pub(super) struct View<'a> {
    pub summary: Option<String>,
    /// The original prompt of a turn split by the tail, sent verbatim beside its summary.
    pub request: Option<String>,
    pub messages: Vec<&'a MessageWithParts>,
}

pub(super) fn view(transcript: &[MessageWithParts]) -> View<'_> {
    // An empty summary replaces nothing, so the previous view stays in effect.
    let latest = transcript.iter().rposition(|message| {
        message.info.summary && message.info.status == MessageStatus::Done && !text_of(message).trim().is_empty()
    });
    let Some(index) = latest else {
        return View {
            summary: None,
            request: None,
            messages: transcript.iter().filter(|message| !is_marker(message)).collect(),
        };
    };

    let tail_from = transcript[..index].iter().rev().find_map(boundary).flatten();
    let messages: Vec<&MessageWithParts> = transcript
        .iter()
        .enumerate()
        .filter(|(position, message)| {
            !is_marker(message)
                && tail_from
                    .as_ref()
                    .map_or(*position > index, |tail| message.info.id >= *tail)
        })
        .map(|(_, message)| message)
        .collect();

    let request = messages
        .first()
        .filter(|first| first.info.role == Role::Assistant)
        .and_then(|first| {
            let prompt = transcript.iter().rev().find(|message| {
                message.info.id < first.info.id && message.info.role == Role::User && !is_marker(message)
            })?;
            Some(text_of(prompt)).filter(|text| !text.trim().is_empty())
        });

    View {
        summary: Some(text_of(&transcript[index])),
        request,
        messages,
    }
}

/// Builds request history for `target`, opening with the summary, any prompt it split and `lead` reminders.
/// The messages the summary kept follow unchanged.
pub(super) fn request_messages(
    transcript: &[MessageWithParts],
    target: &impl convert::Target,
    lead: &[String],
) -> Vec<ChatMessage> {
    let view = view(transcript);
    let mut out = Vec::new();

    if let Some(summary) = &view.summary {
        let request = view
            .request
            .as_ref()
            .map(|text| format!("The request still being worked on, as the user wrote it:\n\n{text}"));
        let blocks = std::iter::once(wrap(summary))
            .chain(request)
            .chain(lead.iter().cloned())
            .map(Block::Text)
            .collect();
        convert::push(&mut out, llm::Role::User, blocks);
    }

    convert::append(&mut out, view.messages, target);
    out
}

/// True once the last reply left less room than a full reply needs; the UI's meter uses the same sum.
pub(super) fn overflowing(model: &Model, transcript: &[MessageWithParts]) -> bool {
    if model.limit.context == 0 {
        return false;
    }

    let used = last_usage(transcript);
    used > 0 && used >= model.compaction_point()
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

    /// Compacts an idle session as its own job, so prompts wait for it and Stop cancels it.
    /// Refused while undone, because the summary would follow hidden messages the next prompt deletes.
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
        let failures = self
            .turns
            .compaction_failures
            .lock()
            .unwrap()
            .get(session_id)
            .copied()
            .unwrap_or(0);

        self.auto_compact() && failures < MAX_AUTO_FAILURES && overflowing(model, transcript)
    }

    /// Writes a boundary and a summary of everything before the recent turns.
    /// The summary message records failure or abort; the caller decides what that means for the turn.
    pub(super) async fn compact(
        self: &Arc<Self>,
        session_id: &str,
        trigger: Trigger,
        abort: &CancellationToken,
    ) -> Result<(), CompactionError> {
        let result = self.compact_once(session_id, trigger, abort).await;
        if result.is_ok() {
            self.turns.files_for(&self.store, session_id).forget_shown();
            self.turns.forget_checked(session_id);
        }

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

    async fn compact_once(
        &self,
        session_id: &str,
        trigger: Trigger,
        abort: &CancellationToken,
    ) -> Result<(), CompactionError> {
        let action = self
            .action_model(session_id, "compaction", Fallback::Conversation)
            .await?;
        let session = self.store.session(session_id).ok().flatten();
        let instructions = self.compaction_instructions(&action, session.as_ref()).await;

        // Only the current view is summarised again; history the last summary covered is not loaded.
        let transcript = self.request_window(session_id).ok_or(CompactionError::Unreadable)?;
        let view = view(&transcript);
        let tail = tail_start(&view.messages, tail_budget(&action.conversation));
        let head = &view.messages[..tail.unwrap_or(view.messages.len())];
        if head.is_empty() && view.summary.is_none() {
            return Err(CompactionError::Empty);
        }

        let tail_from = tail.map(|index| view.messages[index].info.id.clone());
        let mut summary = self.open_compaction(session_id, &action.resolved.model_ref, trigger, tail_from)?;
        let previous = previous_summary(&view);
        let mut spent = Spent::default();
        let input = SummaryInput {
            session_id,
            action: &action,
            instructions: &instructions,
            window: &transcript,
            trigger,
            previous: previous.as_deref(),
            head,
        };
        let outcome = tokio::select! {
            outcome = self.summarise(&input, &mut spent) => outcome,
            () = abort.cancelled() => Err(CompactionError::Aborted),
        };

        // Every attempt is charged like a reply, including attempts that came back unusable.
        summary.usage = spent.usage;
        summary.cost = spent.cost;
        let closed = self.close_compaction(&mut summary, outcome, abort.is_cancelled());

        if let Some(session) = session.filter(|_| closed.is_ok() && !self.hooks.is_empty()) {
            self.hooks
                .session(&crate::hook::session_event(
                    self,
                    &session,
                    crate::hook::SessionKind::Compacted,
                ))
                .await;
        }

        closed
    }

    /// The compaction agent's prompt, followed by any instructions plugins add for this session.
    async fn compaction_instructions(&self, action: &Action, session: Option<&Session>) -> String {
        let mut instructions = action
            .config
            .agent("compaction")
            .map(|agent| agent.prompt.clone())
            .unwrap_or_default();
        let Some(session) = session.filter(|_| !self.hooks.is_empty()) else {
            return instructions;
        };

        let event = crate::hook::CompactionEvent {
            session_id: session.id.clone(),
            workspace: action.workspace.to_string_lossy().into_owned(),
            agent: session.agent.clone(),
        };
        for extra in self.hooks.compaction(&event).await {
            instructions.push_str("\n\n");
            instructions.push_str(&extra);
        }

        instructions
    }

    /// The boundary the UI draws and the streaming summary message it fills.
    fn open_compaction(
        &self,
        session_id: &str,
        model: &ModelRef,
        trigger: Trigger,
        tail_from: Option<String>,
    ) -> rusqlite::Result<Message> {
        let boundary = self.store.create_message(session_id, Role::User, Some(model))?;
        self.hub.publish(Event::MessageCreated {
            message: boundary.clone(),
        });

        let part = self.store.add_part(
            &boundary.id,
            session_id,
            Part::Compaction {
                auto: trigger != Trigger::Manual,
                tail_from,
            },
        )?;
        self.hub.publish(Event::PartCreated { part });

        let summary = self.store.create_summary_message(session_id, model)?;
        self.hub.publish(Event::MessageCreated {
            message: summary.clone(),
        });

        Ok(summary)
    }

    /// Publishes a finished summary only after its text and state are stored together.
    /// Otherwise the summary is marked failed, the previous view stays in use, and the error is returned.
    fn close_compaction(
        &self,
        summary: &mut Message,
        outcome: Result<String, CompactionError>,
        aborted: bool,
    ) -> Result<(), CompactionError> {
        summary.finished_at = Some(id::now_ms());
        let stored = outcome.and_then(|text| {
            summary.status = MessageStatus::Done;
            self.store
                .complete_summary(summary, text.trim())
                .map_err(CompactionError::Unsaved)
        });
        let error = match stored {
            Ok(part) => {
                self.hub.publish(Event::PartCreated { part });
                self.hub.publish(Event::MessageUpdated {
                    message: summary.clone(),
                });
                return Ok(());
            }
            Err(error) => error,
        };

        summary.status = if aborted {
            MessageStatus::Aborted
        } else {
            MessageStatus::Error
        };
        summary.error = Some(error.to_string());
        let _ = self.store.save_message(summary);
        self.hub.publish(Event::MessageUpdated {
            message: summary.clone(),
        });

        Err(error)
    }

    /// Prefers the conversation's next request, with the instructions appended, while its model's cache is warm.
    /// That reads the whole history at the cached price.
    /// Otherwise, or when that reply is unusable or too long, summarises the history before the tail leanly.
    async fn summarise(&self, input: &SummaryInput<'_>, spent: &mut Spent) -> Result<String, CompactionError> {
        if let Some(plan) = input
            .action
            .own
            .as_ref()
            .filter(|_| input.trigger != Trigger::Overflow && warm(input.window))
        {
            match self
                .summarise_cached(input.session_id, plan, input.instructions, input.window.to_vec())
                .await
            {
                Ok(answer) => return Ok(spent.take(&plan.model, answer)),
                Err(failure) => {
                    spent.add(&plan.model, failure.usage());
                    if !failure.retry_another_way() {
                        return Err(CompactionError::Request(failure.message()));
                    }
                }
            }
        }

        self.summarise_lean(input, spent).await
    }

    /// Sends exactly the request the turn's next step would, with the instructions as the last user message.
    /// Frame, reasoning and tool choice stay identical because providers key their prompt cache on them.
    async fn summarise_cached(
        &self,
        session_id: &str,
        plan: &Plan,
        instructions: &str,
        window: Vec<MessageWithParts>,
    ) -> Result<Answer, Failure> {
        let started = self
            .turns
            .began(session_id)
            .or_else(|| self.store.newest_prompt(session_id).ok().flatten());
        let (mut request, _) = self.step_request(plan, window, started.as_deref(), None);
        convert::push(
            &mut request.messages,
            llm::Role::User,
            vec![Block::Text(instructions.into())],
        );

        self.send(
            &plan.provider,
            &plan.credential,
            &request,
            super::oneshot::SendOptions {
                timeout: SUMMARY_TIMEOUT,
                shown_in: Some(session_id),
            },
        )
        .await
    }

    /// Summarises the history before the tail without a system prompt, files mentioned and tool results cut.
    /// When that request is too long, drops the oldest turns and asks again.
    async fn summarise_lean(&self, input: &SummaryInput<'_>, spent: &mut Spent) -> Result<String, CompactionError> {
        let resolved = &input.action.resolved;
        let starts = turn_starts(input.head);
        let mut dropped = 0;

        for attempt in 0..=TRIM_ATTEMPTS {
            let from = starts.get(dropped).copied().unwrap_or(input.head.len());
            let shot = OneShot {
                system: String::new(),
                messages: self.lean_messages(input, from, dropped > 0),
                tools: self.tool_specs(resolved.model.profile, Some(&input.action.workspace)),
                max_tokens: SUMMARY_MAX_TOKENS,
                timeout: SUMMARY_TIMEOUT,
                shown_in: Some(input.session_id.into()),
            };
            let failure = match self.complete(resolved, shot).await {
                Ok(answer) => return Ok(spent.take(&resolved.model, answer)),
                Err(failure) => failure,
            };

            spent.add(&resolved.model, failure.usage());
            let error = failure.message();
            if attempt == TRIM_ATTEMPTS || !llm::mentions_context_overflow(&error) || dropped >= starts.len() {
                return Err(CompactionError::Request(error));
            }
            dropped += (starts.len() / 5).max(1);
        }

        Err(CompactionError::TooLong)
    }

    /// The lean summary request for the head from message `from`, noting when older turns were left out.
    fn lean_messages(&self, input: &SummaryInput<'_>, from: usize, trimmed: bool) -> Vec<ChatMessage> {
        let mut messages = Vec::new();
        if let Some(previous) = input.previous {
            convert::push(&mut messages, llm::Role::User, vec![Block::Text(wrap(previous))]);
        }
        if trimmed {
            let note = "(The oldest part of the conversation was left out to fit.)";
            convert::push(&mut messages, llm::Role::User, vec![Block::Text(note.into())]);
        }

        let catalog = self.catalog.read().unwrap();
        let target = convert::OnCatalog {
            model: &input.action.resolved.model_ref,
            catalog: &catalog,
            account: input.action.resolved.account.as_deref(),
        };
        convert::append(&mut messages, input.head[from..].iter().copied(), &target);

        lean(&mut messages);
        convert::push(
            &mut messages,
            llm::Role::User,
            vec![Block::Text(input.instructions.into())],
        );

        messages
    }
}

/// The previous summary, carrying a split turn's prompt so a second compaction does not lose it.
fn previous_summary(view: &View<'_>) -> Option<String> {
    let summary = view.summary.as_ref()?;
    let previous = match &view.request {
        Some(request) => format!("{summary}\n\nThe request still being worked on, as the user wrote it:\n\n{request}"),
        None => summary.clone(),
    };

    Some(previous)
}

/// What a compaction's requests used and cost, each priced on the model it ran on.
#[derive(Default)]
struct Spent {
    usage: crate::session::types::Usage,
    cost: f64,
}

impl Spent {
    fn add(&mut self, model: &Model, usage: crate::session::types::Usage) {
        self.cost += super::turn::cost(model, usage);
        self.usage.input += usage.input;
        self.usage.output += usage.output;
        self.usage.cache_read += usage.cache_read;
        self.usage.cache_write += usage.cache_write;
    }

    /// Counts a reply's usage and hands back its text.
    fn take(&mut self, model: &Model, answer: Answer) -> String {
        self.add(model, answer.usage);
        answer.text
    }
}

/// What a summary needs of the history: images and PDFs only by mention, each tool result's start.
fn lean(messages: &mut [ChatMessage]) {
    for block in messages.iter_mut().flat_map(|message| message.blocks.iter_mut()) {
        match block {
            Block::Image { mime, .. } | Block::Stored { mime, .. } => {
                *block = Block::Text(format!("[a {mime} file was attached here]"));
            }
            Block::Pdf { .. } => *block = Block::Text("[a PDF was attached here]".into()),
            Block::ToolResult { content, .. } if content.chars().count() > SUMMARY_TOOL_CHARS => {
                let kept: String = content.chars().take(SUMMARY_TOOL_CHARS).collect();
                *content = format!("{kept}\n[... cut for the summary]");
            }
            _ => {}
        }
    }
}

/// Whether the last reply is recent enough for the provider to still hold its prompt cache.
/// Five minutes is Anthropic's default and the shortest common cache lifetime.
fn warm(window: &[MessageWithParts]) -> bool {
    let last = window
        .iter()
        .rev()
        .find(|message| message.info.role == Role::Assistant && !message.info.summary);

    last.and_then(|message| message.info.finished_at)
        .is_some_and(|at| id::now_ms() - at < CACHE_WARM_MS)
}

/// How much recent history stays verbatim, in estimated tokens, for the model that will read it.
fn tail_budget(model: &Model) -> usize {
    usize::try_from((model.compaction_point() / 4).clamp(TAIL_MIN_TOKENS, TAIL_MAX_TOKENS)).unwrap_or(usize::MAX)
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
    message
        .parts
        .iter()
        .filter_map(|row| match &row.part {
            Part::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn turn_starts(messages: &[&MessageWithParts]) -> Vec<usize> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.info.role == Role::User)
        .map(|(index, _)| index)
        .collect()
}

/// Where the verbatim tail begins: whole turns from the end within both limits, leaving something to summarise.
/// When even the last turn does not fit, its newest steps are kept from a reply onward.
/// `None` summarises everything.
fn tail_start(messages: &[&MessageWithParts], budget: usize) -> Option<usize> {
    let starts = turn_starts(messages);
    let mut chosen = None;
    for &start in starts.iter().rev().take(TAIL_TURNS) {
        if start == 0 || estimate(&messages[start..]) > budget {
            break;
        }
        chosen = Some(start);
    }

    chosen.or_else(|| split_turn(messages, starts.last().copied().unwrap_or(0), budget))
}

/// The earliest reply of the turn starting at `turn` from which the rest fits the tail budget.
fn split_turn(messages: &[&MessageWithParts], turn: usize, budget: usize) -> Option<usize> {
    let mut size = 0;
    let mut chosen = None;
    for index in (turn + 1..messages.len()).rev() {
        size += estimate(&messages[index..=index]);
        if size > budget {
            break;
        }
        if messages[index].info.role == Role::Assistant {
            chosen = Some(index);
        }
    }

    chosen
}

/// Roughly four characters a token; close enough to budget a tail.
fn estimate(messages: &[&MessageWithParts]) -> usize {
    let chars: usize = messages
        .iter()
        .flat_map(|message| message.parts.iter())
        .map(|row| match &row.part {
            Part::Text { text }
            | Part::Nudge { text }
            | Part::Context { text, .. }
            | Part::Reasoning { text, .. }
            | Part::TaskResult { text, .. } => text.len(),
            Part::ToolCall { input, output, .. } => input.to_string().len() + output.as_ref().map_or(0, String::len),
            Part::File { url, .. } => url.len(),
            Part::Clarification { request_id, items } => super::convert::clarification_text(request_id, items).len(),
            Part::Compaction { .. } | Part::Unknown { .. } => 0,
        })
        .sum();

    chars / 4
}

#[cfg(test)]
mod tests;
