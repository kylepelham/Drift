//! One prompt, one turn: stream the model, run what it calls, repeat until it stops.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;

use super::assemble::Assembler;
use super::attach::Attach;
use super::compaction::{self, Trigger};
use super::oneshot::Resolved;
use super::prompt;
use crate::config::Config;
use crate::event::{Event, SessionStatus};
use crate::id;
use crate::llm::catalog::Model;
use crate::llm::{self, Credential, Provider, Request, StopReason};
use crate::permission::{self, Outcome};
use crate::session::types::{Message, MessageStatus, MessageWithParts, ModelRef, Part, PartRow, Role, Session, ToolStatus, Usage, Visibility};
use super::tasks::Claimant;
use crate::store::{Admitted, Handover};
use crate::tool::{Context, SessionFiles};
use crate::Engine;

/// Retries after a provider fault before the turn gives up and shows the error.
const MAX_RETRIES: u32 = 8;
/// First backoff when the provider names no wait; it doubles each retry.
#[cfg(not(test))]
const RETRY_BASE: Duration = Duration::from_secs(1);
#[cfg(test)]
const RETRY_BASE: Duration = Duration::from_millis(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// A provider asking for longer than this (a spent quota, say) is not waited on; the error stands.
const MAX_REQUESTED_WAIT: Duration = Duration::from_secs(10 * 60);
/// Output cap when the model allows more; keeps a runaway response from burning the budget.
const MAX_OUTPUT_TOKENS: u32 = 32_000;
/// How long a prompt waits for a job that is not a turn (a compaction, an undo) before it is refused.
#[cfg(not(test))]
const QUEUE_WAIT: Duration = Duration::from_secs(30);
#[cfg(test)]
const QUEUE_WAIT: Duration = Duration::from_secs(2);
/// A finished reply's `error` when it stopped at the output limit rather than ending on its own.
pub const OUTPUT_LIMIT_ENDING: &str = "The reply stopped at the output limit";
/// What a thinking budget always leaves for the answer itself.
const MIN_ANSWER_TOKENS: u32 = 1024;
/// The smallest thinking budget providers accept.
const MIN_THINKING_TOKENS: u32 = 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Prompt {
    pub parts: Vec<Part>,
    #[serde(default)]
    pub model: Option<ModelRef>,
    #[serde(default)]
    pub thinking_budget: Option<u32>,
    /// Client-chosen id; resubmitting with the same id returns the original receipt instead of a second turn.
    #[serde(default)]
    pub submission_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub session: Session,
    pub message: Message,
}

#[derive(Debug, PartialEq)]
pub enum TurnError {
    NoSession,
    NoWorkspace,
    Busy,
    SubmissionReused,
    NoModel,
    UnknownModel,
    NoCredentials,
    /// The session is not waiting to retry a failed request, so there is nothing to switch.
    NotRetrying,
    /// The session is undone back to a prompt; send a prompt or redo first.
    Reverted,
    /// A file in the prompt cannot go to this model; says which and why.
    Attachment(String),
    Store(String),
    /// Stopped before the prompt was admitted; nothing was written.
    Stopped,
    /// The session moved to another workspace after its turn was planned.
    Moved,
}

impl std::fmt::Display for TurnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSession => write!(f, "session not found"),
            Self::NoWorkspace => write!(f, "workspace not found"),
            Self::Busy => write!(f, "session is already running a turn"),
            Self::SubmissionReused => write!(f, "submission id was already used with a different prompt or session"),
            Self::NoModel => write!(f, "no model selected"),
            Self::UnknownModel => write!(f, "model is not in the catalog"),
            Self::NoCredentials => write!(f, "provider has no credentials"),
            Self::NotRetrying => write!(f, "the session is not waiting to retry"),
            Self::Reverted => write!(f, "the session is undone; send a prompt or redo first"),
            Self::Attachment(message) => write!(f, "{message}"),
            Self::Store(message) => write!(f, "store: {message}"),
            Self::Stopped => write!(f, "stopped before it started"),
            Self::Moved => write!(f, "the session moved to another workspace while it waited"),
        }
    }
}

impl From<rusqlite::Error> for TurnError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.to_string())
    }
}

#[derive(Default)]
pub struct Turns {
    active: Mutex<HashMap<String, CancellationToken>>,
    /// Fired when a session's turn finishes; parents await their children through it.
    finished: tokio::sync::Notify,
    files: Mutex<HashMap<String, Arc<SessionFiles>>>,
    refreshing: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Automatic compactions that failed in a row, per session; enough of them turn it off for that session.
    pub(super) compaction_failures: Mutex<HashMap<String, u32>>,
    /// How each subagent's last turn ended, until the task waiting on it takes the answer.
    ended: Mutex<HashMap<String, TurnEnd>>,
    /// Turns waiting out a retry backoff, each ready to take a model the user switches to.
    retry_waits: Mutex<HashMap<String, tokio::sync::oneshot::Sender<Resolved>>>,
    /// Sessions whose running job is a turn still taking prompts sent while it runs. Admitting one and
    /// a turn deciding it is done both hold this lock, so no prompt lands after the turn stops looking.
    /// Each with the model its turn is running on, which is what a steered prompt is judged against.
    steering: Mutex<HashMap<String, ModelRef>>,
    /// Tests swap the wire adapter for a scripted one.
    pub provider_override: Mutex<Option<Provider>>,
}

/// How a turn ended, from the loop's own view rather than whatever message happens to be last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnEnd {
    Replied,
    Failed,
    Stopped,
}

impl Turns {
    /// The recorded end of a subagent's last turn; each end is handed out once.
    pub fn take_end(&self, session_id: &str) -> Option<TurnEnd> {
        self.ended.lock().unwrap().remove(session_id)
    }

    /// Marks the session busy under `abort`; `false` if a turn or job already holds it.
    pub(super) fn claim(&self, session_id: &str, abort: &CancellationToken) -> bool {
        let mut active = self.active.lock().unwrap();
        if active.contains_key(session_id) {
            return false;
        }
        active.insert(session_id.into(), abort.clone());
        true
    }

    /// Frees a claim whose turn never started; anyone waiting for the session to go idle wakes.
    pub(super) fn release(&self, session_id: &str) {
        self.active.lock().unwrap().remove(session_id);
        self.finished.notify_waiters();
    }

    /// Runs `change` only if none of `ids` is claimed, holding claims off until it returns.
    pub(super) fn while_idle<T>(&self, ids: &[String], change: impl FnOnce() -> T) -> Option<T> {
        let active = self.active.lock().unwrap();
        if ids.iter().any(|id| active.contains_key(id)) {
            return None;
        }
        let result = change();
        drop(active);
        Some(result)
    }

    fn files_for(&self, session_id: &str) -> Arc<SessionFiles> {
        self.files.lock().unwrap().entry(session_id.into()).or_default().clone()
    }

    pub(super) fn refresh_lock(&self, provider: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.refreshing.lock().unwrap().entry(provider.into()).or_default().clone()
    }

    pub fn is_running(&self, session_id: &str) -> bool {
        self.active.lock().unwrap().contains_key(session_id)
    }

    /// Cancels whatever holds the session, if anything does.
    pub(super) fn cancel(&self, session_id: &str) -> bool {
        self.active.lock().unwrap().get(session_id).inspect(|token| token.cancel()).is_some()
    }

    /// A turn is running in the session and still takes prompts sent to it.
    pub fn is_steerable(&self, session_id: &str) -> bool {
        self.steering.lock().unwrap().contains_key(session_id)
    }

    /// Resolves once the session has no turn in flight, or the caller is aborted.
    pub async fn wait_idle(&self, session_id: &str, abort: &CancellationToken) {
        loop {
            let notified = self.finished.notified();
            if !self.is_running(session_id) {
                return;
            }
            tokio::select! {
                () = notified => {}
                () = abort.cancelled() => return,
            }
        }
    }
}

/// Everything a turn runs on, fixed when it is admitted: a queued worker keeps what it was given.
pub(crate) struct Plan {
    pub(super) session: Session,
    workspace: PathBuf,
    pub(super) config: Arc<Config>,
    pub(super) model_ref: ModelRef,
    pub(super) model: Model,
    pub(super) provider: Provider,
    pub(super) credential: Credential,
    thinking_budget: Option<u32>,
    offer: Offer,
}

/// How a prompt is admitted beyond the session itself.
#[derive(Clone, Copy, Default)]
pub(super) struct Admission<'a> {
    /// Its turn's abort token descends from this, and every wait before admission ends with it.
    pub(super) parent: Option<&'a CancellationToken>,
    /// It carries this worker's result, marked handed over in the same write.
    pub(super) delivery: Option<&'a str>,
    /// Only into a turn already running; it never starts one.
    pub(super) steer_only: bool,
}

impl Engine {
    /// Records the prompt and starts the turn in the background; the receipt is what was recorded.
    pub async fn submit(self: &Arc<Self>, session_id: &str, prompt: Prompt) -> Result<Receipt, TurnError> {
        self.submit_under(session_id, prompt, None).await
    }

    /// A turn whose abort token descends from parent, so aborting the parent aborts it however the wait ends.
    pub async fn submit_under(self: &Arc<Self>, session_id: &str, prompt: Prompt, parent: Option<&CancellationToken>) -> Result<Receipt, TurnError> {
        self.admit(session_id, prompt, Admission { parent, ..Admission::default() }).await
    }

    pub(super) async fn admit(self: &Arc<Self>, session_id: &str, prompt: Prompt, how: Admission<'_>) -> Result<Receipt, TurnError> {
        let payload_hash = payload_hash(&prompt);
        if let Some(id) = prompt.submission_id.as_deref() {
            if let Some(receipt) = self.replayed_receipt(id, session_id, &payload_hash)? {
                return Ok(receipt);
            }
        }
        // Claimed before planning: the plan captures the workspace, so a move must not slip in while it resolves.
        let abort = how.parent.map_or_else(CancellationToken::new, CancellationToken::child_token);
        if abort.is_cancelled() {
            return Err(TurnError::Stopped);
        }
        if !self.turns.claim(session_id, &abort) {
            return self.steer_or_queue(session_id, prompt, how, &payload_hash).await;
        }
        if how.steer_only {
            self.turns.release(session_id);
            return Err(TurnError::Stopped);
        }
        let planned = tokio::select! {
            planned = self.plan(session_id, &prompt) => planned,
            () = abort.cancelled() => Err(TurnError::Stopped),
        };
        match planned {
            Ok(plan) => self.start(session_id, prompt, plan, abort, &payload_hash, how.delivery),
            Err(error) => {
                self.turns.release(session_id);
                Err(error)
            }
        }
    }

    /// Starts a turn on a queued worker's plan as admitted; only the credential is looked up afresh.
    pub(super) async fn submit_planned(self: &Arc<Self>, session_id: &str, prompt: Prompt, mut plan: Plan, parent: &CancellationToken) -> Result<Receipt, TurnError> {
        let abort = parent.child_token();
        loop {
            if abort.is_cancelled() {
                return Err(TurnError::Stopped);
            }
            if self.turns.claim(session_id, &abort) {
                break;
            }
            self.turns.wait_idle(session_id, &abort).await;
        }
        let ready = tokio::select! {
            ready = self.refresh_plan(&mut plan) => ready,
            () = abort.cancelled() => Err(TurnError::Stopped),
        };
        if let Err(error) = ready {
            self.turns.release(session_id);
            return Err(error);
        }
        let hash = payload_hash(&prompt);
        self.start(session_id, prompt, plan, abort, &hash, None)
    }

    async fn refresh_plan(&self, plan: &mut Plan) -> Result<(), TurnError> {
        let session = self.store.session(&plan.session.id)?.ok_or(TurnError::NoSession)?;
        if session.workspace_id != plan.session.workspace_id {
            return Err(TurnError::Moved);
        }
        let provider = plan.model_ref.provider.clone();
        let env = self.catalog.read().unwrap().providers.get(&provider).map(|p| p.env.clone()).unwrap_or_default();
        let stored = self.credentials.resolve(&provider, &env).ok_or(TurnError::NoCredentials)?;
        plan.credential = self.fresh_credential(&provider, stored).await?;
        Ok(())
    }

    /// Admits the prompt into a session this call has claimed and starts its turn; releases the claim if it cannot.
    fn start(self: &Arc<Self>, session_id: &str, prompt: Prompt, plan: Plan, abort: CancellationToken, payload_hash: &str, delivery: Option<&str>) -> Result<Receipt, TurnError> {
        let files = self.turns.files_for(session_id);
        let attach = Attach { engine: self, session_id, workspace: &plan.workspace, policy: &plan.config.policy(), model: &plan.model, files: &files };
        let submission = prompt.submission_id.as_deref().map(|id| (id, payload_hash));
        let admitted = attach.prepare(prompt.parts).and_then(|parts| self.admit_fenced(session_id, &plan.model_ref, parts, submission, Some(&abort), delivery));
        let admitted = match admitted {
            Ok(admitted) => admitted,
            Err(error) => {
                self.turns.release(session_id);
                return Err(error);
            }
        };
        let receipt = self.announce(session_id, admitted);
        self.turns.steering.lock().unwrap().insert(session_id.into(), plan.model_ref.clone());
        let engine = self.clone();
        self.spawn_job(session_id, async move { engine.run(plan, abort).await });
        Ok(receipt)
    }

    /// The last check before a prompt is written, under the lock every Stop holds, so it lands wholly before a Stop or not at all.
    pub(super) fn admit_fenced(&self, session_id: &str, model: &ModelRef, parts: Vec<Part>, submission: Option<(&str, &str)>, abort: Option<&CancellationToken>, delivery: Option<&str>) -> Result<Admitted, TurnError> {
        let _fence = self.workers.fence();
        if abort.is_some_and(CancellationToken::is_cancelled) {
            return Err(TurnError::Stopped);
        }
        // Results a Stop held back ride along with any admitted prompt, each claimed so no other path takes it meanwhile.
        let held: Vec<_> = self.store.held_tasks(session_id)?.into_iter().filter(|task| self.workers.claim(&task.id, Claimant::Automatic)).collect();
        let carried = held.iter().map(|task| (task.id.clone(), super::tasks::result_part(task))).collect();
        let admitted = self.store.admit_delivering(session_id, model, parts, submission, Handover { delivery, held: carried });
        for task in &held {
            self.workers.release_where_task(&task.id, &Claimant::Automatic);
            self.publish_task(&task.id);
        }
        // `None` only for a delivery another path already landed: this copy, and its riders, are not written.
        admitted?.ok_or(TurnError::SubmissionReused)
    }

    pub(super) fn announce(&self, session_id: &str, admitted: Admitted) -> Receipt {
        let Admitted { message, parts, session, discarded } = admitted;
        for message_id in discarded {
            self.hub.publish(Event::MessageRemoved { session_id: session_id.into(), message_id });
        }
        self.hub.publish(Event::MessageCreated { message: message.clone() });
        for row in parts {
            self.hub.publish(Event::PartCreated { part: row });
        }
        self.hub.publish(Event::SessionUpdated { session: session.clone() });
        Receipt { session, message }
    }

    /// A prompt for a busy session. A running turn takes it at its next model request (after the
    /// calls in flight finish, so their results come first). Any other job, such as a compaction or an
    /// undo, is waited out for up to [`QUEUE_WAIT`], then the prompt starts a turn of its own.
    async fn steer_or_queue(self: &Arc<Self>, session_id: &str, prompt: Prompt, how: Admission<'_>, payload_hash: &str) -> Result<Receipt, TurnError> {
        if let Some(receipt) = self.steer(session_id, &prompt, payload_hash, how)? {
            return Ok(receipt);
        }
        if how.steer_only {
            return Err(TurnError::Stopped);
        }
        let never = CancellationToken::new();
        let stop = how.parent.unwrap_or(&never);
        if tokio::time::timeout(QUEUE_WAIT, self.turns.wait_idle(session_id, stop)).await.is_err() {
            return Err(TurnError::Busy);
        }
        if stop.is_cancelled() {
            return Err(TurnError::Stopped);
        }
        Box::pin(self.admit(session_id, prompt, how)).await
    }

    /// Admits `prompt` into the turn running in `session_id`, if one is running and still taking
    /// prompts. Its files are judged against the model that turn is running on, not one the prompt
    /// names: the running turn does not switch models for a steered prompt.
    fn steer(&self, session_id: &str, prompt: &Prompt, payload_hash: &str, how: Admission<'_>) -> Result<Option<Receipt>, TurnError> {
        let Some(running) = self.turns.steering.lock().unwrap().get(session_id).cloned() else { return Ok(None) };
        let session = self.store.session(session_id)?.ok_or(TurnError::NoSession)?;
        let model = self.catalog.read().unwrap().providers.get(&running.provider).and_then(|p| p.models.get(&running.model)).cloned().ok_or(TurnError::UnknownModel)?;
        let workspace = self.store.workspace(&session.workspace_id)?.ok_or(TurnError::NoWorkspace)?;
        let workspace = crate::tool::canonical(Path::new(&workspace.path));
        let policy = self.workspace_config(&workspace).policy();
        let files = self.turns.files_for(session_id);
        let parts = Attach { engine: self, session_id, workspace: &workspace, policy: &policy, model: &model, files: &files }.prepare(prompt.parts.clone())?;
        let steering = self.turns.steering.lock().unwrap();
        match steering.get(session_id) {
            None => return Ok(None),
            // The turn moved to another model meanwhile; judge the files again against that one.
            Some(now) if *now != running => {
                drop(steering);
                return self.steer(session_id, prompt, payload_hash, how);
            }
            Some(_) => {}
        }
        let submission = prompt.submission_id.as_deref().map(|id| (id, payload_hash));
        let admitted = self.admit_fenced(session_id, &running, parts, submission, how.parent, how.delivery)?;
        drop(steering);
        Ok(Some(self.announce(session_id, admitted)))
    }

    /// Runs `job` in a session already claimed: reports it running, then idle and releases it when done.
    /// The job runs as its own task, so even one that panics releases the session.
    pub(super) fn spawn_job(self: &Arc<Self>, session_id: &str, job: impl std::future::Future<Output = ()> + Send + 'static) {
        self.hub.publish(Event::SessionStatusChanged { session_id: session_id.into(), status: SessionStatus::Running });
        let engine = self.clone();
        let id = session_id.to_string();
        tokio::spawn(async move {
            if let Err(failure) = tokio::spawn(job).await {
                eprintln!("drift: a job for session {id} failed: {failure}");
            }
            engine.turns.retry_waits.lock().unwrap().remove(&id);
            engine.turns.steering.lock().unwrap().remove(&id);
            engine.turns.active.lock().unwrap().remove(&id);
            // A call that panicked never released what it was handing over.
            engine.release_claims_of(&id);
            engine.hub.publish(Event::SessionStatusChanged { session_id: id.clone(), status: SessionStatus::Idle });
            engine.turns.finished.notify_waiters();
            // Results that found the session busy go in now; one racing its own failed attempt is retried by that attempt.
            engine.retry_deliveries(Some(&id));
        });
    }

    /// A known submission id replays its receipt from storage, so a retry after a restart is still one prompt.
    pub(super) fn replayed_receipt(&self, id: &str, session_id: &str, payload_hash: &str) -> Result<Option<Receipt>, TurnError> {
        let Some(found) = self.store.submission(id)? else { return Ok(None) };
        if found.session_id != session_id || found.payload_hash != payload_hash {
            return Err(TurnError::SubmissionReused);
        }
        let session = self.store.session(session_id)?.ok_or(TurnError::NoSession)?;
        let message = self.store.message(&found.message_id)?.ok_or(TurnError::NoSession)?;
        Ok(Some(Receipt { session, message }))
    }

    /// Stops the session's turn, its background workers and, for a worker's transcript, that worker, all under the admission fence.
    pub fn abort(&self, session_id: &str) -> bool {
        let mut owners = self.workers.fence();
        let workers = self.stop_workers(&mut owners, session_id);
        let turn = self.turns.cancel(session_id);
        let worker = self.store.task_for_session(session_id).ok().flatten().is_some_and(|task| !task.state.is_terminal() && self.workers.cancel(&task.id));
        turn || workers || worker
    }

    pub(super) async fn plan(&self, session_id: &str, prompt: &Prompt) -> Result<Plan, TurnError> {
        let session = self.store.session(session_id)?.ok_or(TurnError::NoSession)?;
        let workspace = self.store.workspace(&session.workspace_id)?.ok_or(TurnError::NoWorkspace)?;
        let workspace_path = crate::tool::canonical(Path::new(&workspace.path));
        let config = self.workspace_config(&workspace_path);
        let agent_model = config.agent(&session.agent).and_then(|a| a.model.clone());
        let model_ref = prompt.model.clone().or_else(|| session.model.clone()).or(agent_model).or_else(|| config.model.clone()).ok_or(TurnError::NoModel)?;
        let resolved = self.resolve(&model_ref).await?;
        let provider = resolved.provider.with_timeouts(config.route_timeouts(&resolved.model_ref.provider));
        let mut plan = Plan {
            session,
            workspace: workspace_path,
            config: Arc::new(config),
            model_ref: resolved.model_ref,
            model: resolved.model,
            provider,
            credential: resolved.credential,
            thinking_budget: prompt.thinking_budget,
            offer: Offer::default(),
        };
        // A server connecting right now would otherwise be missing from this turn's tools.
        self.mcp.wait_ready(crate::mcp::READY_WAIT).await;
        plan.offer = self.offer(&plan);
        Ok(plan)
    }

    /// Expired subscription tokens are refreshed once, however many turns notice at the same time.
    pub(super) async fn fresh_credential(&self, provider: &str, credential: Credential) -> Result<Credential, TurnError> {
        if !credential.is_expired() {
            return Ok(credential);
        }
        let lock = self.turns.refresh_lock(provider);
        let _held = lock.lock().await;
        // Another turn may have refreshed while we waited; its token is the one to use.
        if let Some(stored) = self.credentials.get(provider).filter(|stored| !stored.is_expired()) {
            return Ok(stored);
        }
        let Credential::OAuth { refresh, .. } = &credential else { return Ok(credential) };
        let refreshed = match provider {
            "anthropic" => llm::anthropic::oauth::refresh(&self.http, refresh).await,
            "openai" => llm::openai::oauth::refresh(&self.http, refresh).await,
            _ => return Ok(credential),
        };
        let fresh = refreshed.map_err(|_| TurnError::NoCredentials)?;
        // If the user signed in or out while we were refreshing, their change stands and this turn uses it.
        if !self.credentials.replace_if(provider, &credential, &fresh).map_err(TurnError::Store)? {
            return self.credentials.get(provider).ok_or(TurnError::NoCredentials);
        }
        Ok(fresh)
    }

    pub(super) fn provider_for(&self, id: &str, catalog_api: Option<&str>) -> Option<Provider> {
        if let Some(provider) = self.turns.provider_override.lock().unwrap().clone() {
            return Some(provider);
        }
        llm::provider_for(id, catalog_api)
    }

    /// Steps until the model is done, then again for any prompt steered in meanwhile.
    async fn run(self: &Arc<Self>, mut plan: Plan, abort: CancellationToken) {
        self.title_untitled(&plan.session);
        loop {
            let answered = self.run_steps(&mut plan, &abort).await;
            if abort.is_cancelled() || !self.steered_after(&plan.session.id, answered.as_deref()) {
                break;
            }
        }
        self.turns.steering.lock().unwrap().remove(&plan.session.id);
        self.record_end(&plan.session, &abort);
    }

    /// Under the steering lock: whether a prompt arrived after the last one this turn answered. If
    /// none did, the turn stops taking prompts in the same breath, so none can land unanswered.
    fn steered_after(&self, session_id: &str, answered: Option<&str>) -> bool {
        let mut steering = self.turns.steering.lock().unwrap();
        let transcript = self.store.transcript(session_id).unwrap_or_default();
        let newest = transcript.iter().rev().find(|m| m.info.role == Role::User).map(|m| m.info.id.as_str());
        let steered = matches!((newest, answered), (Some(newest), Some(answered)) if newest > answered);
        if !steered {
            steering.remove(session_id);
        }
        steered
    }

    /// One run of model steps; returns the newest prompt the last request included.
    async fn run_steps(self: &Arc<Self>, plan: &mut Plan, abort: &CancellationToken) -> Option<String> {
        let mut attempts = 0;
        let mut recovered = false;
        let limits = plan.config.limits_for(&plan.session.agent);
        let mut steps = 0;
        let mut repeats = Repeats::default();
        let mut answered = None;
        loop {
            if steps >= limits.steps {
                self.pause(plan, format!("Paused after {steps} steps, this turn's limit. Send a message to carry on."));
                break;
            }
            let Some(transcript) = self.transcript_for_step(plan, abort).await else { break };
            answered = transcript.iter().rev().find(|m| m.info.role == Role::User).map(|m| m.info.id.clone());
            let (max_tokens, thinking_budget) = budgets(&plan.model, plan.thinking_budget);
            let request = Request {
                model: plan.model_ref.model.clone(),
                system: plan.offer.system.clone(),
                messages: compaction::request_messages(&transcript, &plan.model_ref),
                tools: plan.offer.specs(),
                max_tokens,
                thinking_budget,
                temperature: None,
                cache_key: Some(plan.session.id.clone()),
            };
            let Ok(message) = self.store.create_message(&plan.session.id, Role::Assistant, Some(&plan.model_ref)) else { break };
            self.hub.publish(Event::MessageCreated { message: message.clone() });
            match self.step(plan, message, &request, abort).await {
                Step::Done => break,
                Step::Continue => {
                    attempts = 0;
                    steps += 1;
                    if let Some(times) = repeats.record(self.last_calls(&plan.session.id), &limits) {
                        let reason = format!("Paused: the last {times} steps made the same calls and got the same results. Send a message to carry on or change course.");
                        self.pause(plan, reason);
                        break;
                    }
                }
                Step::Retry(retry) if retry.allowed(attempts) => {
                    attempts += 1;
                    match self.wait_to_retry(&plan.session.id, attempts, &retry, abort).await {
                        Wait::Elapsed => {}
                        Wait::Switched(resolved) => {
                            self.adopt(plan, *resolved);
                            attempts = 0;
                        }
                        Wait::Stopped => break,
                    }
                }
                Step::Retry(_) => break,
                // A request too long for the model is compacted once and retried.
                Step::Overflow if !recovered => {
                    recovered = true;
                    if self.compact(&plan.session.id, Trigger::Overflow, abort).await.is_err() {
                        break;
                    }
                }
                Step::Overflow => break,
            }
        }
        answered
    }

    /// The tools and system prompt for the plan's model and agent. What was offered is what may run:
    /// a call to any other tool is refused before permission or snapshot.
    fn offer(&self, plan: &Plan) -> Offer {
        let agent = plan.config.agent(&plan.session.agent).cloned();
        let allowed = agent.as_ref().map(|a| a.tools.clone()).unwrap_or_default();
        let subagent = plan.session.visibility == Visibility::Hidden;
        let tools: Vec<_> = self
            .offered_tools(plan.model.profile)
            .into_iter()
            .map(|tool| (tool.spec(), tool))
            .filter(|(spec, _)| allowed.is_empty() || allowed.contains(&spec.name))
            .filter(|(spec, _)| !(subagent && crate::tool::task::DELEGATION.contains(&spec.name.as_str())))
            .collect();
        let system = prompt::system(&plan.workspace, &plan.config, agent.as_ref(), tools.iter().any(|(spec, _)| spec.name == "task"));
        Offer { tools, system }
    }

    /// Waits out a retry backoff, which the UI shows, unless the user switches the turn to another
    /// model first; then it retries at once on that model.
    async fn wait_to_retry(&self, session_id: &str, attempt: u32, retry: &Retry, abort: &CancellationToken) -> Wait {
        let delay = retry.delay(attempt);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.turns.retry_waits.lock().unwrap().insert(session_id.into(), sender);
        let next_at = id::now_ms() + delay.as_millis() as i64;
        self.hub.publish(Event::SessionRetry { session_id: session_id.into(), attempt, message: retry.message.clone(), next_at });
        let wait = tokio::select! {
            () = tokio::time::sleep(delay) => Wait::Elapsed,
            Ok(resolved) = receiver => Wait::Switched(Box::new(resolved)),
            () = abort.cancelled() => Wait::Stopped,
        };
        self.turns.retry_waits.lock().unwrap().remove(session_id);
        self.hub.publish(Event::SessionStatusChanged { session_id: session_id.into(), status: SessionStatus::Running });
        wait
    }

    /// Moves the turn onto a model the user switched to, and makes it the session's model from now on.
    fn adopt(&self, plan: &mut Plan, resolved: Resolved) {
        plan.model_ref = resolved.model_ref;
        if let Some(running) = self.turns.steering.lock().unwrap().get_mut(&plan.session.id) {
            *running = plan.model_ref.clone();
        }
        plan.model = resolved.model;
        plan.provider = resolved.provider.with_timeouts(plan.config.route_timeouts(&plan.model_ref.provider));
        plan.credential = resolved.credential;
        // The user chose another model, whose tool profile may differ.
        plan.offer = self.offer(plan);
        if let Ok(Some(session)) = self.store.update_session(&plan.session.id, None, Some(&plan.model_ref), None) {
            self.hub.publish(Event::SessionUpdated { session });
        }
    }

    /// Switches a turn that is waiting to retry onto `model`. The model and its credential are checked
    /// here, so a bad choice fails for the caller instead of inside the turn.
    pub async fn switch_retry_model(&self, session_id: &str, model: &ModelRef) -> Result<(), TurnError> {
        if !self.turns.retry_waits.lock().unwrap().contains_key(session_id) {
            return Err(TurnError::NotRetrying);
        }
        let resolved = self.resolve(model).await?;
        let waiting = self.turns.retry_waits.lock().unwrap().remove(session_id).ok_or(TurnError::NotRetrying)?;
        waiting.send(resolved).map_err(|_| TurnError::NotRetrying)
    }

    /// For a subagent, how its turn ended: a stop wins however late it came; otherwise the last
    /// attempt decides, and a finished summary is bookkeeping rather than an ending.
    fn record_end(&self, session: &Session, abort: &CancellationToken) {
        if session.visibility != Visibility::Hidden {
            return;
        }
        let end = if abort.is_cancelled() {
            TurnEnd::Stopped
        } else {
            let transcript = self.store.transcript(&session.id).unwrap_or_default();
            match transcript.iter().rev().find(|m| m.info.role == Role::Assistant) {
                // A finished reply that carries an error stopped at the output limit: not an answer.
                Some(last) if last.info.status == MessageStatus::Done && !last.info.summary && last.info.error.is_none() => TurnEnd::Replied,
                Some(last) if last.info.status == MessageStatus::Aborted => TurnEnd::Stopped,
                _ => TurnEnd::Failed,
            }
        };
        self.turns.ended.lock().unwrap().insert(session.id.clone(), end);
    }

    /// The transcript for the next request, compacted first when the last reply left too little room.
    /// A failed compaction still lets the request go; if it is too long, the overflow path tries once more.
    async fn transcript_for_step(self: &Arc<Self>, plan: &Plan, abort: &CancellationToken) -> Option<Vec<MessageWithParts>> {
        let transcript = self.store.transcript(&plan.session.id).ok()?;
        if !self.wants_compaction(&plan.session.id, &plan.model, &transcript) {
            return Some(transcript);
        }
        let _ = self.compact(&plan.session.id, Trigger::Auto, abort).await;
        if abort.is_cancelled() {
            return None;
        }
        self.store.transcript(&plan.session.id).ok()
    }

    /// One assistant message and the tool calls it makes.
    async fn step(self: &Arc<Self>, plan: &Plan, mut message: Message, request: &Request, abort: &CancellationToken) -> Step {
        let streamed = match self.stream(&message, plan, request, abort).await {
            Ok(streamed) => streamed,
            Err(StreamError::Aborted) => {
                message.status = MessageStatus::Aborted;
                let _ = self.finish(&mut message);
                self.settle_unrun(&message, "the reply was stopped before this call ran.");
                return Step::Done;
            }
            Err(StreamError::Provider(error)) => {
                message.status = MessageStatus::Error;
                message.error = Some(error.to_string());
                let _ = self.finish(&mut message);
                self.settle_unrun(&message, "the reply failed before it finished, so its calls were not trusted to run.");
                if error.is_context_overflow() {
                    return Step::Overflow;
                }
                return Retry::from(&error).map_or(Step::Done, Step::Retry);
            }
        };
        message.usage = streamed.usage;
        message.cost = if matches!(plan.credential, Credential::OAuth { .. }) { 0.0 } else { cost(&plan.model, streamed.usage) };
        message.status = MessageStatus::Done;
        // The reply hit its output limit: say so, and run nothing, since a call's input may be cut short.
        let cut_off = streamed.stop == StopReason::MaxTokens;
        if cut_off {
            message.error = Some(format!("{OUTPUT_LIMIT_ENDING} ({} tokens).", request.max_tokens));
        }
        if self.finish(&mut message).is_err() || streamed.calls.is_empty() {
            return Step::Done;
        }
        if cut_off {
            self.settle_unrun(&message, "the reply hit its output limit, so this call's input may be cut short.");
            return Step::Done;
        }
        match self.run_calls(plan, &message, streamed.calls, abort).await {
            Outcome::Aborted => Step::Done,
            _ => Step::Continue,
        }
    }

    /// Persists the message's terminal state. On failure the published state is an error, and the caller stops.
    fn finish(&self, message: &mut Message) -> rusqlite::Result<()> {
        message.finished_at = Some(id::now_ms());
        let saved = self.store.save_message(message).and_then(|()| self.store.touch_session(&message.session_id));
        if let Err(error) = &saved {
            message.status = MessageStatus::Error;
            message.error = Some(format!("response was not persisted ({error})"));
            let _ = self.store.save_message(message);
        }
        self.hub.publish(Event::MessageUpdated { message: message.clone() });
        saved
    }

    async fn stream(&self, message: &Message, plan: &Plan, request: &Request, abort: &CancellationToken) -> Result<Streamed, StreamError> {
        // Stop counts while the request is still being sent or the response has not begun.
        let mut chunks = tokio::select! {
            opened = plan.provider.stream(request, &plan.credential) => opened.map_err(StreamError::Provider)?,
            () = abort.cancelled() => return Err(StreamError::Aborted),
        };
        let mut assembler = Assembler::new(&self.store, &self.hub, message);
        loop {
            let next = tokio::select! {
                chunk = chunks.next() => chunk,
                () = abort.cancelled() => {
                    let _ = assembler.stop_block();
                    return Err(StreamError::Aborted);
                }
            };
            match next {
                Some(Ok(chunk)) => assembler.apply(chunk).map_err(|e| StreamError::Provider(llm::Error::Transport(e.to_string())))?,
                Some(Err(error)) => {
                    let _ = assembler.stop_block();
                    return Err(StreamError::Provider(error));
                }
                None => break,
            }
        }
        let _ = assembler.stop_block();
        // A stream that ends without saying why is a broken stream; its tool calls must not run.
        let Some(stop) = assembler.stop else {
            return Err(StreamError::Provider(llm::Error::Transport("stream ended without a stop reason".into())));
        };
        Ok(Streamed { usage: assembler.usage, stop, calls: assembler.calls })
    }

    /// Calls run in the model's order. Consecutive reads run together; a write waits for what came before it.
    async fn run_calls(self: &Arc<Self>, plan: &Plan, message: &Message, calls: Vec<PartRow>, abort: &CancellationToken) -> Outcome {
        let files = self.turns.files_for(&plan.session.id);
        let scope = CallScope { plan, message, files: &files, abort };
        let mut reads: Vec<PartRow> = Vec::new();
        for row in calls {
            if !call_mutates(plan, &row) {
                reads.push(row);
                continue;
            }
            if self.run_reads(&scope, std::mem::take(&mut reads)).await == Outcome::Aborted {
                return Outcome::Aborted;
            }
            if self.run_call(&scope, row).await == Outcome::Aborted {
                return Outcome::Aborted;
            }
        }
        self.run_reads(&scope, reads).await
    }

    async fn run_reads(self: &Arc<Self>, scope: &CallScope<'_>, reads: Vec<PartRow>) -> Outcome {
        let outcomes = futures_util::future::join_all(reads.into_iter().map(|row| self.run_call(scope, row))).await;
        if outcomes.contains(&Outcome::Aborted) { Outcome::Aborted } else { Outcome::Allowed }
    }

    async fn run_call(self: &Arc<Self>, scope: &CallScope<'_>, mut row: PartRow) -> Outcome {
        let Part::ToolCall { call_id, name, input, .. } = row.part.clone() else { return Outcome::Allowed };
        let ctx = Context {
            workspace: scope.plan.workspace.clone(),
            session_id: scope.plan.session.id.clone(),
            message_id: scope.message.id.clone(),
            call_id: call_id.clone(),
            files: scope.files.clone(),
            abort: scope.abort.clone(),
            engine: self.clone(),
            config: scope.plan.config.clone(),
        };
        // Only what this turn was offered runs, as it was when offered.
        let Some(tool) = scope.plan.offer.tool(&name) else {
            self.settle(&mut row, ToolStatus::Error, None, format!("`{name}` is not available in this session; use only the tools you were given"), None);
            return Outcome::Allowed;
        };
        if !input.is_object() {
            self.settle(&mut row, ToolStatus::Error, None, "call arguments were not valid JSON; the call did not run".into(), None);
            return Outcome::Allowed;
        }
        for ask in tool.asks(&ctx, &input) {
            if let Some(refused) = self.permit(scope, &mut row, &call_id, &name, ask).await {
                return refused;
            }
        }
        let capture = if tool.mutates() {
            self.snapshots.bind(&scope.plan.session.workspace_id, &scope.plan.workspace);
            match self.capture_before(&scope.plan.workspace, tool.touches(&ctx, &input)).await {
                Ok(capture) => Some(capture),
                Err(error) => {
                    // Nothing recorded means no way back, so the write does not happen.
                    self.settle(&mut row, ToolStatus::Error, None, format!("refused to write: could not record the files first ({error})"), None);
                    return Outcome::Allowed;
                }
            }
        } else {
            None
        };
        if let (Some(running), Part::ToolCall { metadata, .. }) = (tool.running_metadata(&ctx, &input), &mut row.part) {
            *metadata = Some(running);
        }
        if let Err(error) = self.start_call(&mut row) {
            self.settle(&mut row, ToolStatus::Error, None, format!("refused to run: could not record the call ({error})"), None);
            return Outcome::Allowed;
        }
        let result = if tool.stops_itself() {
            tool.run(&ctx, input).await
        } else {
            tokio::select! {
                result = tool.run(&ctx, input) => result,
                () = scope.abort.cancelled() => Err(crate::tool::ToolError("Aborted.".into())),
            }
        };
        let (status, title, text, mut meta) = match result {
            Ok(output) => {
                let status = if tool.failed(&output) { ToolStatus::Error } else { ToolStatus::Done };
                let formatted = if tool.mutates() { self.format_written(scope.plan, &output.metadata).await } else { Vec::new() };
                (status, Some(output.title), output.output, with_formatted(output.metadata, formatted))
            }
            Err(error) => (ToolStatus::Error, None, error.0, serde_json::Value::Null),
        };
        // Every result, MCP and tools yet to come included, reaches the model within one bound.
        let spill = self.data_dir.join("tool-output").join(&scope.plan.session.id).join(format!("{call_id}.result.log"));
        let (text, spilled) = crate::tool::spool::bound(text, spill);
        if let Some(file) = spilled {
            meta = merge(meta, Some(json!({ "resultFile": file.to_string_lossy() }))).unwrap_or_default();
        }
        // After formatting, and on failure too: a failed or stopped command may still have written.
        let changes = match capture {
            // `owner` names the workspace whose history holds these blobs, wherever it or the session moves.
            Some(capture) => self.capture_after(&scope.plan.workspace, capture).await.ok().map(|recorded| {
                let mut changes = json!({ "changes": recorded.changes, "owner": scope.plan.session.workspace_id });
                if !recorded.unrecorded.is_empty() {
                    changes["unrecorded"] = json!(recorded.unrecorded);
                }
                changes
            }),
            None => None,
        };
        // A result this call hands over is acknowledged in the write that saves it, if the call holds its claim.
        let claimant = Claimant::call(&scope.plan.session.id, &call_id);
        let delivers = meta.get("delivers").and_then(serde_json::Value::as_str).filter(|task| self.workers.holds(task, &claimant)).map(str::to_owned);
        self.settle_delivering(&mut row, status, title, text, merge(meta, changes), delivers.as_deref());
        self.release_claims(&claimant);
        if scope.abort.is_cancelled() { Outcome::Aborted } else { Outcome::Allowed }
    }

    /// Checks one of a call's asks. `None` lets the call go on; otherwise the call is settled as
    /// refused and the outcome says whether the turn goes on.
    async fn permit(&self, scope: &CallScope<'_>, row: &mut PartRow, call_id: &str, name: &str, ask: crate::tool::Ask) -> Option<Outcome> {
        let request = permission::new_request(&scope.plan.session.id, &scope.message.id, call_id, name, ask);
        match self.permissions.check(&self.hub, &scope.plan.config.policy(), request, scope.abort).await {
            Outcome::Allowed => None,
            Outcome::Refused => {
                self.settle(row, ToolStatus::Denied, None, "A permission rule forbids this call.".into(), None);
                Some(Outcome::Allowed)
            }
            Outcome::Denied { feedback, stop } => {
                self.settle(row, ToolStatus::Denied, None, denial(feedback.as_deref(), stop), None);
                if !stop {
                    return Some(Outcome::Allowed);
                }
                // The user ended the turn along with the call.
                scope.abort.cancel();
                Some(Outcome::Aborted)
            }
            Outcome::Aborted => {
                self.settle(row, ToolStatus::Error, None, "Aborted while waiting for permission.".into(), None);
                Some(Outcome::Aborted)
            }
        }
    }

    /// Runs the workspace's formatters over whatever a mutating tool reported writing.
    async fn format_written(&self, plan: &Plan, metadata: &serde_json::Value) -> Vec<String> {
        let formatters = crate::edit::format::resolve(&plan.config.formatters);
        let mut formatted = Vec::new();
        for file in metadata["files"].as_array().into_iter().flatten().filter_map(|f| f.as_str()) {
            if let Some(name) = crate::edit::format::format(Path::new(file), &plan.workspace, &formatters).await {
                formatted.push(format!("{name}: {}", crate::tool::display(Path::new(file), &plan.workspace)));
            }
        }
        formatted
    }

    /// Marks the call running in storage before it does anything; a call that cannot be recorded does not run.
    fn start_call(&self, row: &mut PartRow) -> rusqlite::Result<()> {
        if let Part::ToolCall { status, started_at, .. } = &mut row.part {
            *status = ToolStatus::Running;
            *started_at = Some(id::now_ms());
        }
        self.store.save_part(row)?;
        self.hub.publish(Event::PartUpdated { part: row.clone() });
        Ok(())
    }

    /// Ends the turn by itself, visibly: a reply-less message whose `error` is the reason.
    fn pause(&self, plan: &Plan, reason: String) {
        let Ok(mut message) = self.store.create_message(&plan.session.id, Role::Assistant, Some(&plan.model_ref)) else { return };
        self.hub.publish(Event::MessageCreated { message: message.clone() });
        message.status = MessageStatus::Paused;
        message.error = Some(reason);
        let _ = self.finish(&mut message);
    }

    /// The calls the session's latest reply made, with their inputs and results.
    fn last_calls(&self, session_id: &str) -> Vec<CallTrace> {
        let transcript = self.store.transcript(session_id).unwrap_or_default();
        let Some(last) = transcript.iter().rev().find(|m| m.info.role == Role::Assistant) else { return Vec::new() };
        last.parts
            .iter()
            .filter_map(|row| match &row.part {
                Part::ToolCall { name, input, output, .. } => Some(CallTrace { name: name.clone(), input: input.to_string(), output: output.clone().unwrap_or_default() }),
                _ => None,
            })
            .collect()
    }

    /// Closes the calls a message made that will never run, with the reason, so none stays pending.
    fn settle_unrun(&self, message: &Message, reason: &str) {
        let Ok(transcript) = self.store.transcript(&message.session_id) else { return };
        let Some(found) = transcript.into_iter().find(|m| m.info.id == message.id) else { return };
        for mut row in found.parts {
            if matches!(row.part, Part::ToolCall { status: ToolStatus::Pending, .. }) {
                self.settle(&mut row, ToolStatus::Error, None, format!("Not run: {reason}"), None);
            }
        }
    }

    /// Writes the outcome. If that write fails, what is published is the failure, never a success the store lacks.
    fn settle(&self, row: &mut PartRow, new_status: ToolStatus, new_title: Option<String>, text: String, meta: Option<serde_json::Value>) {
        self.settle_delivering(row, new_status, new_title, text, meta, None);
    }

    /// [`Self::settle`] that also marks `delivers` handed over in the same write; a failed write leaves it owed.
    pub(super) fn settle_delivering(&self, row: &mut PartRow, new_status: ToolStatus, new_title: Option<String>, text: String, meta: Option<serde_json::Value>, delivers: Option<&str>) {
        if let Part::ToolCall { status, title, output, metadata, finished_at, .. } = &mut row.part {
            *status = new_status;
            *title = new_title.or(title.take());
            *output = Some(text);
            *metadata = meta;
            *finished_at = Some(id::now_ms());
        }
        let saved = match delivers {
            Some(task) => self.store.save_part_delivering(row, task).map(|_| ()),
            None => self.store.save_part(row),
        };
        if let Err(error) = &saved {
            if let Part::ToolCall { status, output, .. } = &mut row.part {
                *status = ToolStatus::Error;
                *output = Some(format!("result was not persisted ({error}); treat this call as failed"));
            }
            let _ = self.store.save_part(row);
        }
        self.hub.publish(Event::PartUpdated { part: row.clone() });
        if let (Some(task), Ok(())) = (delivers, saved) {
            self.publish_task(task);
        }
    }
}

enum Step {
    Done,
    Continue,
    /// A failure worth trying again.
    Retry(Retry),
    /// The request no longer fit the model's context.
    Overflow,
}

/// One call as the loop check sees it: what was asked and what came back.
#[derive(Clone, Debug, PartialEq)]
struct CallTrace {
    name: String,
    input: String,
    output: String,
}

/// Words that mark a shell command as waiting on purpose, the way polling does.
const WAITS: [&str; 5] = ["sleep", "start-sleep", "timeout", "wait", "watch"];

/// Steps in a row whose calls and results were all the same. Different results are progress, so a
/// poll whose answer changes never counts; one that waits on purpose gets the larger `polls` allowance.
#[derive(Default)]
struct Repeats {
    last: Vec<CallTrace>,
    count: u32,
}

impl Repeats {
    /// `Some(times)` once the same step has come back as many times in a row as the limit allows.
    fn record(&mut self, calls: Vec<CallTrace>, limits: &crate::config::Limits) -> Option<u32> {
        if calls.is_empty() {
            *self = Self::default();
            return None;
        }
        if calls == self.last {
            self.count += 1;
        } else {
            self.last = calls;
            self.count = 1;
        }
        let limit = if self.last.iter().any(waits) { limits.polls } else { limits.repeats };
        (self.count >= limit).then_some(self.count)
    }
}

fn waits(call: &CallTrace) -> bool {
    let Ok(input) = serde_json::from_str::<serde_json::Value>(&call.input) else { return false };
    let command = input["command"].as_str().unwrap_or_default().to_ascii_lowercase();
    call.name == "bash" && command.split(|c: char| !c.is_ascii_alphanumeric() && c != '-').any(|word| WAITS.contains(&word))
}

struct Retry {
    /// The provider's words, for the UI.
    message: String,
    /// The wait the provider asked for, if it named one.
    after: Option<Duration>,
}

impl Retry {
    fn from(error: &llm::Error) -> Option<Self> {
        match error {
            llm::Error::Api { retryable: true, retry_after, .. } => Some(Self { message: error.to_string(), after: *retry_after }),
            llm::Error::Transport(_) => Some(Self { message: error.to_string(), after: None }),
            _ => None,
        }
    }

    /// Worth waiting for: attempts remain and the provider did not ask for longer than we will wait.
    fn allowed(&self, retries: u32) -> bool {
        retries < MAX_RETRIES && self.after.is_none_or(|after| after <= MAX_REQUESTED_WAIT)
    }

    /// The provider's wait when it named one, else doubling backoff with jitter, capped.
    fn delay(&self, attempt: u32) -> Duration {
        if let Some(after) = self.after {
            return after;
        }
        let doubled = RETRY_BASE.saturating_mul(1 << attempt.saturating_sub(1).min(16));
        let mut byte = [0u8; 1];
        let _ = getrandom::fill(&mut byte);
        doubled.mul_f64(0.8 + 0.4 * f64::from(byte[0]) / 255.0).min(MAX_BACKOFF)
    }
}

enum Wait {
    Elapsed,
    /// The user moved the turn to another model; retry now on it.
    Switched(Box<Resolved>),
    Stopped,
}

/// What a turn offers the model: each tool's spec and the tool itself, held until the turn ends, so an
/// MCP server dropped or replaced meanwhile still serves this turn's calls.
#[derive(Default)]
struct Offer {
    tools: Vec<(llm::ToolSpec, Arc<dyn crate::tool::Tool>)>,
    system: String,
}

impl Offer {
    fn specs(&self) -> Vec<llm::ToolSpec> {
        self.tools.iter().map(|(spec, _)| spec.clone()).collect()
    }

    fn tool(&self, name: &str) -> Option<Arc<dyn crate::tool::Tool>> {
        self.tools.iter().find(|(spec, _)| spec.name == name).map(|(_, tool)| tool.clone())
    }
}

struct CallScope<'a> {
    plan: &'a Plan,
    message: &'a Message,
    files: &'a Arc<SessionFiles>,
    abort: &'a CancellationToken,
}

struct Streamed {
    usage: Usage,
    stop: StopReason,
    calls: Vec<PartRow>,
}

enum StreamError {
    Aborted,
    Provider(llm::Error),
}

fn call_mutates(plan: &Plan, row: &PartRow) -> bool {
    match &row.part {
        Part::ToolCall { name, .. } => plan.offer.tool(name).is_some_and(|tool| tool.mutates()),
        _ => false,
    }
}

/// Identity of a prompt for replay checks: the same id must carry the same parts and model.
pub(super) fn payload_hash(prompt: &Prompt) -> String {
    use sha2::Digest;
    let body = serde_json::json!({ "parts": prompt.parts, "model": prompt.model, "thinking": prompt.thinking_budget });
    sha2::Sha256::digest(body.to_string().as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// The output limit and thinking budget for one request, valid together: the output never exceeds the
/// model's own limit, a thinking budget may raise it past our usual cap but always leaves
/// [`MIN_ANSWER_TOKENS`] for the answer, and a budget that cannot fit is reduced, or dropped when even
/// the provider's minimum would not fit.
fn budgets(model: &Model, requested: Option<u32>) -> (u32, Option<u32>) {
    let model_limit = u32::try_from(model.limit.output).ok().filter(|limit| *limit > 0).unwrap_or(MAX_OUTPUT_TOKENS);
    let Some(wanted) = requested.filter(|_| model.reasoning) else {
        return (model_limit.min(MAX_OUTPUT_TOKENS), None);
    };
    let max_tokens = model_limit.min(MAX_OUTPUT_TOKENS.max(wanted.saturating_add(MIN_ANSWER_TOKENS)));
    let room = max_tokens.saturating_sub(MIN_ANSWER_TOKENS);
    let thinking = (room >= MIN_THINKING_TOKENS).then(|| wanted.clamp(MIN_THINKING_TOKENS, room));
    (max_tokens, thinking)
}

/// Prices are per million tokens.
fn cost(model: &Model, usage: Usage) -> f64 {
    let c = &model.cost;
    (usage.input as f64 * c.input + usage.output as f64 * c.output + usage.cache_read as f64 * c.cache_read + usage.cache_write as f64 * c.cache_write)
        / 1_000_000.0
}

/// What the model is told about a call the user refused.
fn denial(feedback: Option<&str>, stop: bool) -> String {
    let refused = if stop { "The user denied permission for this call and stopped the turn." } else { "The user denied permission for this call." };
    match feedback {
        Some(said) => format!("{refused} They said: {said}"),
        None => refused.to_string(),
    }
}

fn with_formatted(mut metadata: serde_json::Value, formatted: Vec<String>) -> serde_json::Value {
    if !formatted.is_empty() {
        metadata["formatted"] = serde_json::json!(formatted);
    }
    metadata
}

fn merge(metadata: serde_json::Value, extra: Option<serde_json::Value>) -> Option<serde_json::Value> {
    match (metadata, extra) {
        (serde_json::Value::Object(mut base), Some(serde_json::Value::Object(extra))) => {
            base.extend(extra);
            Some(serde_json::Value::Object(base))
        }
        (serde_json::Value::Null, extra) => extra,
        (metadata, _) => Some(metadata),
    }
}

#[cfg(test)]
pub(crate) mod tests;
