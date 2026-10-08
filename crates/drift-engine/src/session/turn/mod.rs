//! One prompt, one turn: stream the model, run what it calls, repeat until it stops.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;

use super::assemble::Assembler;
use super::attach::Attach;
use super::compaction::Trigger;
use super::oneshot::Resolved;
use super::tasks::Claimant;
use super::types::{CheckStatus, ToolCheck, ToolMetadata};
use super::{
    branch, changes, command, compaction, convert, drive, early, oneshot, prompt, snapshot, tasks, trust, types,
};
use crate::Engine;
use crate::config::Config;
use crate::event::{Event, SessionStatus};
use crate::id;
use crate::llm::catalog::{self, Model, Reasoning, Variant};
use crate::llm::{self, Credential, Provider, Request, StopReason};
use crate::permission::{self, Outcome};
use crate::session::types::{
    Message, MessageStatus, MessageWithParts, ModelRef, Part, PartRow, Role, Session, ToolStatus, Usage, Visibility,
};
use crate::store::{Admit, Admitted, Handover, Pick};
use crate::tool::{Context, SessionFiles};

mod admission;
mod approval;
mod calls;
mod checks;
mod history;
mod hooks;
mod images;
mod limits;
mod planning;
mod receipts;
mod request;
mod retry;
mod settle;
mod state;
mod steps;

use hooks::{AfterTool, BeforeTool};
#[cfg(test)]
use limits::waits;
use planning::pickable;
pub(super) use receipts::payload_hash;
pub(super) use request::cost;
#[cfg(test)]
use request::{MAX_OUTPUT_TOKENS, MIN_ANSWER_TOKENS, MIN_THINKING_TOKENS, budgets};
pub(super) use retry::Retry;
pub(super) use settle::Settlement;

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
/// A finished reply's `error` when it stopped at the output limit rather than ending on its own.
pub const OUTPUT_LIMIT_ENDING: &str = "The reply stopped at the output limit";
/// A finished reply's `error` when the provider's safety filter ended it, so it never ends in silence.
pub const REFUSED_ENDING: &str = "The provider's safety filter ended the reply.";

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
    /// A plugin refused the prompt; says which and why.
    Refused(String),
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

#[derive(Debug, thiserror::Error)]
enum FollowError {
    #[error("{0}")]
    Agent(#[from] TurnError),
    #[error("Could not switch to {model}: {error}. Send a message to carry on.")]
    Switch { model: String, error: TurnError },
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
    pub(super) trust: trust::Answers,
    refreshing: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Automatic compactions that failed in a row, per session; enough of them turn it off for that session.
    pub(super) compaction_failures: Mutex<HashMap<String, u32>>,
    /// How each subagent's last turn ended, until the task waiting on it takes the answer.
    ended: Mutex<HashMap<String, TurnEnd>>,
    /// Turns waiting out a retry backoff, each ready to take a model the user switches to.
    retry_waits: Mutex<HashMap<String, tokio::sync::oneshot::Sender<Switch>>>,
    /// Running turns that still accept prompts, with the model and agent used to validate each prompt.
    /// Admission and turn completion both hold this lock, so no prompt lands after the turn stops looking.
    /// Command turns retain the session's defaults for steered prompts.
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

/// How a turn ended, from the loop's own view rather than whatever message happens to be last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnEnd {
    Replied,
    Failed,
    Stopped,
}

/// The fixed admission plan for a queued worker, then the model and agent choices followed at each step.
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
    bootstrap: Vec<command::Bootstrap>,
    /// Its agent and model are a command's, for this turn only: it does not follow the session's.
    turn_only: bool,
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
    pub(super) bootstrap: &'a [command::Bootstrap],
    /// The prompt's agent and model run this turn only; the session keeps its own, so it never steers.
    pub(super) turn_only: bool,
    pub(super) config: Option<&'a Config>,
}

/// A prepared prompt and its Stop fence and delivery bookkeeping.
pub(super) struct FencedPrompt<'a> {
    pub pick: Pick<'a>,
    pub parts: Vec<Part>,
    pub submission: Option<(&'a str, &'a str)>,
    pub abort: Option<&'a CancellationToken>,
    pub delivery: Option<&'a str>,
}

struct CallBatch {
    rows: Vec<PartRow>,
    early: early::Early,
}

struct CallAsk<'a> {
    call_id: &'a str,
    name: &'a str,
    ask: crate::tool::Ask,
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

/// Steps in a row whose calls and results were all the same. Different results are progress, so a
/// poll whose answer changes never counts; one that waits on purpose gets the larger `polls` allowance.
#[derive(Default)]
struct Repeats {
    last: Vec<CallTrace>,
    count: u32,
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

struct CallScope<'a> {
    plan: &'a Plan,
    message: &'a Message,
    files: &'a Arc<SessionFiles>,
    abort: &'a CancellationToken,
    wrote: Mutex<StepWrites>,
    /// The tree the step's last whole-tree call ended on, so the next one takes one capture, not two.
    tree: Mutex<Option<snapshot::Tree>>,
    /// Calls the reply started while it streamed, each taken by its own call when it runs.
    early: Mutex<early::Early>,
}

/// What a step's calls wrote, for the checks that run once the step's calls are done.
#[derive(Default)]
struct StepWrites {
    files: Vec<PathBuf>,
    /// The step's last writing call, whose result carries what the checks found.
    last: Option<PartRow>,
}

struct Streamed {
    usage: Usage,
    stop: StopReason,
    calls: Vec<PartRow>,
    /// Calls already started while the reply streamed.
    early: early::Early,
}

enum StreamError {
    Aborted,
    Provider(llm::Error),
}

/// A field that is present, even as null, is `Some`; only an absent one stays `None`.
pub(crate) fn present<'de, D: serde::Deserializer<'de>>(value: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(value).map(Some)
}

fn call_id_of(row: &PartRow) -> &str {
    match &row.part {
        Part::ToolCall { call_id, .. } => call_id,
        _ => "",
    }
}

impl std::fmt::Display for TurnError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSession => write!(formatter, "session not found"),
            Self::NoWorkspace => write!(formatter, "workspace not found"),
            Self::Busy => write!(formatter, "session is already running a turn"),
            Self::SubmissionReused => write!(
                formatter,
                "submission id was already used with a different prompt or session"
            ),
            Self::NoModel => write!(formatter, "no model selected"),
            Self::UnknownModel => write!(formatter, "model is not in the catalog"),
            Self::NoCredentials => write!(formatter, "provider has no credentials"),
            Self::Config(problem) => write!(formatter, "{problem}"),
            Self::Refused(reason) => write!(formatter, "{reason}"),
            Self::SignInExpired(reason) => write!(
                formatter,
                "the sign-in has expired and could not be renewed; \
                 sign in again under Settings > Providers ({reason})"
            ),
            Self::NotRetrying => write!(formatter, "the session is not waiting to retry"),
            Self::Reverted => write!(formatter, "the session is undone; send a prompt or redo first"),
            Self::Attachment(message) => write!(formatter, "{message}"),
            Self::Store(message) => write!(formatter, "store: {message}"),
            Self::Stopped => write!(formatter, "stopped before it started"),
            Self::Moved => write!(formatter, "the session moved to another workspace while it waited"),
            Self::UnknownAgent => write!(
                formatter,
                "no agent by that name can run a conversation in this workspace"
            ),
            Self::Replayed(message) => write!(formatter, "already admitted as {message}"),
        }
    }
}

impl From<rusqlite::Error> for TurnError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.to_string())
    }
}

impl std::error::Error for TurnError {}

impl Engine {
    /// Steps until the model is done, then again for any prompt steered in meanwhile.
    async fn run(self: &Arc<Self>, mut plan: Plan, abort: CancellationToken) {
        self.title_untitled(&plan.session);
        let started = self.store.newest_prompt(&plan.session.id).ok().flatten();
        if !self.run_bootstrap(&mut plan, &abort).await {
            self.turns.steering.lock().unwrap().remove(&plan.session.id);
            self.record_end(&plan.session, &abort);
            return;
        }

        let mut continued = 0;
        loop {
            let answered = self.run_steps(&mut plan, &abort, started.as_deref()).await;
            if self.hook_turn_end(&plan, &mut continued, &abort).await {
                continue;
            }
            if abort.is_cancelled() || !self.carries_on(&plan, answered.as_deref(), &abort) {
                break;
            }
        }

        self.turns.steering.lock().unwrap().remove(&plan.session.id);
        self.record_end(&plan.session, &abort);
    }

    /// Runs `job` in a session already claimed: reports it running, then idle and releases it when done.
    /// The job runs as its own task, so even one that panics releases the session.
    pub(super) fn spawn_job(
        self: &Arc<Self>,
        session_id: &str,
        job: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        self.hub.publish(Event::SessionStatusChanged {
            session_id: session_id.into(),
            status: SessionStatus::Running,
        });
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

            // A panicking call may still hold a delivery claim.
            engine.release_claims_of(&id);
            engine.hub.publish(Event::SessionStatusChanged {
                session_id: id.clone(),
                status: SessionStatus::Idle,
            });
            engine.turns.finished.notify_waiters();
            engine.retry_deliveries(Some(&id));
        });
    }

    /// Stops the session's turn and workers under the admission fence, including a worker owning this transcript.
    pub fn abort(&self, session_id: &str) -> bool {
        let mut owners = self.workers.fence();
        let workers = self.stop_workers(&mut owners, session_id);
        let turn = self.turns.cancel(session_id);
        let worker = self
            .store
            .task_for_session(session_id)
            .ok()
            .flatten()
            .is_some_and(|task| !task.state.is_terminal() && self.workers.cancel(&task.id));

        turn || workers || worker
    }

    /// Checks for an unanswered steered prompt or an orchestrator nudge under the steering lock.
    /// If neither exists, it stops accepting prompts under that same lock.
    /// No prompt can land after the turn has decided to finish.
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

    /// Sends an orchestrator nudge when its reply still requires work; false ends the turn.
    fn nudge(&self, plan: &Plan, abort: &CancellationToken) -> bool {
        let session_id = plan.session.id.as_str();
        if plan.turn_only || plan.session.agent != drive::AGENT {
            return false;
        }
        let Ok(Some(reply)) = self.store.last_reply(session_id) else {
            return false;
        };
        let rounds = self.store.nudges_since_prompt(session_id).unwrap_or(usize::MAX);
        let Some(text) = drive::next(&plan.session, &reply, rounds) else {
            return false;
        };

        let prompt = FencedPrompt {
            pick: Pick {
                model: &plan.model_ref,
                variant: None,
                agent: None,
                sticky: true,
            },
            parts: vec![Part::Nudge { text: text.into() }],
            submission: None,
            abort: Some(abort),
            delivery: None,
        };
        match self.admit_fenced(session_id, prompt) {
            Ok(admitted) => {
                self.announce(session_id, admitted);
                true
            }
            Err(_) => false,
        }
    }

    /// Records how a subagent turn ended; a Stop wins even if a reply finished first.
    /// Compaction summaries are bookkeeping, not a subagent's final answer.
    fn record_end(&self, session: &Session, abort: &CancellationToken) {
        if session.visibility != Visibility::Hidden {
            return;
        }

        let end = if abort.is_cancelled() {
            TurnEnd::Stopped
        } else {
            match self.store.last_reply(&session.id).ok().flatten() {
                Some(last)
                    if last.info.status == MessageStatus::Done && !last.info.summary && last.info.error.is_none() =>
                {
                    TurnEnd::Replied
                }
                Some(last) if last.info.status == MessageStatus::Aborted => TurnEnd::Stopped,
                _ => TurnEnd::Failed,
            }
        };
        self.turns.ended.lock().unwrap().insert(session.id.clone(), end);
    }
}

#[cfg(test)]
pub(crate) mod tests;
