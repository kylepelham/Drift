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
use super::types::{Ending, MessageStatus, Part, PartRow, Role, ToolStatus};
use crate::Engine;
use crate::event::Event;

/// Background workers running at once across the engine unless Settings say otherwise; more wait their turn.
pub const DEFAULT_BACKGROUND_LIMIT: usize = 4;
/// The most background workers Settings may allow at once.
pub const MAX_BACKGROUND_LIMIT: usize = 16;
pub const BACKGROUND_TASKS_KEY: &str = "backgroundTasks";
pub const BACKGROUND_LIMIT_KEY: &str = "backgroundTaskLimit";
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
    /// Kept from waking a stopped parent; it goes along with the parent's next prompt instead.
    pub held: bool,
    /// Why this owed result has not been handed over yet; it is retried when that may have changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_error: Option<String>,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    /// The owner's Stop count when it was launched.
    #[serde(skip)]
    pub generation: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ModeError {
    #[error("background tasks are turned off in Settings; run this task in the foreground")]
    BackgroundDisabled,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum WorkerAdmissionError {
    #[error("The subagent was stopped before it finished.")]
    Stopped,
    #[error("The subagent could not start: {0}")]
    Plan(TurnError),
}

/// How a worker runs, and why. An explicit choice wins; otherwise the agent's default; otherwise the
/// foreground. Nothing in the prompt's wording decides it.
pub fn resolve_mode(
    explicit: Option<bool>,
    agent_default: Option<bool>,
    enabled: bool,
) -> Result<(Mode, &'static str), ModeError> {
    match (explicit, agent_default) {
        (Some(true), _) if !enabled => Err(ModeError::BackgroundDisabled),
        (Some(true), _) => Ok((Mode::Background, "requested")),
        (Some(false), _) => Ok((Mode::Foreground, "requested")),
        (None, Some(true)) if enabled => Ok((Mode::Background, "agent default")),
        (None, Some(true)) => Ok((Mode::Foreground, "background turned off")),
        (None, Some(false)) => Ok((Mode::Foreground, "agent default")),
        (None, None) => Ok((Mode::Foreground, "default")),
    }
}

/// Shared slots, owners' stop scopes, each worker's own stop, and who is handing each result over.
pub struct Workers {
    slots: tokio::sync::Semaphore,
    limit: Mutex<SlotLimit>,
    /// Also the fence: a Stop and the last check before a prompt is admitted both hold it.
    owners: Mutex<HashMap<String, Owner>>,
    tokens: Mutex<HashMap<String, CancellationToken>>,
    claims: Mutex<HashMap<String, Claim>>,
}

/// What an owner's background workers run under: a token its Stop cancels, and its durable Stop count.
pub(crate) struct Owner {
    token: CancellationToken,
    generation: i64,
}

pub(crate) type Owners = HashMap<String, Owner>;

/// The slot count Settings ask for, and how many slots still held by running workers must be retired to reach it.
struct SlotLimit {
    size: usize,
    owed: usize,
}

/// Why a new background limit was not applied.
#[derive(Debug, thiserror::Error)]
pub enum LimitError {
    #[error("the background task limit must be from 1 to {MAX_BACKGROUND_LIMIT}")]
    OutOfRange,
    #[error(transparent)]
    Store(#[from] rusqlite::Error),
}

/// Who is handing a finished result to its parent. One at a time, so it lands once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Claimant {
    /// The engine, as a prompt to the parent.
    Automatic,
    /// A call of the parent's, as its result (`task_output`, or the foreground `task` call itself).
    Call { session_id: String, call_id: String },
}

impl Claimant {
    pub(crate) fn call(session_id: &str, call_id: &str) -> Self {
        Self::Call {
            session_id: session_id.into(),
            call_id: call_id.into(),
        }
    }
}

impl Workers {
    /// Workers with `limit` slots; [`Workers::resize`] changes it later.
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            slots: tokio::sync::Semaphore::new(limit),
            limit: Mutex::new(SlotLimit { size: limit, owed: 0 }),
            owners: Mutex::default(),
            tokens: Mutex::default(),
            claims: Mutex::default(),
        }
    }

    /// Changes how many workers run at once. Running workers are never stopped: a smaller limit takes
    /// its slots back as they finish, and a larger one starts queued workers at once.
    pub(crate) fn resize(&self, size: usize) {
        let mut limit = self.limit.lock().unwrap();
        let current = limit.size;
        limit.size = size;

        // Growing first cancels slots still owed from an earlier shrink, then adds the rest.
        if size > current {
            let extra = size - current;
            let repaid = extra.min(limit.owed);
            limit.owed -= repaid;
            self.slots.add_permits(extra - repaid);
            return;
        }

        // Shrinking takes free slots now and the rest from workers as they finish.
        let surplus = current - size;
        let taken = self.slots.forget_permits(surplus);
        limit.owed += surplus - taken;
    }

    /// Gives a finished worker's slot back, or retires it while a smaller limit is still owed slots.
    fn release(&self, permit: tokio::sync::SemaphorePermit<'_>) {
        let mut limit = self.limit.lock().unwrap();
        if limit.owed > 0 {
            limit.owed -= 1;
            permit.forget();
        }
    }

    pub(crate) fn fence(&self) -> std::sync::MutexGuard<'_, Owners> {
        self.owners.lock().unwrap()
    }

    /// Gives a worker its own stop before it is queued. A Stop that came first leaves it stopped.
    pub(crate) fn register(&self, task_id: &str, token: &CancellationToken) {
        let mut tokens = self.tokens.lock().unwrap();
        if tokens.get(task_id).is_some_and(CancellationToken::is_cancelled) {
            token.cancel();
        }
        tokens.insert(task_id.into(), token.clone());
    }

    /// Stops one worker wherever it is; a Stop for one not yet registered waits for it.
    pub(crate) fn cancel(&self, task_id: &str) -> bool {
        let mut tokens = self.tokens.lock().unwrap();
        let token = tokens.entry(task_id.into()).or_default();
        let live = !token.is_cancelled();
        token.cancel();
        live
    }

    pub(crate) fn forget(&self, task_id: &str) {
        self.tokens.lock().unwrap().remove(task_id);
    }

    /// Takes the right to hand `task_id`'s result over; `false` if someone else has it, who is then asked to try again.
    pub(crate) fn claim(&self, task_id: &str, claimant: Claimant) -> bool {
        let mut claims = self.claims.lock().unwrap();
        match claims.get_mut(task_id) {
            // A call may claim what it already holds; two automatic attempts never run at once.
            Some(claim) if claim.holder == claimant && claimant != Claimant::Automatic => true,
            Some(claim) => {
                claim.again = true;
                false
            }
            None => {
                claims.insert(
                    task_id.into(),
                    Claim {
                        holder: claimant,
                        again: false,
                    },
                );
                true
            }
        }
    }

    pub(crate) fn holds(&self, task_id: &str, claimant: &Claimant) -> bool {
        self.claims
            .lock()
            .unwrap()
            .get(task_id)
            .is_some_and(|claim| claim.holder == *claimant)
    }

    /// Gives up `claimant`'s claim; `true` if someone found it taken meanwhile and wants another try.
    pub(crate) fn release_where_task(&self, task_id: &str, claimant: &Claimant) -> bool {
        let mut claims = self.claims.lock().unwrap();
        match claims.get(task_id) {
            Some(claim) if claim.holder == *claimant => claims.remove(task_id).is_some_and(|claim| claim.again),
            _ => false,
        }
    }

    /// Gives up every claim `matches` selects; returns the tasks given up.
    fn release_where(&self, matches: impl Fn(&Claimant) -> bool) -> Vec<String> {
        let mut claims = self.claims.lock().unwrap();
        let released: Vec<String> = claims
            .iter()
            .filter(|(_, claim)| matches(&claim.holder))
            .map(|(task, _)| task.clone())
            .collect();
        for task in &released {
            claims.remove(task);
        }
        released
    }
}

/// Who holds a result, and whether anyone asked for it while it was held.
struct Claim {
    holder: Claimant,
    again: bool,
}

/// The owner's entry, its Stop count read from the store the first time it is needed.
fn owner_entry<'a>(owners: &'a mut Owners, store: &crate::store::Store, owner: &str) -> &'a mut Owner {
    owners.entry(owner.into()).or_insert_with(|| Owner {
        token: CancellationToken::new(),
        generation: store.stop_generation(owner).unwrap_or(0),
    })
}

/// The background limit saved in Settings, or the default when none is saved or it is out of range.
pub(crate) fn stored_background_limit(store: &crate::store::Store) -> usize {
    let saved: Option<usize> = store.setting(BACKGROUND_LIMIT_KEY).ok().flatten();
    saved
        .filter(|limit| (1..=MAX_BACKGROUND_LIMIT).contains(limit))
        .unwrap_or(DEFAULT_BACKGROUND_LIMIT)
}

impl Engine {
    pub fn background_enabled(&self) -> bool {
        self.store.setting(BACKGROUND_TASKS_KEY).ok().flatten().unwrap_or(true)
    }

    /// How many background workers run at once.
    pub fn background_limit(&self) -> usize {
        stored_background_limit(&self.store)
    }

    /// Saves a new background limit and applies it to the slots straight away.
    pub fn set_background_limit(&self, limit: usize) -> Result<(), LimitError> {
        if !(1..=MAX_BACKGROUND_LIMIT).contains(&limit) {
            return Err(LimitError::OutOfRange);
        }

        self.store.set_setting(BACKGROUND_LIMIT_KEY, &limit)?;
        self.workers.resize(limit);
        Ok(())
    }

    /// The token a new background worker of `owner` descends from, and the owner's Stop count now.
    pub(crate) fn worker_scope(&self, owner: &str) -> (CancellationToken, i64) {
        let mut owners = self.workers.fence();
        let entry = owner_entry(&mut owners, &self.store, owner);
        (entry.token.clone(), entry.generation)
    }

    /// The owner's scope if it has not been stopped since `generation`, which a restart does not reset.
    pub(super) fn scope_at(&self, owner: &str, generation: i64) -> Option<CancellationToken> {
        let mut owners = self.workers.fence();
        let entry = owner_entry(&mut owners, &self.store, owner);
        (entry.generation == generation).then(|| entry.token.clone())
    }

    /// Plans a worker under its own token, then queues it or runs it to the end; `Err` is why it could not start.
    pub(crate) async fn admit_worker(
        self: &Arc<Self>,
        task: &TaskRecord,
        prompt: Prompt,
        token: CancellationToken,
    ) -> Result<Option<&'static str>, WorkerAdmissionError> {
        self.workers.register(&task.id, &token);
        let planned = tokio::select! {
            planned = self.plan(&task.session_id, &prompt) => planned,
            () = token.cancelled() => Err(TurnError::Stopped),
        };
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => {
                let (state, failure) = if error == TurnError::Stopped {
                    (TaskState::Stopped, WorkerAdmissionError::Stopped)
                } else {
                    (TaskState::Failed, WorkerAdmissionError::Plan(error))
                };
                let text = failure.to_string();
                self.end_task(&task.id, state, &text);
                self.workers.forget(&task.id);
                return Err(failure);
            }
        };
        if task.mode == Mode::Foreground {
            return Ok(Some(self.run_worker(task, prompt, plan, &token).await));
        }
        let (engine, task) = (self.clone(), task.clone());
        tokio::spawn(async move { engine.work(task, prompt, plan, token).await });
        Ok(None)
    }

    /// A queued worker: waits for a slot, then runs unless it was stopped meanwhile, however the wait ended.
    async fn work(
        self: Arc<Self>,
        task: TaskRecord,
        prompt: Prompt,
        plan: super::turn::Plan,
        token: CancellationToken,
    ) {
        let permit = tokio::select! {
            permit = self.workers.slots.acquire() => permit.ok(),
            () = token.cancelled() => None,
        };
        let dispatch = permit.is_some() && !token.is_cancelled() && self.store.start_task(&task.id).unwrap_or(false);
        if dispatch {
            self.publish_task(&task.id);
            self.run_worker(&task, prompt, plan, &token).await;
        } else {
            self.end_task(&task.id, TaskState::Stopped, STOPPED);
            self.workers.forget(&task.id);
        }
        if let Some(permit) = permit {
            self.workers.release(permit);
        }
        self.deliver(&task.id).await;
    }

    /// Runs an admitted worker's turn to its end under its token, records how it ended, and returns the outcome.
    async fn run_worker(
        self: &Arc<Self>,
        task: &TaskRecord,
        prompt: Prompt,
        plan: super::turn::Plan,
        token: &CancellationToken,
    ) -> &'static str {
        let (state, text, outcome) = match self.submit_planned(&task.session_id, prompt, plan, token).await {
            Ok(_) => {
                self.turns.wait_idle(&task.session_id, &CancellationToken::new()).await;
                self.worker_result(&task.session_id)
            }
            Err(TurnError::Stopped) => (TaskState::Stopped, STOPPED.to_string(), "stopped"),
            Err(error) => (
                TaskState::Failed,
                format!("The subagent could not start: {error}"),
                "failed",
            ),
        };
        self.end_task(&task.id, state, &text);
        self.workers.forget(&task.id);
        outcome
    }

    /// How a worker's turn ended, what it said, and the outcome to report. A stop wins however late it
    /// came; otherwise the last attempt decides, and an earlier reply never stands in for a later
    /// failure. A reply cut off at the output limit is kept but is not an answer.
    pub fn worker_result(&self, session_id: &str) -> (TaskState, String, &'static str) {
        let attempt = match (self.turns.take_end(session_id), last_attempt(&self.store, session_id)) {
            (Some(TurnEnd::Stopped), _) => Attempt::Stopped,
            (Some(TurnEnd::Failed), Attempt::Replied(_)) => Attempt::Failed("its turn ended without finishing".into()),
            (_, attempt) => attempt,
        };
        match attempt {
            Attempt::Replied(reply) => (TaskState::Replied, clip(&reply, RESULT_CHARS), "replied"),
            Attempt::Incomplete(partial) => {
                let text = format!(
                    "The subagent stopped at its output limit before finishing; this is not a complete answer. What it had written:\n\n{}",
                    clip(&partial, RESULT_CHARS)
                );
                (TaskState::Failed, text, "incomplete")
            }
            Attempt::Limited(write_up) => {
                let text = format!(
                    "The subagent reached its step or repeat limit before finishing; this is its account of where it got to, not a complete answer:\n\n{}",
                    clip(&write_up, RESULT_CHARS)
                );
                (TaskState::Failed, text, "incomplete")
            }
            Attempt::Refused(partial) => {
                let before = if partial.trim().is_empty() {
                    String::new()
                } else {
                    format!(" What it had written:\n\n{}", clip(&partial, RESULT_CHARS))
                };
                (
                    TaskState::Failed,
                    format!("The provider's safety filter ended the subagent's reply; this is not an answer.{before}"),
                    "refused",
                )
            }
            Attempt::Failed(error) => (TaskState::Failed, format!("The subagent failed: {error}"), "failed"),
            Attempt::Stopped => (TaskState::Stopped, STOPPED.into(), "stopped"),
            Attempt::None => (
                TaskState::Failed,
                "The subagent finished without a reply.".into(),
                "failed",
            ),
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

    /// Hands a finished background result to its parent as a prompt, once, unless a call of the parent's is taking it.
    pub async fn deliver(self: &Arc<Self>, task_id: &str) {
        while self.workers.claim(task_id, Claimant::Automatic) {
            // Read after claiming: a call may have taken it just before.
            if let Ok(Some(task)) = self.store.task(task_id)
                && !task.delivered
                && !task.held
                && task.state.is_terminal()
                && task.mode == Mode::Background
            {
                self.deliver_claimed(&task).await;
            }
            // A trigger that found it claimed while this attempt failed is not lost: it is tried once more here.
            if !self.workers.release_where_task(task_id, &Claimant::Automatic) {
                return;
            }
        }
    }

    /// Tries owed background results of `parent`, or of every session, again; each is one attempt, never a loop.
    pub fn retry_deliveries(self: &Arc<Self>, parent: Option<&str>) {
        let Some(runtime) = tokio::runtime::Handle::try_current()
            .ok()
            .or_else(|| self.runtime.get().cloned())
        else {
            return;
        };
        for task in self.store.owed_background(parent).unwrap_or_default() {
            let engine = self.clone();
            runtime.spawn(async move { engine.deliver(&task.id).await });
        }
    }

    async fn deliver_claimed(self: &Arc<Self>, task: &TaskRecord) {
        let owner = &task.parent_session_id;
        // Every wait before admission, and admission itself, ends when a Stop cancels this.
        let Some(scope) = self.scope_at(owner, task.generation) else {
            return self.hold(&task.id);
        };
        let prompt = Prompt {
            parts: vec![result_part(task)],
            model: None,
            variant: None,
            agent: None,
            submission_id: Some(format!("task:{}", task.id)),
        };
        let wakes = matches!(task.state, TaskState::Replied | TaskState::Failed);
        let how = super::turn::Admission {
            parent: Some(&scope),
            delivery: Some(&task.id),
            steer_only: !wakes,
            ..super::turn::Admission::default()
        };
        match self.admit(owner, prompt, how).await {
            Ok(_) | Err(TurnError::SubmissionReused) => self.publish_task(&task.id),
            Err(TurnError::Stopped) => self.hold(&task.id),
            // Left owed with its reason; the parent's job ending or a repair tries it again.
            Err(error) => {
                let reason = if error == TurnError::Busy {
                    "the conversation is busy with another job; it goes in when that ends".to_string()
                } else {
                    error.to_string()
                };
                if self.store.set_delivery_error(&task.id, &reason).is_ok() {
                    self.publish_task(&task.id);
                }
            }
        }
    }

    /// Keeps a result from waking its parent; it rides along with the parent's next prompt instead.
    fn hold(&self, task_id: &str) {
        if self.store.hold_task(task_id).unwrap_or(false) {
            self.publish_task(task_id);
        }
    }

    /// Gives up the claims `claimant` holds. A finished background result left owed goes out by itself.
    pub(super) fn release_claims(self: &Arc<Self>, claimant: &Claimant) {
        for task in self.workers.release_where(|holder| holder == claimant) {
            self.redeliver(&task);
        }
    }

    /// Gives up every claim held by a call of `session_id`.
    pub(super) fn release_claims_of(self: &Arc<Self>, session_id: &str) {
        for task in self
            .workers
            .release_where(|holder| matches!(holder, Claimant::Call { session_id: s, .. } if s == session_id))
        {
            self.redeliver(&task);
        }
    }

    fn redeliver(self: &Arc<Self>, task_id: &str) {
        let owed = self
            .store
            .task(task_id)
            .ok()
            .flatten()
            .is_some_and(|t| t.mode == Mode::Background && t.state.is_terminal() && !t.delivered && !t.held);
        if owed {
            let (engine, task_id) = (self.clone(), task_id.to_string());
            tokio::spawn(async move { engine.deliver(&task_id).await });
        }
    }

    /// Stops one worker and nothing else, wherever it is: queued, starting, planning or running.
    pub fn stop_task(&self, task_id: &str) -> Result<TaskRecord, TurnError> {
        let task = self.store.task(task_id)?.ok_or(TurnError::NoSession)?;
        if !task.state.is_terminal() {
            self.workers.cancel(task_id);
            if task.state == TaskState::Queued {
                self.end_task(task_id, TaskState::Stopped, STOPPED);
            }
        }
        Ok(self.store.task(task_id)?.unwrap_or(task))
    }

    /// Stops the owner's background workers and counts the Stop durably, so nothing launched before it wakes the owner.
    pub(super) fn stop_workers(&self, owners: &mut Owners, owner: &str) -> bool {
        let running = self
            .store
            .tasks_of(owner)
            .unwrap_or_default()
            .iter()
            .any(|t| t.mode == Mode::Background && !t.state.is_terminal());
        let entry = owner_entry(owners, &self.store, owner);
        entry.generation = self.store.bump_stop_generation(owner).unwrap_or(entry.generation + 1);
        entry.token.cancel();
        entry.token = CancellationToken::new();
        running
    }

    /// After a restart, owed results go where they belong: background ones as prompts, foreground ones into their own call.
    pub async fn recover_tasks(self: &Arc<Self>) {
        for task in self.store.undelivered_tasks().unwrap_or_default() {
            match task.mode {
                Mode::Background => self.deliver(&task.id).await,
                Mode::Foreground => self.recover_foreground(&task),
            }
        }
    }

    /// Writes a foreground result into its launching call if that call's own result never landed.
    fn recover_foreground(&self, task: &TaskRecord) {
        let transcript = self.store.transcript(&task.parent_session_id).unwrap_or_default();
        let call = transcript
            .into_iter()
            .flat_map(|m| m.parts)
            .find(|row| matches!(&row.part, Part::ToolCall { call_id, .. } if *call_id == task.call_id));
        let unsettled = |row: &PartRow| {
            matches!(
                row.part,
                Part::ToolCall {
                    status: ToolStatus::Pending | ToolStatus::Running | ToolStatus::Error,
                    ..
                }
            )
        };
        let Some(mut row) = call.filter(unsettled) else {
            // The call already shows its result (saved before that write also acknowledged it).
            let _ = self.store.mark_task_delivered(&task.id);
            return self.publish_task(&task.id);
        };
        let status = if task.state == TaskState::Replied {
            ToolStatus::Done
        } else {
            ToolStatus::Error
        };
        let metadata = super::types::ToolMetadata {
            session_id: Some(task.session_id.clone()),
            task_id: Some(task.id.clone()),
            agent: Some(task.agent.clone()),
            outcome: Some(task.state.as_str().into()),
            mode: Some("foreground".into()),
            ..Default::default()
        };
        self.settle_delivering(
            &mut row,
            super::turn::Settlement::new(
                status,
                Some(task.description.clone()),
                task.result.clone().unwrap_or_default(),
                Some(metadata),
            ),
            Some(&task.id),
        );
    }
}

const STOPPED: &str = "The subagent was stopped before it finished.";

/// A finished worker's result as the part that carries it into its parent.
pub(super) fn result_part(task: &TaskRecord) -> Part {
    Part::TaskResult {
        task_id: task.id.clone(),
        worker_session_id: task.session_id.clone(),
        description: task.description.clone(),
        outcome: task.state.as_str().into(),
        text: task.result.clone().unwrap_or_default(),
    }
}

/// How a session's last model attempt ended, as its transcript shows it.
pub(crate) enum Attempt {
    Replied(String),
    /// It finished writing only because it hit the output limit; the text is partial.
    Incomplete(String),
    /// The turn's step or repeat limit stopped it; the text is its write-up of unfinished work.
    Limited(String),
    /// The provider's safety filter ended it; the text is whatever came before.
    Refused(String),
    Failed(String),
    Stopped,
    None,
}

/// Judged by the last attempt alone. A finished summary is bookkeeping and skipped; a stopped or failed
/// one is how the session last ended.
pub(crate) fn last_attempt(store: &crate::store::Store, session_id: &str) -> Attempt {
    let transcript = store.transcript(session_id).unwrap_or_default();
    let Some(last) = transcript
        .iter()
        .rev()
        .find(|m| m.info.role == Role::Assistant && !(m.info.summary && m.info.status == MessageStatus::Done))
    else {
        return Attempt::None;
    };
    let text = || {
        last.parts
            .iter()
            .filter_map(|row| match &row.part {
                Part::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    match last.info.status {
        MessageStatus::Done if last.info.ending == Some(Ending::Refused) => Attempt::Refused(text()),
        MessageStatus::Done if last.info.ending == Some(Ending::Limit) => Attempt::Limited(text()),
        // Typed for new replies; one from before the field is known by its error alone.
        MessageStatus::Done if last.info.ending == Some(Ending::Length) || last.info.error.is_some() => {
            Attempt::Incomplete(text())
        }
        MessageStatus::Done => Attempt::Replied(text()),
        MessageStatus::Error | MessageStatus::Paused => {
            Attempt::Failed(last.info.error.clone().unwrap_or_else(|| "unknown error".into()))
        }
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
