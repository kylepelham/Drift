//! Workers the model launches with `task`. A foreground worker is waited for; a background one runs on
//! under the engine while the conversation goes on, and its result is handed back once, at a point
//! where the parent can take it. Parent link and permissions are the worker's; stopping follows the
//! owner, never the launching turn.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;

use super::turn::{Prompt, TurnEnd, TurnError};
use super::types::{MessageStatus, Part, Role};
use crate::event::Event;
use crate::Engine;

/// Background workers running at once across the engine; more wait their turn.
pub const MAX_BACKGROUND: usize = 4;
pub const BACKGROUND_TASKS_KEY: &str = "backgroundTasks";
/// How much of a worker's final reply comes back verbatim.
const RESULT_CHARS: usize = 20_000;
/// The longest `task_output` may wait for a worker to finish.
pub const MAX_WAIT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Foreground,
    Background,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Foreground => "foreground",
            Self::Background => "background",
        }
    }

    pub fn parse(text: &str) -> Self {
        if text == "background" {
            Self::Background
        } else {
            Self::Foreground
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Launched in the background and waiting for a free slot.
    Queued,
    Running,
    Replied,
    Failed,
    Stopped,
    /// The engine stopped while it ran; it is never rerun by itself.
    Interrupted,
}

impl TaskState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Replied => "replied",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
            Self::Interrupted => "interrupted",
        }
    }

    pub fn parse(text: &str) -> Self {
        match text {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "replied" => Self::Replied,
            "stopped" => Self::Stopped,
            "interrupted" => Self::Interrupted,
            _ => Self::Failed,
        }
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
    }
}

/// One worker: who launched it, how it runs and why, how it ended, and whether its parent has it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskRecord {
    pub id: String,
    pub parent_session_id: String,
    /// The worker's own transcript.
    pub session_id: String,
    /// The `task` call that launched it.
    pub call_id: String,
    pub description: String,
    pub agent: String,
    pub mode: Mode,
    /// Why this mode: `requested`, `agent default`, `default` or `background turned off`.
    pub reason: String,
    pub state: TaskState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// The result reached the parent (as the call's own result, or delivered later).
    pub delivered: bool,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
}

/// How a worker runs, and why. An explicit choice wins; otherwise the agent's default; otherwise the
/// foreground. Nothing in the prompt's wording decides it.
pub fn resolve_mode(explicit: Option<bool>, agent_default: Option<bool>, enabled: bool) -> Result<(Mode, &'static str), String> {
    match (explicit, agent_default) {
        (Some(true), _) if !enabled => Err("background tasks are turned off in Settings; run this task in the foreground".into()),
        (Some(true), _) => Ok((Mode::Background, "requested")),
        (Some(false), _) => Ok((Mode::Foreground, "requested")),
        (None, Some(true)) if enabled => Ok((Mode::Background, "agent default")),
        (None, Some(true)) => Ok((Mode::Foreground, "background turned off")),
        (None, Some(false)) => Ok((Mode::Foreground, "agent default")),
        (None, None) => Ok((Mode::Foreground, "default")),
    }
}

/// Background workers' shared slots, and each owner's stop scope.
pub struct Workers {
    slots: tokio::sync::Semaphore,
    owners: Mutex<HashMap<String, Owner>>,
}

/// What an owner's background workers run under: a token its Stop cancels, and how many Stops so far.
struct Owner {
    token: CancellationToken,
    generation: u64,
}

impl Default for Workers {
    fn default() -> Self {
        Self { slots: tokio::sync::Semaphore::new(MAX_BACKGROUND), owners: Mutex::default() }
    }
}

impl Workers {
    /// The token a new background worker of `owner` descends from, and the Stop generation it starts in.
    fn scope(&self, owner: &str) -> (CancellationToken, u64) {
        let mut owners = self.owners.lock().unwrap();
        let entry = owners.entry(owner.into()).or_insert_with(|| Owner { token: CancellationToken::new(), generation: 0 });
        (entry.token.clone(), entry.generation)
    }

    fn generation(&self, owner: &str) -> u64 {
        self.owners.lock().unwrap().get(owner).map_or(0, |o| o.generation)
    }

    /// Cancels everything `owner` launched in the background; later launches start a fresh scope.
    fn stop(&self, owner: &str) {
        let mut owners = self.owners.lock().unwrap();
        let entry = owners.entry(owner.into()).or_insert_with(|| Owner { token: CancellationToken::new(), generation: 0 });
        entry.token.cancel();
        entry.token = CancellationToken::new();
        entry.generation += 1;
    }
}

impl Engine {
    pub fn background_enabled(&self) -> bool {
        self.store.setting(BACKGROUND_TASKS_KEY).ok().flatten().unwrap_or(true)
    }

    /// Runs a recorded background task and returns at once; the worker outlives the call that launched it.
    pub fn launch(self: &Arc<Self>, task: TaskRecord, prompt: Prompt) {
        let (scope, generation) = self.workers.scope(&task.parent_session_id);
        let engine = self.clone();
        tokio::spawn(async move { engine.work(task, prompt, scope, generation).await });
    }

    async fn work(self: Arc<Self>, task: TaskRecord, prompt: Prompt, scope: CancellationToken, generation: u64) {
        let permit = tokio::select! {
            permit = self.workers.slots.acquire() => permit.ok(),
            () = scope.cancelled() => None,
        };
        let started = permit.is_some() && self.store.start_task(&task.id).unwrap_or(false);
        if started {
            self.publish_task(&task.id);
            match self.submit_under(&task.session_id, prompt, Some(&scope)).await {
                Ok(_) => {
                    self.turns.wait_idle(&task.session_id, &CancellationToken::new()).await;
                    let (state, text) = self.worker_result(&task.session_id);
                    self.end_task(&task.id, state, &text);
                }
                Err(error) => self.end_task(&task.id, TaskState::Failed, &format!("The subagent could not start: {error}")),
            }
        } else {
            self.end_task(&task.id, TaskState::Stopped, STOPPED);
        }
        drop(permit);
        self.deliver(&task.id, generation).await;
    }

    /// How a worker's turn ended and what it said. A stop wins however late it came; otherwise the
    /// last attempt decides, and an earlier reply never stands in for a later failure.
    pub fn worker_result(&self, session_id: &str) -> (TaskState, String) {
        let attempt = match (self.turns.take_end(session_id), last_attempt(&self.store, session_id)) {
            (Some(TurnEnd::Stopped), _) => Attempt::Stopped,
            (Some(TurnEnd::Failed), Attempt::Replied(_)) => Attempt::Failed("its turn ended without finishing".into()),
            (_, attempt) => attempt,
        };
        match attempt {
            Attempt::Replied(reply) => (TaskState::Replied, clip(&reply, RESULT_CHARS)),
            Attempt::Failed(error) => (TaskState::Failed, format!("The subagent failed: {error}")),
            Attempt::Stopped => (TaskState::Stopped, STOPPED.into()),
            Attempt::None => (TaskState::Failed, "The subagent finished without a reply.".into()),
        }
    }

    /// Records how a worker ended, once; a later ending (a stop racing a reply) changes nothing.
    pub fn end_task(&self, task_id: &str, state: TaskState, text: &str) {
        if self.store.finish_task(task_id, state, text).unwrap_or(false) {
            self.publish_task(task_id);
        }
    }

    pub fn publish_task(&self, task_id: &str) {
        if let Ok(Some(task)) = self.store.task(task_id) {
            self.hub.publish(Event::TaskUpdated { task });
        }
    }

    /// Hands a finished worker's result to its parent once. A reply or failure goes in as an
    /// engine-origin prompt (submission `task:<id>`, so it can land only once): a running turn takes it
    /// at its next request, an idle parent starts a turn for it. A stopped or interrupted worker never
    /// wakes an idle parent, and nothing wakes a parent the user has stopped since the launch.
    pub async fn deliver(self: &Arc<Self>, task_id: &str, generation: u64) {
        let Ok(Some(task)) = self.store.task(task_id) else { return };
        if task.delivered || !task.state.is_terminal() {
            return;
        }
        let owner = &task.parent_session_id;
        let stopped_since = self.workers.generation(owner) != generation;
        let wakes = matches!(task.state, TaskState::Replied | TaskState::Failed);
        if stopped_since || (!wakes && !self.turns.is_steerable(owner)) {
            self.settle_delivery(task_id);
            return;
        }
        let part = Part::TaskResult {
            task_id: task.id.clone(),
            worker_session_id: task.session_id.clone(),
            description: task.description.clone(),
            outcome: task.state.as_str().into(),
            text: task.result.clone().unwrap_or_default(),
        };
        let prompt = Prompt { parts: vec![part], model: None, thinking_budget: None, submission_id: Some(format!("task:{task_id}")) };
        match self.submit(owner, prompt).await {
            Ok(_) | Err(TurnError::SubmissionReused) => self.settle_delivery(task_id),
            // Left undelivered: the next start delivers it.
            Err(error) => eprintln!("drift: task {task_id} result not delivered: {error}"),
        }
    }

    fn settle_delivery(&self, task_id: &str) {
        if self.store.mark_task_delivered(task_id).is_ok() {
            self.publish_task(task_id);
        }
    }

    /// Stops one worker: a queued one never starts, a running one stops as its turn would.
    pub fn stop_task(&self, task_id: &str) -> Result<TaskRecord, TurnError> {
        let task = self.store.task(task_id)?.ok_or(TurnError::NoSession)?;
        match task.state {
            TaskState::Queued => self.end_task(task_id, TaskState::Stopped, STOPPED),
            TaskState::Running => {
                self.abort(&task.session_id);
            }
            _ => {}
        }
        Ok(self.store.task(task_id)?.unwrap_or(task))
    }

    /// Session Stop reaches its background workers too, even with no turn running.
    pub(super) fn stop_workers(&self, owner: &str) -> bool {
        let running = self.store.tasks_of(owner).unwrap_or_default().iter().any(|t| t.mode == Mode::Background && !t.state.is_terminal());
        self.workers.stop(owner);
        running
    }

    /// After a restart, finished results that had not reached their parent are delivered, once. (Workers
    /// that were still going were marked interrupted when the store opened.)
    pub async fn recover_tasks(self: &Arc<Self>) {
        for task in self.store.undelivered_tasks().unwrap_or_default() {
            self.deliver(&task.id, self.workers.generation(&task.parent_session_id)).await;
        }
    }
}

const STOPPED: &str = "The subagent was stopped before it finished.";

/// How a session's last model attempt ended, as its transcript shows it.
pub(crate) enum Attempt {
    Replied(String),
    Failed(String),
    Stopped,
    None,
}

/// Judged by the last attempt alone. A finished summary is bookkeeping and skipped; a stopped or failed
/// one is how the session last ended.
pub(crate) fn last_attempt(store: &crate::store::Store, session_id: &str) -> Attempt {
    let transcript = store.transcript(session_id).unwrap_or_default();
    let Some(last) = transcript.iter().rev().find(|m| m.info.role == Role::Assistant && !(m.info.summary && m.info.status == MessageStatus::Done)) else {
        return Attempt::None;
    };
    match last.info.status {
        MessageStatus::Done => Attempt::Replied(last.parts.iter().filter_map(|row| match &row.part { Part::Text { text } => Some(text.as_str()), _ => None }).collect::<Vec<_>>().join("\n")),
        MessageStatus::Error | MessageStatus::Paused => Attempt::Failed(last.info.error.clone().unwrap_or_else(|| "unknown error".into())),
        MessageStatus::Aborted => Attempt::Stopped,
        MessageStatus::Streaming => Attempt::None,
    }
}

pub(crate) fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.into();
    }
    format!("{}\n\n(truncated)", text.chars().take(max).collect::<String>())
}

#[cfg(test)]
mod tests;
