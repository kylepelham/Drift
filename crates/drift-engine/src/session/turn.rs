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
use crate::llm::catalog::{self, Model, Reasoning, Variant};
use crate::llm::{self, Credential, Provider, Request, StopReason};
use crate::permission::{self, Outcome};
use crate::session::types::{Message, MessageStatus, MessageWithParts, ModelRef, Part, PartRow, Role, Session, ToolStatus, Usage, Visibility};
use super::tasks::Claimant;
use crate::store::{Admit, Admitted, Handover, Pick};
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
/// A finished reply's `error` when the provider's safety filter ended it, so it never ends in silence.
pub const REFUSED_ENDING: &str = "The provider's safety filter ended the reply.";
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
    /// The model's reasoning level by variant name: absent keeps the session's, null asks for the model's default.
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, nullable = true)]
    pub variant: Option<Option<String>>,
    /// The agent the session runs as from this prompt on; absent keeps the session's. Only a primary agent of the workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Client-chosen id; resubmitting with the same id returns the original receipt instead of a second turn.
    #[serde(default)]
    pub submission_id: Option<String>,
}

/// A field that is present, even as null, is `Some`; only an absent one stays `None`.
pub(crate) fn present<'de, D: serde::Deserializer<'de>>(value: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(value).map(Some)
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
    /// The sign-in's token could not be renewed; carries the provider's reason.
    SignInExpired(String),
    /// A config file could not be read, so its rules are unknown; says which and why.
    Config(String),
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
    /// The prompt chose an agent the workspace has no primary agent by.
    UnknownAgent,
    /// This submission already landed as this message; resolved to its receipt before any caller sees it.
    Replayed(String),
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
            Self::Config(problem) => write!(f, "{problem}"),
            Self::SignInExpired(reason) => write!(f, "the sign-in has expired and could not be renewed; sign in again under Settings > Providers ({reason})"),
            Self::NotRetrying => write!(f, "the session is not waiting to retry"),
            Self::Reverted => write!(f, "the session is undone; send a prompt or redo first"),
            Self::Attachment(message) => write!(f, "{message}"),
            Self::Store(message) => write!(f, "store: {message}"),
            Self::Stopped => write!(f, "stopped before it started"),
            Self::Moved => write!(f, "the session moved to another workspace while it waited"),
            Self::UnknownAgent => write!(f, "no agent by that name can run a conversation in this workspace"),
            Self::Replayed(message) => write!(f, "already admitted as {message}"),
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
    /// What each session's checks last said, by check and file, so an unchanged report is not sent again.
    checked: Mutex<HashMap<String, HashMap<String, String>>>,
    /// What each session answered about running its project's own commands.
    pub(super) trust: super::trust::Answers,
    refreshing: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Automatic compactions that failed in a row, per session; enough of them turn it off for that session.
    pub(super) compaction_failures: Mutex<HashMap<String, u32>>,
    /// How each subagent's last turn ended, until the task waiting on it takes the answer.
    ended: Mutex<HashMap<String, TurnEnd>>,
    /// Turns waiting out a retry backoff, each ready to take a model the user switches to.
    retry_waits: Mutex<HashMap<String, tokio::sync::oneshot::Sender<Switch>>>,
    /// Sessions whose running job is a turn still taking prompts sent while it runs. Admitting one and
    /// a turn deciding it is done both hold this lock, so no prompt lands after the turn stops looking.
    /// Each with what its turn is running as, which is what a steered prompt is judged against.
    steering: Mutex<HashMap<String, Steering>>,
    /// The prompt each running turn began at: a fork stops before it, whatever is steered in later.
    began: Mutex<HashMap<String, String>>,
    /// Tests swap the wire adapter for a scripted one.
    pub provider_override: Mutex<Option<Provider>>,
}

/// The model a running turn makes its next request on, which a steered prompt's files are judged against.
#[derive(Clone, Debug, PartialEq)]
struct Steering {
    model: ModelRef,
    agent: String,
    config: Arc<Config>,
    workspace: PathBuf,
    catalog: Arc<crate::llm::catalog::Catalog>,
    mcp_commands: Vec<crate::config::Command>,
    /// A command's turn: a prompt steered in is the session's, not the command's.
    turn_only: bool,
}

impl Steering {
    fn of(plan: &Plan) -> Self {
        Self { model: plan.model_ref.clone(), agent: plan.session.agent.clone(), config: plan.config.clone(), workspace: plan.workspace.clone(), catalog: plan.catalog.clone(), mcp_commands: plan.mcp_commands.clone(), turn_only: plan.turn_only }
    }

    /// The model and agent a steered prompt runs as when it names neither: the session's own during a command's turn.
    fn defaults(&self, session: Option<&Session>) -> (ModelRef, String) {
        match session.filter(|_| self.turn_only) {
            Some(session) => (session.model.clone().unwrap_or_else(|| self.model.clone()), session.agent.clone()),
            None => (self.model.clone(), self.agent.clone()),
        }
    }
}

/// What a variant name asks of a model offering `variants`; a name it does not offer asks nothing.
fn reasoning_in(variants: &[Variant], name: Option<&str>) -> Option<Reasoning> {
    let name = name?;
    variants.iter().find(|variant| variant.name == name).map(|variant| variant.reasoning.clone())
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

    pub(super) fn cancellation(&self, session_id: &str) -> CancellationToken {
        self.active.lock().unwrap().get(session_id).cloned().unwrap_or_default()
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

    /// The prompt the session's running turn began at, if a turn is running.
    pub(super) fn began(&self, session_id: &str) -> Option<String> {
        self.began.lock().unwrap().get(session_id).cloned()
    }

    /// Loads the session's persisted read record on first access.
    pub(super) fn files_for(&self, store: &Arc<crate::store::Store>, session_id: &str) -> Arc<SessionFiles> {
        self.files.lock().unwrap().entry(session_id.into()).or_insert_with(|| Arc::new(SessionFiles::kept(store.clone(), session_id))).clone()
    }

    /// Forgets what the session's checks said, once the model may no longer see it (compaction, undo), so the next report is sent in full.
    pub(super) fn forget_checked(&self, session_id: &str) {
        self.checked.lock().unwrap().remove(session_id);
    }

    /// Records what a check said (`None` when it passed); whether the session was told exactly this last time.
    fn repeated(&self, session_id: &str, label: &str, said: Option<&str>) -> bool {
        let mut checked = self.checked.lock().unwrap();
        let session = checked.entry(session_id.into()).or_default();
        match said {
            Some(said) => session.insert(label.into(), said.into()).as_deref() == Some(said),
            None => {
                session.remove(label);
                false
            }
        }
    }

    pub(super) fn refresh_lock(&self, provider: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.refreshing.lock().unwrap().entry(provider.into()).or_default().clone()
    }

    pub fn is_running(&self, session_id: &str) -> bool {
        self.active.lock().unwrap().contains_key(session_id)
    }

    /// Whether any session has a job running.
    pub fn any_running(&self) -> bool {
        !self.active.lock().unwrap().is_empty()
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

/// Everything a turn runs on: fixed when it is admitted (a queued worker keeps what it was given), then following the session's choice step by step.
pub(crate) struct Plan {
    pub(super) session: Session,
    pub(super) workspace: PathBuf,
    pub(super) config: Arc<Config>,
    pub(super) catalog: Arc<crate::llm::catalog::Catalog>,
    pub(super) model_ref: ModelRef,
    pub(super) model: Model,
    pub(super) provider: Provider,
    pub(super) credential: Credential,
    /// The reasoning variant by name, looked up on each request so a switched model reads it as its own.
    variant: Option<String>,
    offer: Offer,
    mcp_tools: Vec<(llm::ToolSpec, Arc<dyn crate::tool::Tool>)>,
    mcp_servers: Vec<(String, String)>,
    mcp_commands: Vec<crate::config::Command>,
    bootstrap: Vec<super::command::Bootstrap>,
    /// Its agent and model are a command's, for this turn only: it does not follow the session's.
    turn_only: bool,
}

impl Plan {
    /// A tool this turn offers, as it was offered.
    pub(super) fn offered(&self, name: &str) -> Option<Arc<dyn crate::tool::Tool>> {
        self.offer.tool(name)
    }

    /// What the variant asks of the model now planned; a name this model does not offer asks nothing.
    fn reasoning(&self) -> Option<Reasoning> {
        reasoning_in(&self.model.variants, self.variant.as_deref())
    }
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
    pub(super) bootstrap: &'a [super::command::Bootstrap],
    /// The prompt's agent and model run this turn only; the session keeps its own, so it never steers.
    pub(super) turn_only: bool,
    pub(super) config: Option<&'a Config>,
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

    /// A replay found inside admission's own write is the original receipt, as one found before it is.
    pub(super) async fn admit(self: &Arc<Self>, session_id: &str, prompt: Prompt, how: Admission<'_>) -> Result<Receipt, TurnError> {
        match self.admit_once(session_id, prompt, how).await {
            Err(TurnError::Replayed(message_id)) => self.receipt_for(session_id, &message_id),
            other => other,
        }
    }

    async fn admit_once(self: &Arc<Self>, session_id: &str, prompt: Prompt, how: Admission<'_>) -> Result<Receipt, TurnError> {
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
            if !how.bootstrap.is_empty() || how.turn_only {
                return Err(TurnError::Busy);
            }
            return self.steer_or_wait(session_id, prompt, how, &payload_hash).await;
        }
        if how.steer_only {
            self.turns.release(session_id);
            return Err(TurnError::Stopped);
        }
        let planned = tokio::select! {
            planned = self.plan_for(session_id, &prompt, how.turn_only, how.config) => planned,
            () = abort.cancelled() => Err(TurnError::Stopped),
        };
        match planned {
            Ok(mut plan) => {
                plan.bootstrap = how.bootstrap.to_vec();
                self.start(session_id, prompt, plan, abort, &payload_hash, how)
            }
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
        self.start(session_id, prompt, plan, abort, &hash, Admission::default())
    }

    async fn refresh_plan(&self, plan: &mut Plan) -> Result<(), TurnError> {
        let session = self.store.session(&plan.session.id)?.ok_or(TurnError::NoSession)?;
        if session.workspace_id != plan.session.workspace_id {
            return Err(TurnError::Moved);
        }
        let provider = plan.model_ref.provider.clone();
        let (env, api) = plan.catalog.providers.get(&provider).map(|p| (p.env.clone(), p.api.clone())).unwrap_or_default();
        let stored = self.credentials.resolve(&provider, &env).ok_or(TurnError::NoCredentials)?;
        super::oneshot::refuse_signin_elsewhere(&provider, &stored, api.as_deref())?;
        plan.credential = self.fresh_credential(&provider, stored).await?;
        Ok(())
    }

    /// Admits the prompt into a session this call has claimed and starts its turn; releases the claim if it cannot.
    fn start(self: &Arc<Self>, session_id: &str, prompt: Prompt, plan: Plan, abort: CancellationToken, payload_hash: &str, how: Admission) -> Result<Receipt, TurnError> {
        let attach = Attach { engine: self, session_id, workspace: &plan.workspace, policy: &plan.config.policy(), agent_policy: &plan.config.agent_policy(&plan.session.agent), model: &plan.model };
        let submission = prompt.submission_id.as_deref().map(|id| (id, payload_hash));
        let pick = Pick { model: &plan.model_ref, variant: prompt.variant.as_ref().map(Option::as_deref), agent: prompt.agent.as_deref(), sticky: !plan.turn_only };
        let admitted = attach.prepare(prompt.parts).and_then(|prepared| {
            let admitted = self.admit_fenced(session_id, pick, prepared.parts, submission, Some(&abort), how.delivery)?;
            self.count_as_read(session_id, &prepared.read);
            Ok(admitted)
        });
        let admitted = match admitted {
            Ok(admitted) => admitted,
            Err(error) => {
                self.turns.release(session_id);
                return Err(error);
            }
        };
        let receipt = self.announce(session_id, admitted);
        self.turns.began.lock().unwrap().insert(session_id.into(), receipt.message.id.clone());
        self.turns.steering.lock().unwrap().insert(session_id.into(), Steering::of(&plan));
        let engine = self.clone();
        self.spawn_job(session_id, async move { engine.run(plan, abort).await });
        Ok(receipt)
    }

    /// The last check before a prompt is written, under the lock every Stop holds, so it lands wholly before a Stop or not at all.
    pub(super) fn admit_fenced(&self, session_id: &str, pick: Pick, parts: Vec<Part>, submission: Option<(&str, &str)>, abort: Option<&CancellationToken>, delivery: Option<&str>) -> Result<Admitted, TurnError> {
        let _fence = self.workers.fence();
        if abort.is_some_and(CancellationToken::is_cancelled) {
            return Err(TurnError::Stopped);
        }
        // Results a Stop held back ride along with any admitted prompt, each claimed so no other path takes it meanwhile.
        let held: Vec<_> = self.store.held_tasks(session_id)?.into_iter().filter(|task| self.workers.claim(&task.id, Claimant::Automatic)).collect();
        let carried = held.iter().map(|task| (task.id.clone(), super::tasks::result_part(task))).collect();
        let admitted = self.store.admit_delivering(session_id, pick, parts, submission, Handover { delivery, held: carried });
        for task in &held {
            self.workers.release_where_task(&task.id, &Claimant::Automatic);
            self.publish_task(&task.id);
        }
        match admitted? {
            Admit::New(admitted) => Ok(*admitted),
            Admit::Replayed { message_id } => Err(TurnError::Replayed(message_id)),
            // A different prompt under the same id, or a result already handed over: nothing is written.
            Admit::Conflict | Admit::Delivered => Err(TurnError::SubmissionReused),
        }
    }

    /// The receipt of a prompt that already landed under the same submission id.
    pub(super) fn receipt_for(&self, session_id: &str, message_id: &str) -> Result<Receipt, TurnError> {
        let session = self.store.session(session_id)?.ok_or(TurnError::NoSession)?;
        let message = self.store.message(message_id)?.ok_or(TurnError::NoSession)?;
        Ok(Receipt { session, message })
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
    /// calls in flight finish, so their results come first), switching to any model, agent or level it
    /// names. Any other job, such as a compaction or an undo, is waited out for up to [`QUEUE_WAIT`],
    /// then the prompt starts a turn of its own.
    async fn steer_or_wait(self: &Arc<Self>, session_id: &str, prompt: Prompt, how: Admission<'_>, payload_hash: &str) -> Result<Receipt, TurnError> {
        // A model the turn would switch to is checked now, so a bad choice fails for the sender, not inside the turn.
        if let Some(model) = &prompt.model {
            let running = self.turns.steering.lock().unwrap().get(session_id).cloned();
            if let Some(running) = running.filter(|running| running.model != *model) {
                self.resolve_from(model, &running.catalog).await?;
            }
        }
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
    /// prompts. Its files are judged against the model the turn's next request runs on: the one the
    /// prompt names, or the one the turn is on.
    fn steer(&self, session_id: &str, prompt: &Prompt, payload_hash: &str, how: Admission<'_>) -> Result<Option<Receipt>, TurnError> {
        let Some(running) = self.turns.steering.lock().unwrap().get(session_id).cloned() else { return Ok(None) };
        let session = if running.turn_only { self.store.session(session_id)? } else { None };
        let (own_model, own_agent) = running.defaults(session.as_ref());
        let target = prompt.model.clone().unwrap_or(own_model);
        let model = running.catalog.providers.get(&target.provider).and_then(|p| p.models.get(&target.model)).cloned().ok_or(TurnError::UnknownModel)?;
        let workspace = &running.workspace;
        let config = &running.config;
        if let Some(agent) = &prompt.agent {
            pickable(config, agent)?;
        }
        let policy = config.policy();
        let agent_policy = config.agent_policy(prompt.agent.as_deref().unwrap_or(&own_agent));
        let prepared = Attach { engine: self, session_id, workspace, policy: &policy, agent_policy: &agent_policy, model: &model }.prepare(prompt.parts.clone())?;
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
        // Written to the session as it lands; the turn reads the session before its next request and follows it.
        let pick = Pick { model: &target, variant: prompt.variant.as_ref().map(Option::as_deref), agent: prompt.agent.as_deref(), sticky: true };
        let admitted = self.admit_fenced(session_id, pick, prepared.parts, submission, how.parent, how.delivery)?;
        drop(steering);
        self.count_as_read(session_id, &prepared.read);
        Ok(Some(self.announce(session_id, admitted)))
    }

    /// Files a prompt showed in full count as read once it is admitted, not before: a refused prompt showed nothing.
    fn count_as_read(&self, session_id: &str, paths: &[PathBuf]) {
        let files = self.turns.files_for(&self.store, session_id);
        for path in paths {
            files.mark_read(path);
        }
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
            engine.turns.began.lock().unwrap().remove(&id);
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
        self.plan_for(session_id, prompt, false, None).await
    }

    async fn plan_for(&self, session_id: &str, prompt: &Prompt, turn_only: bool, config: Option<&Config>) -> Result<Plan, TurnError> {
        let mut session = self.store.session(session_id)?.ok_or(TurnError::NoSession)?;
        let workspace = self.store.workspace(&session.workspace_id)?.ok_or(TurnError::NoWorkspace)?;
        self.bind_permissions(&session.id, &session.workspace_id);
        let workspace_path = crate::tool::canonical(Path::new(&workspace.path));
        let config = config.cloned().unwrap_or_else(|| self.workspace_config(&workspace_path));
        if let Some(problem) = config.problems.first() {
            return Err(TurnError::Config(problem.clone()));
        }
        if let Some(agent) = &prompt.agent {
            pickable(&config, agent)?;
            session.agent = agent.clone();
        }
        if let Some(agent) = config.agent(&session.agent) {
            agent.usable().map_err(TurnError::Config)?;
        }
        let agent_model = config.agent(&session.agent).and_then(|a| a.model.clone());
        let model_ref = prompt.model.clone().or_else(|| session.model.clone()).or(agent_model).or_else(|| config.model.clone()).ok_or(TurnError::NoModel)?;
        let catalog = Arc::new(self.catalog_view());
        let resolved = self.resolve_from(&model_ref, &catalog).await?;
        let provider = resolved.provider.with_timeouts(config.route_timeouts(&resolved.model_ref.provider));
        let variant = prompt.variant.clone().unwrap_or_else(|| session.variant.clone()).or_else(|| config.agent(&session.agent).and_then(|agent| agent.variant.clone()));
        let mut plan = Plan {
            session,
            workspace: workspace_path,
            config: Arc::new(config),
            catalog,
            model_ref: resolved.model_ref,
            model: resolved.model,
            provider,
            credential: resolved.credential,
            variant,
            offer: Offer::default(),
            mcp_tools: Vec::new(),
            mcp_servers: Vec::new(),
            mcp_commands: Vec::new(),
            bootstrap: Vec::new(),
            turn_only,
        };
        // A server connecting right now would otherwise be missing from this turn's tools.
        self.mcp.wait_ready(crate::mcp::READY_WAIT).await;
        self.mcp.refresh_stale(&self.store, &self.hub).await;
        plan.mcp_tools = self.mcp.tools(&self.store).into_iter().map(|tool| (tool.spec(), tool)).collect();
        plan.mcp_servers = self.mcp.instructions();
        plan.mcp_commands = self.mcp.prompt_commands();
        plan.offer = self.offer(&plan);
        Ok(plan)
    }

    /// Expired subscription tokens are refreshed before they are sent.
    pub(super) async fn fresh_credential(&self, provider: &str, credential: Credential) -> Result<Credential, TurnError> {
        if !credential.is_expired() {
            return Ok(credential);
        }
        self.renew(provider, credential).await
    }

    /// The provider's stored credential, renewed first when it is a sign-in past its expiry, for
    /// callers outside a turn (the shell's usage limits); `None` when there is none or it cannot be renewed.
    pub async fn current_credential(&self, provider: &str) -> Option<Credential> {
        let stored = self.credentials.get(provider)?;
        if !stored.is_expired() {
            return Some(stored);
        }
        self.renew(provider, stored).await.ok()
    }

    /// A new token for a sign-in that expired or was refused, refreshed once however many turns ask at the same time.
    pub(super) async fn renew(&self, provider: &str, credential: Credential) -> Result<Credential, TurnError> {
        let lock = self.turns.refresh_lock(provider);
        let _held = lock.lock().await;
        // Another turn, or the user signing in again, may have replaced it while we waited; that token is the one to use.
        if let Some(stored) = self.credentials.get(provider).filter(|stored| *stored != credential && !stored.is_expired()) {
            return Ok(stored);
        }
        let Credential::OAuth { refresh, .. } = &credential else { return Ok(credential) };
        let refreshed = match provider {
            "anthropic" => llm::anthropic::oauth::refresh(&self.http, refresh).await,
            "openai" => llm::openai::oauth::refresh(&self.http, refresh).await,
            "xai" => llm::xai::refresh(&self.http, refresh).await,
            _ => return Ok(credential),
        };
        let fresh = refreshed.map_err(TurnError::SignInExpired)?;
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
        // The prompt this turn answers; replies before it belong to turns already over.
        let started = self.store.newest_prompt(&plan.session.id).ok().flatten();
        if !self.run_bootstrap(&mut plan, &abort).await {
            self.turns.steering.lock().unwrap().remove(&plan.session.id);
            self.record_end(&plan.session, &abort);
            return;
        }
        loop {
            let answered = self.run_steps(&mut plan, &abort, started.as_deref()).await;
            if abort.is_cancelled() || !self.carries_on(&plan, answered.as_deref(), &abort) {
                break;
            }
        }
        self.turns.steering.lock().unwrap().remove(&plan.session.id);
        self.record_end(&plan.session, &abort);
    }

    /// The calls a command makes before the model answers (a skill, a delegated task, its shell lines),
    /// in one message, each through the permission check as the model's own calls are.
    async fn run_bootstrap(self: &Arc<Self>, plan: &mut Plan, abort: &CancellationToken) -> bool {
        let bootstraps = std::mem::take(&mut plan.bootstrap);
        if bootstraps.is_empty() {
            return true;
        }
        let prepared = (|| -> rusqlite::Result<_> {
            let mut message = self.store.create_reply(&plan.session.id, &plan.model_ref, &plan.session.agent)?;
            self.hub.publish(Event::MessageCreated { message: message.clone() });
            let mut rows = Vec::new();
            for bootstrap in bootstraps {
                let mut metadata = json!({ "engineCommand": bootstrap.command });
                if let Some(model) = &bootstrap.model {
                    metadata["commandModel"] = json!(format!("{}/{}", model.provider, model.model));
                }
                let part = Part::ToolCall { call_id: id::new("call"), name: bootstrap.tool, input: bootstrap.input, status: ToolStatus::Pending, title: None, output: None, metadata: Some(metadata), started_at: None, finished_at: None };
                let row = self.store.add_part(&message.id, &plan.session.id, part)?;
                self.hub.publish(Event::PartCreated { part: row.clone() });
                rows.push(row);
            }
            message.status = MessageStatus::Done;
            self.finish(&mut message)?;
            Ok((message, rows))
        })();
        let (message, rows) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => { self.pause(plan, format!("could not record command execution: {error}")); return false; }
        };
        self.run_calls(plan, &message, rows, super::early::Early::new(abort), abort).await != Outcome::Aborted
    }

    pub(crate) fn command_config(&self, session_id: &str, workspace: &Path) -> Config {
        let running = self.turns.steering.lock().unwrap().get(session_id).cloned();
        let (mut config, commands) = match running {
            Some(running) => ((*running.config).clone(), running.mcp_commands),
            None => (self.workspace_config(workspace), self.mcp.prompt_commands()),
        };
        config.commands.extend(commands);
        config
    }

    /// Under the steering lock: whether a prompt arrived after the last one this turn answered, or
    /// the orchestrator driver sent one now. If neither, the turn stops taking prompts in the same
    /// breath, so none can land unanswered.
    fn carries_on(&self, plan: &Plan, answered: Option<&str>, abort: &CancellationToken) -> bool {
        let session_id = plan.session.id.as_str();
        let mut steering = self.turns.steering.lock().unwrap();
        let newest = self.store.newest_prompt(session_id).ok().flatten();
        let steered = matches!((newest.as_deref(), answered), (Some(newest), Some(answered)) if newest > answered);
        if steered || self.nudge(plan, abort) {
            return true;
        }
        steering.remove(session_id);
        false
    }

    /// Sends the orchestrator's next prompt when its last reply says it is still working; false when the turn should end.
    fn nudge(&self, plan: &Plan, abort: &CancellationToken) -> bool {
        let session_id = plan.session.id.as_str();
        if plan.turn_only || plan.session.agent != super::drive::AGENT {
            return false;
        }
        let Ok(Some(reply)) = self.store.last_reply(session_id) else { return false };
        let rounds = self.store.nudges_since_prompt(session_id).unwrap_or(usize::MAX);
        let Some(text) = super::drive::next(&plan.session, &reply, rounds) else { return false };
        let pick = Pick { model: &plan.model_ref, variant: None, agent: None, sticky: true };
        match self.admit_fenced(session_id, pick, vec![Part::Nudge { text: text.into() }], None, Some(abort), None) {
            Ok(admitted) => {
                self.announce(session_id, admitted);
                true
            }
            Err(_) => false,
        }
    }

    /// A step's request on the request window, as every step of a turn builds it, so one built
    /// elsewhere (a summary on the conversation's own model) reads the same cached prefix. `closing`
    /// is a last user instruction; it forbids tool calls. Also returns the newest prompt it includes.
    pub(super) fn step_request(&self, plan: &Plan, mut transcript: Vec<MessageWithParts>, started: Option<&str>, closing: Option<String>) -> (Request, Option<String>) {
        super::branch::frame_spawned(&plan.session, &mut transcript);
        let lead = prompt::remind_agents(&plan.config, &plan.session.agent, &mut transcript);
        super::convert::drop_earlier_reasoning(&mut transcript, started);
        let answered = transcript.iter().rev().find(|m| m.info.role == Role::User).map(|m| m.info.id.clone());
        let provider = plan.model_ref.provider.as_str();
        let (max_tokens, reasoning) = budgets(&plan.model, plan.reasoning().or_else(|| catalog::default_reasoning(provider, &plan.model)));
        let sampling = catalog::sampling(&plan.model);
        let target = super::convert::OnCatalog { model: &plan.model_ref, catalog: &plan.catalog };
        let mut messages = compaction::request_messages(&transcript, &target, &lead);
        let no_tool_calls = closing.is_some();
        if let Some(text) = closing {
            super::convert::push(&mut messages, llm::Role::User, vec![llm::Block::Text(text)]);
        }
        let request = Request {
            model: plan.model.wire(&plan.model_ref.model).to_string(),
            system: plan.offer.system.clone(),
            messages: llm::prepare_files(messages, &plan.model, |hash| self.store.blob(hash).ok().flatten()),
            tools: plan.offer.specs(),
            max_tokens,
            reasoning,
            temperature: sampling.temperature,
            cache_key: Some(plan.session.id.clone()),
            no_tool_calls,
            verbosity: catalog::verbosity(provider, &plan.model),
            show_thinking: catalog::shows_thinking(provider, &plan.model),
            top_p: sampling.top_p,
            top_k: sampling.top_k,
            mode: plan.model.mode.clone(),
        };
        (request, answered)
    }

    /// One run of model steps; returns the newest prompt the last request included.
    async fn run_steps(self: &Arc<Self>, plan: &mut Plan, abort: &CancellationToken, started: Option<&str>) -> Option<String> {
        let mut attempts = 0;
        let mut recovered = false;
        let mut steps = 0;
        let mut repeats = Repeats::default();
        let mut answered = None;
        let mut wrapping = None;
        loop {
            if let Err(reason) = self.follow_session(plan).await {
                self.pause(plan, reason);
                break;
            }
            let limits = plan.config.limits_for(&plan.session.agent);
            if wrapping.is_none() && steps + 1 >= limits.steps {
                wrapping = Some(WrapUp::Steps(limits.steps));
            }
            let Some(transcript) = self.transcript_for_step(plan, abort).await else { break };
            let wrap_up = wrapping.map(|wrap_up| wrap_up.instruction());
            let request;
            (request, answered) = self.step_request(plan, transcript, started, wrap_up);
            let Ok(message) = self.store.create_reply(&plan.session.id, &plan.model_ref, &plan.session.agent) else { break };
            self.hub.publish(Event::MessageCreated { message: message.clone() });
            match self.step(plan, message, &request, abort).await {
                Step::Done | Step::Continue if wrapping.is_some() => {
                    self.end_wrap_up(plan, wrapping);
                    break;
                }
                Step::Done => break,
                Step::Continue => {
                    attempts = 0;
                    steps += 1;
                    // One more request, without tools, so what the turn found is written up rather than lost.
                    wrapping = repeats.record(self.last_calls(&plan.session.id), &limits).map(WrapUp::Repeats);
                }
                Step::Retry(retry) if retry.allowed(attempts) => {
                    attempts += 1;
                    match self.wait_to_retry(&plan.session.id, attempts, &retry, abort).await {
                        Wait::Elapsed => {}
                        Wait::Switched(switch) => {
                            self.adopt(plan, *switch);
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

    /// Before each request: a model, agent or level a prompt chose since the last one, written on the
    /// session as it landed, becomes the turn's, so the conversation carries on as that choice.
    async fn follow_session(&self, plan: &mut Plan) -> Result<(), String> {
        // A command's agent and model last until the user steers in a prompt of their own; from then on the turn is the session's.
        if plan.turn_only {
            let newest = self.store.newest_prompt(&plan.session.id).ok().flatten();
            if newest.is_none() || newest == self.turns.began(&plan.session.id) {
                return Ok(());
            }
            plan.turn_only = false;
            if let Some(running) = self.turns.steering.lock().unwrap().get_mut(&plan.session.id) {
                running.turn_only = false;
            }
        }
        let Ok(Some(session)) = self.store.session(&plan.session.id) else { return Ok(()) };
        let model = session.model.clone().filter(|model| *model != plan.model_ref);
        let variant = session.variant.clone().or_else(|| plan.config.agent(&session.agent).and_then(|agent| agent.variant.clone()));
        if model.is_none() && session.agent == plan.session.agent && variant == plan.variant {
            return Ok(());
        }
        if session.agent != plan.session.agent {
            pickable(&plan.config, &session.agent).map_err(|error| error.to_string())?;
        }
        if let Some(model) = model {
            let resolved = self.resolve_from(&model, &plan.catalog).await.map_err(|error| format!("Could not switch to {}: {error}. Send a message to carry on.", model.model))?;
            plan.model_ref = resolved.model_ref;
            plan.model = resolved.model;
            plan.provider = resolved.provider.with_timeouts(plan.config.route_timeouts(&plan.model_ref.provider));
            plan.credential = resolved.credential;
        }
        plan.session.agent = session.agent;
        plan.variant = variant;
        plan.offer = self.offer(plan);
        if let Some(running) = self.turns.steering.lock().unwrap().get_mut(&plan.session.id) {
            *running = Steering::of(plan);
        }
        Ok(())
    }

    /// The tools and system prompt for the plan's model and agent. What was offered is what may run:
    /// a call to any other tool is refused before permission or snapshot.
    fn offer(&self, plan: &Plan) -> Offer {
        let agent = plan.config.agent(&plan.session.agent).cloned();
        let subagent = plan.session.visibility == Visibility::Hidden;
        let tools: Vec<_> = self.tools
            .offered(plan.model.profile)
            .into_iter()
            .map(|tool| (tool.spec(), tool))
            .chain(plan.mcp_tools.iter().cloned())
            .filter(|(spec, _)| agent.as_ref().is_none_or(|agent| agent.allows_tool(&spec.name)))
            .filter(|(spec, _)| !(subagent && crate::tool::task::DELEGATION.contains(&spec.name.as_str())))
            .collect();
        // A server's instructions come only with its tools, so an agent without them is not told about it.
        let servers: Vec<(String, String)> = plan.mcp_servers.iter().filter(|(server, _)| tools.iter().any(|(_, tool)| tool.server() == Some(server.as_str()))).cloned().collect();
        let base = prompt::base_for(&self.store, plan.model.prompt);
        let rules = self.permissions.compiled(&plan.config.policy(), &plan.config.agent_policy(&plan.session.agent));
        let denied = |kind: &str, name: &str| rules.explicit(&crate::tool::Ask::new(kind, name, "")) == Some(crate::permission::Decision::Deny);
        let setting = prompt::Setting {
            base: &base,
            workspace: &plan.workspace,
            config: &plan.config,
            agent: agent.as_ref(),
            delegates: tools.iter().any(|(spec, _)| spec.name == "task"),
            loads_skills: tools.iter().any(|(spec, _)| spec.name == "skill"),
            denied: &denied,
            model: &plan.model.name,
            servers: &servers,
        };
        let system = prompt::system(&setting);
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
            Ok(switch) = receiver => Wait::Switched(Box::new(switch)),
            () = abort.cancelled() => Wait::Stopped,
        };
        self.turns.retry_waits.lock().unwrap().remove(session_id);
        self.hub.publish(Event::SessionStatusChanged { session_id: session_id.into(), status: SessionStatus::Running });
        wait
    }

    /// Moves the turn onto a model the user switched to, and makes it, and any variant chosen with it, the session's from now on.
    fn adopt(&self, plan: &mut Plan, switch: Switch) {
        let Switch { resolved, variant } = switch;
        if let Some(variant) = variant {
            let _ = self.store.set_session_variant(&plan.session.id, variant.as_deref());
            plan.variant = variant.or_else(|| plan.config.agent(&plan.session.agent).and_then(|agent| agent.variant.clone()));
        }
        plan.model_ref = resolved.model_ref;
        plan.model = resolved.model;
        if let Some(running) = self.turns.steering.lock().unwrap().get_mut(&plan.session.id) {
            *running = Steering::of(plan);
        }
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
    pub async fn switch_retry_model(&self, session_id: &str, model: &ModelRef, variant: Option<Option<String>>) -> Result<(), TurnError> {
        if !self.turns.retry_waits.lock().unwrap().contains_key(session_id) {
            return Err(TurnError::NotRetrying);
        }
        let running = self.turns.steering.lock().unwrap().get(session_id).cloned().ok_or(TurnError::NotRetrying)?;
        let resolved = self.resolve_from(model, &running.catalog).await?;
        let waiting = self.turns.retry_waits.lock().unwrap().remove(session_id).ok_or(TurnError::NotRetrying)?;
        waiting.send(Switch { resolved, variant }).map_err(|_| TurnError::NotRetrying)
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
            match self.store.last_reply(&session.id).ok().flatten() {
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
        let transcript = self.request_window(&plan.session.id)?;
        if !self.wants_compaction(&plan.session.id, &plan.model, &transcript) {
            return Some(transcript);
        }
        let _ = self.compact(&plan.session.id, Trigger::Auto, abort).await;
        if abort.is_cancelled() {
            return None;
        }
        self.request_window(&plan.session.id)
    }

    /// The part of the transcript a request can show: from the latest summary's kept tail on, or all
    /// of it before any compaction. History already summarised is never loaded.
    pub(super) fn request_window(&self, session_id: &str) -> Option<Vec<MessageWithParts>> {
        let Some(start) = self.store.view_start(session_id).ok()? else { return self.store.transcript(session_id).ok() };
        let mut window = self.store.messages_from(session_id, &start).ok()?;
        // A summary with no text stands for nothing, so the view reaches back past it.
        if compaction::view(&window).summary.is_none() {
            return self.store.transcript(session_id).ok();
        }
        // A tail kept from inside a turn brings that turn's prompt along, alone, for the view to quote.
        if window.first().is_some_and(|first| first.info.role == Role::Assistant) {
            if let Ok(Some(prompt)) = self.store.prompt_before(session_id, &start) {
                window.insert(0, prompt);
            }
        }
        Some(window)
    }

    /// One assistant message and the tool calls it makes.
    async fn step(self: &Arc<Self>, plan: &mut Plan, mut message: Message, request: &Request, abort: &CancellationToken) -> Step {
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
        // A reply that did not end on its own runs nothing, since a call's input may be cut short.
        let ending = match streamed.stop {
            StopReason::MaxTokens => Some((format!("{OUTPUT_LIMIT_ENDING} ({} tokens).", request.max_tokens), "the reply hit its output limit, so this call's input may be cut short.")),
            StopReason::Refused => Some((REFUSED_ENDING.to_string(), "the provider's safety filter ended the reply before this call ran.")),
            // Not replayed: the turn compacts and asks again, as for a request the provider refused as too long.
            StopReason::ContextFull => {
                message.status = MessageStatus::Error;
                Some(("The reply ran into the end of the context window.".to_string(), "the reply ran out of context, so this call's input may be cut short."))
            }
            _ => None,
        };
        if let Some((error, _)) = &ending {
            message.error = Some(error.clone());
        }
        message.ending = match streamed.stop {
            StopReason::MaxTokens => Some(super::types::Ending::Length),
            StopReason::Refused => Some(super::types::Ending::Refused),
            // Only a wrap-up turns tools off within a turn.
            _ if request.no_tool_calls => Some(super::types::Ending::Limit),
            _ => None,
        };
        if self.finish(&mut message).is_err() {
            return Step::Done;
        }
        if let Some((_, unrun)) = ending {
            self.settle_unrun(&message, unrun);
            return if streamed.stop == StopReason::ContextFull { Step::Overflow } else { Step::Done };
        }
        if streamed.calls.is_empty() {
            return Step::Done;
        }
        // A server that ignores `tool_choice: none` still gets nothing run.
        if request.no_tool_calls {
            self.settle_unrun(&message, "tools were off for this reply, so this call was not run.");
            return Step::Done;
        }
        match self.run_calls(plan, &message, streamed.calls, streamed.early, abort).await {
            Outcome::Aborted => {
                // Calls queued behind the one that stopped never started; none is left pending.
                self.settle_unrun(&message, "the turn was stopped before this call ran.");
                Step::Done
            }
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

    /// Opens the response. A refused sign-in is renewed once and the request sent again; the turn keeps the new token.
    async fn open_response(&self, plan: &mut Plan, request: &Request) -> Result<llm::ChunkStream, llm::Error> {
        let refused = match plan.provider.stream(request, &plan.credential).await {
            Err(llm::Error::Unauthenticated(words)) if matches!(plan.credential, Credential::OAuth { .. }) => words,
            opened => return opened,
        };
        match self.renew(&plan.model_ref.provider, plan.credential.clone()).await {
            Ok(fresh) => plan.credential = fresh,
            Err(error) => return Err(llm::Error::Unauthenticated(format!("{refused} ({error})"))),
        }
        plan.provider.stream(request, &plan.credential).await
    }

    async fn stream(self: &Arc<Self>, message: &Message, plan: &mut Plan, request: &Request, abort: &CancellationToken) -> Result<Streamed, StreamError> {
        // Stop counts while the request is still being sent or the response has not begun.
        let mut chunks = tokio::select! {
            opened = self.open_response(plan, request) => opened.map_err(StreamError::Provider)?,
            () = abort.cancelled() => return Err(StreamError::Aborted),
        };
        let mut assembler = Assembler::new(&self.store, &self.hub, message);
        let files = self.turns.files_for(&self.store, &plan.session.id);
        let mut early = super::early::Early::new(abort);
        loop {
            // Each call that closed since the last chunk may start now, in the order the model wrote them; none when tools are off.
            for row in assembler.calls[early.seen()..].iter().filter(|_| !request.no_tool_calls) {
                early.consider(self, plan, message, &files, row);
            }
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
        Ok(Streamed { usage: assembler.usage, stop, calls: assembler.calls, early })
    }

    /// Calls run in the model's order. Consecutive reads run together; a write waits for what came before it.
    async fn run_calls(self: &Arc<Self>, plan: &Plan, message: &Message, calls: Vec<PartRow>, early: super::early::Early, abort: &CancellationToken) -> Outcome {
        let files = self.turns.files_for(&self.store, &plan.session.id);
        let scope = CallScope { plan, message, files: &files, abort, wrote: Mutex::default(), tree: Mutex::default(), early: Mutex::new(early) };
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
        let outcome = self.run_reads(&scope, reads).await;
        if outcome == Outcome::Allowed {
            self.check_step(&scope).await;
        }
        outcome
    }

    /// Runs the workspace's checks once over everything the step wrote, and adds what they found to its last writing call.
    async fn check_step(&self, scope: &CallScope<'_>) {
        let StepWrites { mut files, last } = std::mem::take(&mut *scope.wrote.lock().unwrap());
        let Some(row) = last.filter(|_| !scope.plan.config.checks.is_empty() && !files.is_empty()) else { return };
        files.sort();
        files.dedup();
        let asker = super::trust::Asker { message_id: &scope.message.id, call_id: call_id_of(&row), abort: scope.abort };
        let lines = scope.plan.config.project_command_lines("check", &files);
        let allowed = self.project_commands_allowed(scope.plan, asker, lines).await;
        let checks = crate::edit::check::resolve(&scope.plan.config.only_allowed(|line| allowed.contains(line)).1);
        if checks.is_empty() || scope.abort.is_cancelled() {
            return;
        }
        // A whole-workspace check may touch anything, so it holds and captures the whole workspace.
        let whole = checks.iter().any(|check| !check.command.iter().any(|part| part.contains("$FILE")));
        let named = (!whole).then(|| files.clone());
        let Some(_turn) = self.wait_turn(&scope.plan.workspace, named.as_deref(), scope.abort).await else { return };
        let capture = self.capture_before(&scope.plan.workspace, named).await;
        // Without a capture the bytes are compared instead, so a rewrite is still announced.
        let bytes = match &capture {
            Ok(_) => Vec::new(),
            Err(_) => futures_util::future::join_all(files.iter().map(tokio::fs::read)).await.into_iter().map(Result::ok).collect(),
        };
        // A Stop kills the checks, but what a fixer already rewrote is still recorded before the turn is let go.
        let reports = crate::edit::check::run(&files, &scope.plan.workspace, &checks, CHECK_BUDGET, scope.abort).await;
        let recorded = match capture {
            Ok(capture) => self.record_call(&scope.plan.workspace, capture).await.map(|recorded| attribute(recorded, &checks, &files, &scope.plan.workspace)),
            Err(error) => Err(self.unrecorded_rewrites(scope, &files, bytes, &error).await),
        };
        self.report_checks(scope, row, &reports, recorded);
    }

    /// The files checks changed when there was no capture to record them in, said so.
    async fn unrecorded_rewrites(&self, scope: &CallScope<'_>, files: &[PathBuf], before: Vec<Option<Vec<u8>>>, error: &str) -> super::changes::Lost {
        let mut unrecorded = Vec::new();
        for (file, was) in files.iter().zip(before) {
            if tokio::fs::read(file).await.ok() != was {
                unrecorded.push(crate::tool::display(file, &scope.plan.workspace));
            }
        }
        let note = (!unrecorded.is_empty()).then(|| format!("Drift could not record these files before the checks ran ({error}), so undo cannot put back what the checks rewrote."));
        super::changes::Lost { note: note.unwrap_or_default(), put_back: false, unrecorded }
    }

    /// Adds what the checks found, and any file they rewrote, to the step's last writing call: its result, and its change record for undo.
    fn report_checks(&self, scope: &CallScope<'_>, mut row: PartRow, reports: &[crate::edit::check::Report], recorded: Result<super::changes::Recorded, super::changes::Lost>) {
        let workspace = &scope.plan.workspace;
        let (changes, unrecorded, lost) = match recorded {
            Ok(recorded) => (recorded.changes, Vec::new(), None),
            Err(lost) => (Vec::new(), lost.unrecorded, Some(lost.note).filter(|note| !note.is_empty())),
        };
        let changed: Vec<String> = changes.iter().filter(|change| !change.observed).map(|change| change.path.clone()).chain(unrecorded.iter().cloned()).collect();
        let elsewhere: Vec<String> = changes.iter().filter(|change| change.observed).map(|change| change.path.clone()).collect();
        let found = checks_note(reports, workspace, |label, said| self.turns.repeated(&scope.plan.session.id, label, said));
        let notes: Vec<String> = [found, changed_note(&changed), elsewhere_note(&elsewhere), lost].into_iter().flatten().collect();
        let Part::ToolCall { output, metadata, .. } = &mut row.part else { return };
        if !notes.is_empty() {
            *output = Some(format!("{}\n\n{}", output.take().unwrap_or_default(), notes.join("\n\n")));
        }
        let mut meta = metadata.take().unwrap_or_else(|| json!({}));
        meta["checks"] = checks_metadata(reports, workspace);
        if !changed.is_empty() {
            meta["checkChanged"] = json!(changed);
        }
        if !elsewhere.is_empty() {
            meta["checkObserved"] = json!(elsewhere);
        }
        if !unrecorded.is_empty() {
            let mut all: Vec<serde_json::Value> = meta["unrecorded"].as_array().cloned().unwrap_or_default();
            all.extend(unrecorded.iter().map(|path| json!(path)));
            meta["unrecorded"] = json!(all);
        }
        if !changes.is_empty() {
            // After the call's own changes, so undo chains them: the check's rewrite is put back first.
            match meta.get_mut("changes").and_then(serde_json::Value::as_array_mut) {
                Some(list) => list.extend(changes.iter().map(|change| json!(change))),
                None => meta["changes"] = json!(changes),
            }
        }
        *metadata = Some(meta);
        // Unsaved, they are not shown either: what the user sees is what the model will be sent.
        if self.store.save_part(&row).is_ok() {
            self.hub.publish(Event::PartUpdated { part: row });
        }
    }

    async fn run_reads(self: &Arc<Self>, scope: &CallScope<'_>, reads: Vec<PartRow>) -> Outcome {
        let outcomes = futures_util::future::join_all(reads.into_iter().map(|row| self.run_call(scope, row))).await;
        if outcomes.contains(&Outcome::Aborted) { Outcome::Aborted } else { Outcome::Allowed }
    }

    async fn run_call(self: &Arc<Self>, scope: &CallScope<'_>, mut row: PartRow) -> Outcome {
        let Part::ToolCall { call_id, name, input, metadata, .. } = row.part.clone() else { return Outcome::Allowed };
        let command_model = metadata.as_ref().filter(|metadata| metadata["engineCommand"].is_string()).and_then(|metadata| metadata["commandModel"].as_str()).and_then(crate::config::parse_model);
        let mut ctx = Context {
            workspace: scope.plan.workspace.clone(),
            session_id: scope.plan.session.id.clone(),
            agent: scope.plan.session.agent.clone(),
            message_id: scope.message.id.clone(),
            call_id: call_id.clone(),
            files: scope.files.clone(),
            abort: scope.abort.clone(),
            engine: self.clone(),
            config: scope.plan.config.clone(),
            progress: Default::default(),
            command_model,
        };
        // Only what this turn was offered runs, as it was when offered; `Read` for `read` is the same tool.
        let Some((name, tool)) = scope.plan.offer.tool_named(&name) else {
            self.settle(&mut row, ToolStatus::Error, None, format!("`{name}` is not available in this session; use only the tools you were given"), None);
            return Outcome::Allowed;
        };
        if let Part::ToolCall { name: stored, .. } = &mut row.part {
            stored.clone_from(&name);
        }
        if !input.is_object() {
            self.settle(&mut row, ToolStatus::Error, None, unparsed(&input), None);
            return Outcome::Allowed;
        }
        let problems = crate::tool::schema::problems(&tool.spec().input_schema, &input);
        if !problems.is_empty() {
            self.settle(&mut row, ToolStatus::Error, None, format!("The call did not run: {}. Send it again with arguments that fit the tool's schema.", problems.join("; ")), None);
            return Outcome::Allowed;
        }
        // A read-only agent is offered the usual tools; whatever would change something is refused here, before any ask.
        let agent = &scope.plan.session.agent;
        if scope.plan.config.agent(agent).is_some_and(|found| found.read_only) && !tool.stays_read_only(&ctx, &input) {
            let refusal = format!("The {agent} agent only reads, so this call was not run: it would change something. Use read-only commands and tools, or hand the work to a read-only subagent such as explore.");
            self.settle(&mut row, ToolStatus::Error, None, refusal, None);
            return Outcome::Allowed;
        }
        let writes = tool.call_mutates(&input);
        let touches = writes.then(|| tool.touches(&ctx, &input));
        let _turn = match self.lock_call(scope, &mut row, touches.as_ref().and_then(|paths| paths.as_deref())).await {
            Ok(turn) => turn,
            Err(outcome) => return outcome,
        };
        for ask in tool.asks(&ctx, &input) {
            if let Some(refused) = self.permit(scope, &mut row, &call_id, &name, ask).await {
                return refused;
            }
        }
        let capture = match self.before_write(scope, &mut row, touches).await {
            Ok(ready) => ready,
            Err(outcome) => return outcome,
        };
        if let (Some(running), Part::ToolCall { metadata, .. }) = (tool.running_metadata(&ctx, &input), &mut row.part) {
            *metadata = merge(running, metadata.take());
        }
        if let Err(error) = self.start_call(&mut row) {
            self.settle(&mut row, ToolStatus::Error, None, format!("refused to run: could not record the call ({error})"), None);
            return Outcome::Allowed;
        }
        ctx.progress = self.progress_for(&row);
        let started = scope.early.lock().unwrap().take(&call_id);
        let result = match started {
            Some(started) => tokio::select! {
                result = started.finish(scope.files) => result,
                () = scope.abort.cancelled() => Err(crate::tool::ToolError("Aborted.".into())),
            },
            None if tool.stops_itself() => tool.run(&ctx, input).await,
            None => tokio::select! {
                result = tool.run(&ctx, input) => result,
                () = scope.abort.cancelled() => Err(crate::tool::ToolError("Aborted.".into())),
            },
        };
        let (status, title, text, meta) = match result {
            Ok(output) => {
                let status = if tool.failed(&output) { ToolStatus::Error } else { ToolStatus::Done };
                let title = output.title;
                let (text, meta) = if writes { self.after_write(scope, &call_id, output.output, output.metadata).await } else { (output.output, output.metadata) };
                (status, Some(title), text, meta)
            }
            Err(error) => (ToolStatus::Error, None, error.0, serde_json::Value::Null),
        };
        let (mut meta, text) = self.keep_images(&scope.message.id, meta, text).await;
        // Every result, MCP and tools yet to come included, reaches the model within one bound.
        let spill = self.data_dir.join("tool-output").join(&scope.plan.session.id).join(format!("{call_id}.result.log"));
        let (text, spilled) = crate::tool::spool::bound(text, spill);
        if let Some(file) = spilled {
            meta = merge(meta, Some(json!({ "resultFile": file.to_string_lossy() }))).unwrap_or_default();
        }
        // After formatting, and on failure too: a failed or stopped command may still have written.
        let (status, text, changes) = match capture {
            Some(capture) => self.history_of(scope, capture, status, text).await,
            None => (status, text, None),
        };
        // A result this call hands over is acknowledged in the write that saves it, if the call holds its claim.
        let claimant = Claimant::call(&scope.plan.session.id, &call_id);
        let delivers = meta.get("delivers").and_then(serde_json::Value::as_str).filter(|task| self.workers.holds(task, &claimant)).map(str::to_owned);
        self.settle_delivering(&mut row, status, title, text, merge(meta, changes), delivers.as_deref());
        self.release_claims(&claimant);
        if writes {
            scope.wrote.lock().unwrap().note(&row);
        }
        if scope.abort.is_cancelled() { Outcome::Aborted } else { Outcome::Allowed }
    }

    /// Holds named files before preparing their approval preview, until the call is recorded.
    async fn lock_call(&self, scope: &CallScope<'_>, row: &mut PartRow, paths: Option<&[PathBuf]>) -> Result<Option<crate::tool::lock::Held>, Outcome> {
        let Some(paths) = paths else { return Ok(None) };
        if let Some(held) = self.wait_turn(&scope.plan.workspace, Some(paths), scope.abort).await {
            return Ok(Some(held));
        }
        self.settle(row, ToolStatus::Error, None, "Aborted while waiting for another write to these files.".into(), None);
        Err(Outcome::Aborted)
    }

    /// Captures an approved writing call under its already-held file locks.
    async fn before_write(&self, scope: &CallScope<'_>, row: &mut PartRow, touches: Option<Option<Vec<PathBuf>>>) -> Result<Option<super::changes::Capture>, Outcome> {
        let Some(touches) = touches else { return Ok(None) };
        self.snapshots.bind(&scope.plan.session.workspace_id, &scope.plan.workspace);
        // A whole-tree call starts from the tree the step's last one ended on; a file tool's write in between ends the chain.
        let chained = scope.tree.lock().unwrap().take().filter(|_| touches.is_none());
        let captured = match chained {
            Some(tree) => Ok(super::changes::Capture::Tree(tree)),
            None => self.capture_before(&scope.plan.workspace, touches).await,
        };
        match captured {
            Ok(capture) => Ok(Some(capture)),
            Err(error) => {
                // Nothing recorded means no way back, so the write does not happen.
                self.settle(row, ToolStatus::Error, None, format!("refused to write: could not record the files first ({error})"), None);
                Err(Outcome::Allowed)
            }
        }
    }

    /// The turn to write `paths` in `workspace`, or all of it for `None`; `None` back if stopped while waiting.
    async fn wait_turn(&self, workspace: &Path, paths: Option<&[PathBuf]>, abort: &CancellationToken) -> Option<crate::tool::lock::Held> {
        let turn = async {
            match paths {
                Some(paths) => crate::tool::lock::files(paths).await,
                None => crate::tool::lock::workspace(workspace).await,
            }
        };
        tokio::select! {
            held = turn => Some(held),
            () = abort.cancelled() => None,
        }
    }

    /// The call's change record; one that could not be taken is said in its result, and a call whose files were put back fails.
    async fn history_of(&self, scope: &CallScope<'_>, capture: super::changes::Capture, status: ToolStatus, text: String) -> (ToolStatus, String, Option<serde_json::Value>) {
        let plan = scope.plan;
        // `owner` names the workspace whose history holds these blobs, wherever it or the session moves.
        let owner = &plan.session.workspace_id;
        let recorded = self.record_call(&plan.workspace, capture).await;
        // The tree this call ended on is where the step's next whole-tree call starts; anything changed in between is that call's to observe.
        *scope.tree.lock().unwrap() = recorded.as_ref().ok().and_then(|recorded| recorded.tree.clone());
        match recorded {
            Ok(recorded) => {
                let mut changes = json!({ "changes": recorded.changes, "owner": owner, "at": recorded.at });
                if !recorded.unrecorded.is_empty() {
                    changes["unrecorded"] = json!(recorded.unrecorded);
                }
                (status, text, Some(changes))
            }
            Err(lost) => {
                let status = if lost.put_back { ToolStatus::Error } else { status };
                let history = json!({ "changes": [], "owner": owner, "unrecorded": lost.unrecorded, "historyError": lost.note });
                (status, format!("{text}\n\n{}", lost.note), Some(history))
            }
        }
    }

    /// Checks one of a call's asks. `None` lets the call go on; otherwise the call is settled as
    /// refused and the outcome says whether the turn goes on.
    async fn permit(&self, scope: &CallScope<'_>, row: &mut PartRow, call_id: &str, name: &str, ask: crate::tool::Ask) -> Option<Outcome> {
        let request = permission::new_request(&scope.plan.session.id, &scope.message.id, call_id, name, ask);
        match self.permissions.check_under(&self.hub, &scope.plan.config.policy(), &scope.plan.config.agent_policy(&scope.plan.session.agent), request, scope.abort).await {
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

    /// Formats what a mutating call wrote; the model hears when a formatter changed it. Checks wait for the step's end.
    async fn after_write(&self, scope: &CallScope<'_>, call_id: &str, mut text: String, metadata: serde_json::Value) -> (String, serde_json::Value) {
        let asker = super::trust::Asker { message_id: &scope.message.id, call_id, abort: scope.abort };
        let files: Vec<PathBuf> = metadata["files"].as_array().into_iter().flatten().filter_map(|file| file.as_str()).map(PathBuf::from).collect();
        // Formatters are asked about apart from checks, so refusing one never stops the other.
        let config = &scope.plan.config;
        let (written, workspace, formatters) = (files.clone(), scope.plan.workspace.clone(), crate::edit::format::resolve(&config.formatters));
        let programs = tokio::task::spawn_blocking(move || crate::edit::format::project_programs(&written, &workspace, &formatters)).await.unwrap_or_default();
        let lines: Vec<String> = config.project_command_lines("formatter", &files).into_iter().chain(programs.iter().map(|program| program.line.clone())).collect();
        let allowed = self.project_commands_allowed(scope.plan, asker, lines).await;
        let overrides = config.only_allowed(|line| allowed.contains(line)).0;
        let local: Vec<PathBuf> = programs.into_iter().filter(|program| allowed.contains(&program.line)).map(|program| program.path).collect();
        let formatted = self.format_written(scope.plan, &metadata, &overrides, &local).await;
        if !formatted.is_empty() {
            text = format!("{text}\n\n{}", reformatted_note(&formatted));
        }
        // After the formatters, so the servers see the files as they stay; Stop ends the wait.
        let found = tokio::select! {
            found = self.lsp.report(&scope.plan.workspace, &files, &config.lsp) => found,
            () = scope.abort.cancelled() => Vec::new(),
        };
        if let Some(note) = crate::lsp::note(&found, &scope.plan.workspace) {
            text = format!("{text}\n\n{note}");
        }
        let mut metadata = with_formatted(metadata, formatted);
        if !found.is_empty() {
            metadata["diagnostics"] = crate::lsp::metadata(&found, &scope.plan.workspace);
        }
        (text, metadata)
    }

    /// Runs the workspace's formatters over whatever a mutating tool reported writing; names the files they changed.
    async fn format_written(&self, plan: &Plan, metadata: &serde_json::Value, overrides: &std::collections::BTreeMap<String, crate::config::FormatterConfig>, local: &[PathBuf]) -> Vec<String> {
        let formatters = crate::edit::format::resolve(overrides);
        let mut formatted = Vec::new();
        for file in metadata["files"].as_array().into_iter().flatten().filter_map(|f| f.as_str()) {
            let before = tokio::fs::read(file).await.ok();
            let Some(name) = crate::edit::format::format(Path::new(file), &plan.workspace, &formatters, &self.store, local).await else { continue };
            if tokio::fs::read(file).await.ok() != before {
                formatted.push(format!("{name}: {}", crate::tool::display(Path::new(file), &plan.workspace)));
            }
        }
        formatted
    }

    /// Scales the images a call returned within provider limits and moves them to the blob table,
    /// leaving `{mime, hash}` in its metadata; a scaled or dropped image is said in the result.
    async fn keep_images(&self, message_id: &str, mut meta: serde_json::Value, mut text: String) -> (serde_json::Value, String) {
        let returned = crate::tool::image::returned(&meta);
        if returned.is_empty() {
            return (meta, text);
        }
        let mimes: Vec<String> = returned.iter().map(|image| image.mime.clone()).collect();
        let normalized = tokio::task::spawn_blocking(move || returned.into_iter().map(crate::tool::image::normalize).collect::<Vec<_>>())
            .await
            .unwrap_or_else(|_| mimes.iter().map(|_| Err("it could not be prepared".to_string())).collect());
        let mut stored = Vec::new();
        for (mime, result) in mimes.into_iter().zip(normalized) {
            match result.and_then(|(image, note)| self.keep_image(message_id, image).map(|kept| (kept, note))) {
                Ok((kept, note)) => {
                    if let Some(note) = note {
                        text.push_str(&format!("\n\n[an image ({mime}) was {note} to fit the model's limits]"));
                    }
                    stored.push(kept);
                }
                Err(reason) => text.push_str(&format!("\n\n[an image ({mime}) is not shown: {reason}]")),
            }
        }
        meta["images"] = crate::tool::image::stored_metadata(&stored);
        (meta, text)
    }

    fn keep_image(&self, message_id: &str, image: crate::tool::image::Image) -> Result<crate::tool::image::Stored, String> {
        let bytes = image.bytes().ok_or("its data is not valid base64")?;
        let hash = self.store.put_blob(message_id, &bytes).map_err(|_| "it could not be kept".to_string())?;
        Ok(crate::tool::image::Stored { mime: image.mime, hash })
    }

    /// Publishes a running call's part with what it reports merged into its metadata; nothing is stored.
    fn progress_for(self: &Arc<Self>, row: &PartRow) -> crate::tool::Progress {
        let engine = Arc::downgrade(self);
        let running = Mutex::new(row.clone());
        crate::tool::Progress::new(move |patch| {
            let Some(engine) = engine.upgrade() else { return };
            let mut row = running.lock().unwrap();
            if let Part::ToolCall { metadata, .. } = &mut row.part {
                *metadata = merge(metadata.take().unwrap_or_default(), Some(patch));
            }
            engine.hub.publish_transient(Event::PartUpdated { part: row.clone() });
        })
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
    /// After the wrap-up reply a conversation pauses with the reason; a subagent's reply is its result.
    fn end_wrap_up(&self, plan: &Plan, wrapping: Option<WrapUp>) {
        if let Some(wrap_up) = wrapping.filter(|_| plan.session.visibility != Visibility::Hidden) {
            self.pause(plan, wrap_up.pause_reason());
        }
    }

    fn pause(&self, plan: &Plan, reason: String) {
        let Ok(mut message) = self.store.create_reply(&plan.session.id, &plan.model_ref, &plan.session.agent) else { return };
        self.hub.publish(Event::MessageCreated { message: message.clone() });
        message.status = MessageStatus::Paused;
        message.error = Some(reason);
        let _ = self.finish(&mut message);
    }

    /// The calls the session's latest reply made, with their inputs and results.
    fn last_calls(&self, session_id: &str) -> Vec<CallTrace> {
        let Ok(Some(last)) = self.store.last_reply(session_id) else { return Vec::new() };
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
        let Ok(Some(found)) = self.store.with_parts(&message.id) else { return };
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
            let command = metadata.as_ref().and_then(|meta| meta["engineCommand"].as_str()).map(str::to_string);
            let mut value = meta.unwrap_or(serde_json::Value::Null);
            if let Some(object) = value.as_object_mut() { object.remove("engineCommand"); }
            if let Some(command) = command {
                if !value.is_object() { value = json!({}); }
                value["engineCommand"] = json!(command);
            }
            *metadata = (!value.is_null()).then_some(value);
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

/// Why a turn makes one last request with tools off before it stops, as opencode does at its step limit.
#[derive(Clone, Copy, Debug, PartialEq)]
enum WrapUp {
    Steps(u32),
    Repeats(u32),
}

impl WrapUp {
    fn instruction(self) -> String {
        let why = match self {
            WrapUp::Steps(_) => "This is the last step this turn allows".to_string(),
            WrapUp::Repeats(times) => format!("Your last {times} steps made the same calls and got the same results"),
        };
        format!(
            "<system-reminder>\n{why}, so tools are off for this reply. Answer with text only, no tool calls: say that the turn stops here, summarise what you found and did, list what is still undone, and say what should happen next.\n</system-reminder>"
        )
    }

    fn pause_reason(self) -> String {
        match self {
            WrapUp::Steps(steps) => format!("Paused after {steps} steps, this turn's limit. Send a message to carry on."),
            WrapUp::Repeats(times) => format!("Paused: the last {times} steps made the same calls and got the same results. Send a message to carry on or change course."),
        }
    }
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

pub(super) struct Retry {
    /// The provider's words, for the UI.
    message: String,
    /// The wait the provider asked for, if it named one.
    after: Option<Duration>,
}

impl Retry {
    pub(super) fn from(error: &llm::Error) -> Option<Self> {
        match error {
            llm::Error::Api { retryable: true, retry_after, .. } => Some(Self { message: error.to_string(), after: *retry_after }),
            llm::Error::Transport(_) => Some(Self { message: error.to_string(), after: None }),
            _ => None,
        }
    }

    /// Worth waiting for: attempts remain and the provider did not ask for longer than we will wait.
    pub(super) fn message(&self) -> &str {
        &self.message
    }

    pub(super) fn allowed(&self, retries: u32) -> bool {
        retries < MAX_RETRIES && self.after.is_none_or(|after| after <= MAX_REQUESTED_WAIT)
    }

    /// The provider's wait when it named one, else doubling backoff with jitter, capped.
    pub(super) fn delay(&self, attempt: u32) -> Duration {
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
    Switched(Box<Switch>),
    Stopped,
}

/// A model the user moved a waiting retry to, and the variant, when they chose one, to run it at.
struct Switch {
    resolved: Resolved,
    variant: Option<Option<String>>,
}

/// What a turn offers the model: each tool's spec and the tool itself, held until the turn ends.
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

    /// The tool by its exact name, else the one offered tool whose name differs only in case.
    fn tool_named(&self, name: &str) -> Option<(String, Arc<dyn crate::tool::Tool>)> {
        if let Some(tool) = self.tool(name) {
            return Some((name.to_string(), tool));
        }
        let mut close = self.tools.iter().filter(|(spec, _)| spec.name.eq_ignore_ascii_case(name));
        match (close.next(), close.next()) {
            (Some((spec, tool)), None) => Some((spec.name.clone(), tool.clone())),
            _ => None,
        }
    }
}

/// What the model hears for arguments that never parsed: the parser's own complaint, so it can fix the call.
fn unparsed(input: &serde_json::Value) -> String {
    let raw = input.as_str().unwrap_or_default();
    let reason = serde_json::from_str::<serde_json::Value>(raw).err().map_or_else(|| "they are not a JSON object".to_string(), |error| error.to_string());
    let shown: String = raw.chars().take(200).collect();
    format!("The call did not run: its arguments were not valid JSON ({reason}). They began: {shown}\nSend it again with one JSON object that fits the tool's schema.")
}

struct CallScope<'a> {
    plan: &'a Plan,
    message: &'a Message,
    files: &'a Arc<SessionFiles>,
    abort: &'a CancellationToken,
    wrote: Mutex<StepWrites>,
    /// The tree the step's last whole-tree call ended on, so the next one takes one capture, not two.
    tree: Mutex<Option<super::snapshot::Tree>>,
    /// Calls the reply started while it streamed, each taken by its own call when it runs.
    early: Mutex<super::early::Early>,
}

/// What a step's calls wrote, for the checks that run once the step's calls are done.
#[derive(Default)]
struct StepWrites {
    files: Vec<std::path::PathBuf>,
    /// The step's last writing call, whose result carries what the checks found.
    last: Option<PartRow>,
}

struct Streamed {
    usage: Usage,
    stop: StopReason,
    calls: Vec<PartRow>,
    /// Calls already started while the reply streamed.
    early: super::early::Early,
}

enum StreamError {
    Aborted,
    Provider(llm::Error),
}

fn call_mutates(plan: &Plan, row: &PartRow) -> bool {
    match &row.part {
        Part::ToolCall { name, input, .. } => plan.offer.tool_named(name).is_some_and(|(_, tool)| tool.call_mutates(input)),
        _ => false,
    }
}

/// Whether a prompt may switch its session to `agent`: only a usable primary agent of the workspace runs a conversation.
fn pickable(config: &Config, agent: &str) -> Result<(), TurnError> {
    match config.agent(agent) {
        Some(found) if found.kind.runs_conversations() => found.usable().map(|_| ()).map_err(TurnError::Config),
        _ => Err(TurnError::UnknownAgent),
    }
}

/// Identity of a prompt for replay checks: the same id must carry the same parts and model.
pub(super) fn payload_hash(prompt: &Prompt) -> String {
    use sha2::Digest;
    // Wrapped, so a variant left unnamed and one cleared to the model's default hash apart.
    let variant = prompt.variant.as_ref().map(|chosen| serde_json::json!({ "chosen": chosen }));
    let body = serde_json::json!({ "parts": prompt.parts, "model": prompt.model, "variant": variant, "agent": prompt.agent });
    sha2::Sha256::digest(body.to_string().as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// The output limit and thinking budget for one request, valid together: the output never exceeds the
/// model's own limit, a thinking budget may raise it past our usual cap but always leaves
/// [`MIN_ANSWER_TOKENS`] for the answer, and a budget that cannot fit is reduced, or dropped when even
/// the provider's minimum would not fit.
fn budgets(model: &Model, requested: Option<Reasoning>) -> (u32, Option<Reasoning>) {
    // An unknown output limit asks for a quarter of a known window: a server may refuse a request whose reply could not fit.
    let unknown = u32::try_from(model.reply_room()).unwrap_or(MAX_OUTPUT_TOKENS).max(MIN_ANSWER_TOKENS);
    // Never more than half a known window, however large the listed output limit: the prompt needs the rest.
    let half = u32::try_from(model.limit.context / 2).ok().filter(|half| *half > 0).unwrap_or(u32::MAX).max(MIN_ANSWER_TOKENS);
    let model_limit = u32::try_from(model.limit.output).ok().filter(|limit| *limit > 0).map_or(unknown, |limit| limit.min(half));
    let wanted = match requested.filter(|_| model.reasoning) {
        Some(Reasoning::Budget { tokens }) => tokens,
        effort => return (model_limit.min(MAX_OUTPUT_TOKENS), effort),
    };
    let max_tokens = model_limit.min(MAX_OUTPUT_TOKENS.max(wanted.saturating_add(MIN_ANSWER_TOKENS)));
    let room = max_tokens.saturating_sub(MIN_ANSWER_TOKENS);
    let thinking = (room >= MIN_THINKING_TOKENS).then(|| Reasoning::Budget { tokens: wanted.clamp(MIN_THINKING_TOKENS, room) });
    (max_tokens, thinking)
}

/// Prices are per million tokens.
/// What a request cost, at the prices for its prompt's length (all of its input, cached or not).
pub(super) fn cost(model: &Model, usage: Usage) -> f64 {
    let (input, output, cache_read, cache_write) = model.cost.at(usage.input + usage.cache_read + usage.cache_write);
    (usage.input as f64 * input + usage.output as f64 * output + usage.cache_read as f64 * cache_read + usage.cache_write as f64 * cache_write) / 1_000_000.0
}

/// What the model is told about a call the user refused.
fn denial(feedback: Option<&str>, stop: bool) -> String {
    let refused = if stop { "The user denied permission for this call and stopped the turn." } else { "The user denied permission for this call." };
    match feedback {
        Some(said) => format!("{refused} They said: {said}"),
        None => refused.to_string(),
    }
}

/// What the model needs to hear after a formatter rewrote its change: the file is not what it wrote.
fn reformatted_note(formatted: &[String]) -> String {
    format!("A formatter then changed the result ({}). The file no longer matches what you wrote; read it again before editing those lines.", formatted.join(", "))
}

/// A change to a file none of the checks runs over cannot be theirs: someone else made it while they ran, so undo leaves it alone.
/// A check's own change is to a file the step wrote and a check covers; any other change seen while they ran could be anyone's, so undo leaves it alone.
fn attribute(mut recorded: super::changes::Recorded, checks: &[crate::edit::check::Check], written: &[PathBuf], workspace: &Path) -> super::changes::Recorded {
    let written: Vec<String> = written.iter().map(|file| super::changes::relative(workspace, file)).collect();
    for change in &mut recorded.changes {
        change.observed = !(written.contains(&change.path) && crate::edit::check::covers(checks, Path::new(&change.path)));
    }
    recorded
}

fn call_id_of(row: &PartRow) -> &str {
    match &row.part {
        Part::ToolCall { call_id, .. } => call_id,
        _ => "",
    }
}

/// The most time one step's checks may take together.
const CHECK_BUDGET: std::time::Duration = std::time::Duration::from_secs(90);

impl StepWrites {
    /// Adds what a settled writing call reports writing.
    fn note(&mut self, row: &PartRow) {
        let Part::ToolCall { metadata: Some(meta), .. } = &row.part else { return };
        let files: Vec<std::path::PathBuf> = meta["files"].as_array().into_iter().flatten().filter_map(|file| file.as_str()).map(Into::into).collect();
        if files.is_empty() {
            return;
        }
        self.files.extend(files);
        self.last = Some(row.clone());
    }
}

/// What the model hears after checks changed files it wrote, as after a formatter.
/// What the model hears of other files that changed while the checks ran: a whole-workspace fixer's work, or anyone's.
fn elsewhere_note(changed: &[String]) -> Option<String> {
    (!changed.is_empty()).then(|| {
        format!("While the checks ran, files this step did not write changed too ({}), by a whole-workspace check or by someone else. Read them again before relying on what you knew of them; undo leaves them as they are.", changed.join(", "))
    })
}

fn changed_note(changed: &[String]) -> Option<String> {
    (!changed.is_empty()).then(|| format!("A check then changed {}. Those files no longer match what you wrote; read them again before editing those lines.", changed.join(", ")))
}

fn check_label(report: &crate::edit::check::Report, workspace: &Path) -> String {
    match &report.file {
        Some(file) => format!("{}: {}", report.name, crate::tool::display(file, workspace)),
        None => report.name.clone(),
    }
}

/// What the model hears from checks that found problems; output `repeated` says it was given before is named, not sent again.
fn checks_note(reports: &[crate::edit::check::Report], workspace: &Path, repeated: impl Fn(&str, Option<&str>) -> bool) -> Option<String> {
    use crate::edit::check::Verdict;
    let mut problems = Vec::new();
    for report in reports {
        let label = check_label(report, workspace);
        match &report.verdict {
            Verdict::Problems(said) if repeated(&label, Some(said)) => problems.push(format!("[{label}] the same problems as reported before")),
            Verdict::Problems(said) => problems.push(format!("[{label}]\n{said}")),
            Verdict::Passed => {
                repeated(&label, None);
            }
            Verdict::Unavailable(_) => {}
        }
    }
    (!problems.is_empty()).then(|| format!("Checks reported problems after this step's changes. Fix the ones your changes caused:\n\n{}", problems.join("\n\n")))
}

fn checks_metadata(reports: &[crate::edit::check::Report], workspace: &Path) -> serde_json::Value {
    use crate::edit::check::Verdict;
    let entry = |report: &crate::edit::check::Report| match &report.verdict {
        Verdict::Passed => json!({ "check": check_label(report, workspace), "status": "passed" }),
        Verdict::Problems(said) => json!({ "check": check_label(report, workspace), "status": "problems", "output": said }),
        Verdict::Unavailable(why) => json!({ "check": check_label(report, workspace), "status": "unavailable", "output": why }),
    };
    json!(reports.iter().map(entry).collect::<Vec<_>>())
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
