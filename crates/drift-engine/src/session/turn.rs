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
use super::compaction::{self, Trigger};
use super::prompt;
use crate::config::Config;
use crate::event::{Event, SessionStatus};
use crate::id;
use crate::llm::catalog::Model;
use crate::llm::{self, Credential, Provider, Request, StopReason};
use crate::permission::{self, Outcome};
use crate::session::types::{Message, MessageStatus, MessageWithParts, ModelRef, Part, PartRow, Role, Session, ToolStatus, Usage, Visibility};
use crate::tool::{Context, SessionFiles};
use crate::Engine;

const MAX_ATTEMPTS: u32 = 3;
/// Output cap when the model allows more; keeps a runaway response from burning the budget.
const MAX_OUTPUT_TOKENS: u32 = 32_000;

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
    Store(String),
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
            Self::Store(message) => write!(f, "store: {message}"),
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
    /// Tests swap the wire adapter for a scripted one.
    pub provider_override: Mutex<Option<Provider>>,
}

impl Turns {
    /// Marks the session busy under `abort`; `false` if a turn or job already holds it.
    pub(super) fn claim(&self, session_id: &str, abort: &CancellationToken) -> bool {
        let mut active = self.active.lock().unwrap();
        if active.contains_key(session_id) {
            return false;
        }
        active.insert(session_id.into(), abort.clone());
        true
    }

    fn files_for(&self, session_id: &str) -> Arc<SessionFiles> {
        self.files.lock().unwrap().entry(session_id.into()).or_default().clone()
    }

    fn refresh_lock(&self, provider: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.refreshing.lock().unwrap().entry(provider.into()).or_default().clone()
    }

    pub fn is_running(&self, session_id: &str) -> bool {
        self.active.lock().unwrap().contains_key(session_id)
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

pub(super) struct Plan {
    pub(super) session: Session,
    workspace: PathBuf,
    pub(super) config: Config,
    pub(super) model_ref: ModelRef,
    pub(super) model: Model,
    pub(super) provider: Provider,
    pub(super) credential: Credential,
    thinking_budget: Option<u32>,
}

impl Engine {
    /// Records the prompt and starts the turn in the background; the receipt is what was recorded.
    pub async fn submit(self: &Arc<Self>, session_id: &str, prompt: Prompt) -> Result<Receipt, TurnError> {
        self.submit_under(session_id, prompt, None).await
    }

    /// A turn whose abort token descends from parent, so aborting the parent aborts it however the wait ends.
    pub async fn submit_under(self: &Arc<Self>, session_id: &str, prompt: Prompt, parent: Option<&CancellationToken>) -> Result<Receipt, TurnError> {
        let payload_hash = payload_hash(&prompt);
        if let Some(id) = prompt.submission_id.as_deref() {
            if let Some(receipt) = self.replayed_receipt(id, session_id, &payload_hash)? {
                return Ok(receipt);
            }
        }
        let plan = self.plan(session_id, &prompt).await?;
        let abort = parent.map_or_else(CancellationToken::new, CancellationToken::child_token);
        if !self.turns.claim(session_id, &abort) {
            return Err(TurnError::Busy);
        }
        let submission = prompt.submission_id.as_deref().map(|id| (id, payload_hash.as_str()));
        let admitted = self.store.admit_prompt(session_id, &plan.model_ref, prompt.parts, submission);
        let (message, parts, session) = match admitted {
            Ok(admitted) => admitted,
            Err(error) => {
                self.turns.active.lock().unwrap().remove(session_id);
                return Err(error.into());
            }
        };
        self.hub.publish(Event::MessageCreated { message: message.clone() });
        for row in parts {
            self.hub.publish(Event::PartCreated { part: row });
        }
        self.hub.publish(Event::SessionUpdated { session: session.clone() });
        let engine = self.clone();
        self.spawn_job(session_id, async move { engine.run(plan, abort).await });
        Ok(Receipt { session, message })
    }

    /// Runs `job` in a session already claimed: reports it running, then idle and releases it when done.
    pub(super) fn spawn_job(self: &Arc<Self>, session_id: &str, job: impl std::future::Future<Output = ()> + Send + 'static) {
        self.hub.publish(Event::SessionStatusChanged { session_id: session_id.into(), status: SessionStatus::Running });
        let engine = self.clone();
        let id = session_id.to_string();
        tokio::spawn(async move {
            job.await;
            engine.turns.active.lock().unwrap().remove(&id);
            engine.hub.publish(Event::SessionStatusChanged { session_id: id, status: SessionStatus::Idle });
            engine.turns.finished.notify_waiters();
        });
    }

    /// A known submission id replays its receipt from storage, so a retry after a restart is still one prompt.
    fn replayed_receipt(&self, id: &str, session_id: &str, payload_hash: &str) -> Result<Option<Receipt>, TurnError> {
        let Some(found) = self.store.submission(id)? else { return Ok(None) };
        if found.session_id != session_id || found.payload_hash != payload_hash {
            return Err(TurnError::SubmissionReused);
        }
        let session = self.store.session(session_id)?.ok_or(TurnError::NoSession)?;
        let message = self.store.message(&found.message_id)?.ok_or(TurnError::NoSession)?;
        Ok(Some(Receipt { session, message }))
    }

    pub fn abort(&self, session_id: &str) -> bool {
        match self.turns.active.lock().unwrap().get(session_id) {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    pub(super) async fn plan(&self, session_id: &str, prompt: &Prompt) -> Result<Plan, TurnError> {
        let session = self.store.session(session_id)?.ok_or(TurnError::NoSession)?;
        let workspace = self.store.workspace(&session.workspace_id)?.ok_or(TurnError::NoWorkspace)?;
        let workspace_path = crate::tool::canonical(Path::new(&workspace.path));
        let config = self.workspace_config(&workspace_path);
        let agent_model = config.agent(&session.agent).and_then(|a| a.model.clone());
        let model_ref = prompt.model.clone().or_else(|| session.model.clone()).or(agent_model).or_else(|| config.model.clone()).ok_or(TurnError::NoModel)?;
        let resolved = self.resolve(&model_ref).await?;
        Ok(Plan {
            session,
            workspace: workspace_path,
            config,
            model_ref: resolved.model_ref,
            model: resolved.model,
            provider: resolved.provider,
            credential: resolved.credential,
            thinking_budget: prompt.thinking_budget,
        })
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

    async fn run(self: &Arc<Self>, plan: Plan, abort: CancellationToken) {
        self.title_untitled(&plan.session);
        let agent = plan.config.agent(&plan.session.agent).cloned();
        let allowed = agent.as_ref().map(|a| a.tools.clone()).unwrap_or_default();
        let subagent = plan.session.visibility == Visibility::Hidden;
        let tools: Vec<_> = self.tools.specs(plan.model.profile).into_iter().filter(|spec| allowed.is_empty() || allowed.contains(&spec.name)).filter(|spec| !(subagent && crate::tool::task::DELEGATION.contains(&spec.name.as_str()))).collect();
        // What was offered is what may run; a call to any other tool is refused before permission or snapshot.
        let offered: std::collections::HashSet<String> = tools.iter().map(|t| t.name.clone()).collect();
        let system = prompt::system(&plan.workspace, &plan.config, agent.as_ref(), offered.contains("task"));
        let mut attempts = 0;
        let mut recovered = false;
        loop {
            let Some(transcript) = self.transcript_for_step(&plan, &abort).await else { break };
            let request = Request {
                model: plan.model_ref.model.clone(),
                system: system.clone(),
                messages: compaction::request_messages(&transcript, &plan.model_ref),
                tools: tools.clone(),
                max_tokens: max_tokens(&plan.model, plan.thinking_budget),
                thinking_budget: plan.thinking_budget.filter(|_| plan.model.reasoning),
                temperature: None,
            };
            let Ok(message) = self.store.create_message(&plan.session.id, Role::Assistant, Some(&plan.model_ref)) else { break };
            self.hub.publish(Event::MessageCreated { message: message.clone() });
            match self.step(&plan, message, &request, &offered, &abort).await {
                Step::Done => break,
                Step::Continue => attempts = 0,
                Step::Retry if attempts + 1 < MAX_ATTEMPTS => {
                    attempts += 1;
                    tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempts))).await;
                }
                Step::Retry => break,
                // A request too long for the model is compacted once and retried.
                Step::Overflow if !recovered => {
                    recovered = true;
                    if self.compact(&plan.session.id, Trigger::Overflow, &abort).await.is_err() {
                        break;
                    }
                }
                Step::Overflow => break,
            }
        }
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
    async fn step(self: &Arc<Self>, plan: &Plan, mut message: Message, request: &Request, offered: &std::collections::HashSet<String>, abort: &CancellationToken) -> Step {
        let streamed = match self.stream(&message, plan, request, abort).await {
            Ok(streamed) => streamed,
            Err(StreamError::Aborted) => {
                message.status = MessageStatus::Aborted;
                let _ = self.finish(&mut message);
                return Step::Done;
            }
            Err(StreamError::Provider(error)) => {
                message.status = MessageStatus::Error;
                message.error = Some(error.to_string());
                let _ = self.finish(&mut message);
                if error.is_context_overflow() {
                    return Step::Overflow;
                }
                return if matches!(error, llm::Error::Api { retryable: true, .. } | llm::Error::Transport(_)) { Step::Retry } else { Step::Done };
            }
        };
        message.usage = streamed.usage;
        message.cost = if matches!(plan.credential, Credential::OAuth { .. }) { 0.0 } else { cost(&plan.model, streamed.usage) };
        message.status = MessageStatus::Done;
        if self.finish(&mut message).is_err() || streamed.calls.is_empty() {
            return Step::Done;
        }
        // The model already said it ran out of room; running its calls would only invite a continuation it cannot make.
        if streamed.stop == StopReason::MaxTokens {
            return Step::Done;
        }
        match self.run_calls(plan, &message, streamed.calls, offered, abort).await {
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
        let mut chunks = plan.provider.stream(request, &plan.credential).await.map_err(StreamError::Provider)?;
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
    async fn run_calls(self: &Arc<Self>, plan: &Plan, message: &Message, calls: Vec<PartRow>, offered: &std::collections::HashSet<String>, abort: &CancellationToken) -> Outcome {
        let files = self.turns.files_for(&plan.session.id);
        let snapshot = tokio::sync::OnceCell::new();
        let scope = CallScope { plan, message, files: &files, snapshot: &snapshot, offered, abort };
        let mut reads: Vec<PartRow> = Vec::new();
        for row in calls {
            if !self.call_mutates(&row) {
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
    fn call_mutates(&self, row: &PartRow) -> bool {
        match &row.part {
            Part::ToolCall { name, .. } => self.tools.get(name).is_some_and(|tool| tool.mutates()),
            _ => false,
        }
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
        };
        if !scope.offered.contains(&name) {
            self.settle(&mut row, ToolStatus::Error, None, format!("`{name}` is not available in this session; use only the tools you were given"), None);
            return Outcome::Allowed;
        }
        let Some(tool) = self.tools.get(&name) else {
            self.settle(&mut row, ToolStatus::Error, None, format!("unknown tool `{name}`"), None);
            return Outcome::Allowed;
        };
        if !input.is_object() {
            self.settle(&mut row, ToolStatus::Error, None, "call arguments were not valid JSON; the call did not run".into(), None);
            return Outcome::Allowed;
        }
        if let Some(ask) = tool.ask(&ctx, &input) {
            let request = permission::new_request(&scope.plan.session.id, &scope.message.id, &call_id, &name, ask);
            match self.permissions.check(&self.hub, &scope.plan.config.policy(), request, scope.abort).await {
                Outcome::Allowed => {}
                Outcome::Denied => {
                    self.settle(&mut row, ToolStatus::Denied, None, "Permission denied by the user.".into(), None);
                    return Outcome::Allowed;
                }
                Outcome::Aborted => {
                    self.settle(&mut row, ToolStatus::Error, None, "Aborted while waiting for permission.".into(), None);
                    return Outcome::Aborted;
                }
            }
        }
        let snapshot_meta = if tool.mutates() {
            let taken = scope.snapshot.get_or_init(|| async { self.snapshots.take(&scope.plan.workspace).await.map_err(|e| e.to_string()) }).await;
            match taken {
                Ok(tree) => Some(json!({ "snapshot": tree })),
                Err(error) => {
                    // No snapshot means no way back, so the write does not happen.
                    self.settle(&mut row, ToolStatus::Error, None, format!("refused to write: could not snapshot the workspace first ({error})"), None);
                    return Outcome::Allowed;
                }
            }
        } else {
            None
        };
        if let Err(error) = self.start_call(&mut row) {
            self.settle(&mut row, ToolStatus::Error, None, format!("refused to run: could not record the call ({error})"), None);
            return Outcome::Allowed;
        }
        let result = tokio::select! {
            result = tool.run(&ctx, input) => result,
            () = scope.abort.cancelled() => Err(crate::tool::ToolError("Aborted.".into())),
        };
        match result {
            Ok(output) => {
                let formatted = if tool.mutates() { self.format_written(scope.plan, &output.metadata).await } else { Vec::new() };
                let meta = merge(output.metadata, snapshot_meta).map(|m| with_formatted(m, formatted));
                self.settle(&mut row, ToolStatus::Done, Some(output.title), output.output, meta)
            }
            Err(error) => self.settle(&mut row, ToolStatus::Error, None, error.0, snapshot_meta),
        }
        if scope.abort.is_cancelled() { Outcome::Aborted } else { Outcome::Allowed }
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

    /// Writes the outcome. If that write fails, what is published is the failure, never a success the store lacks.
    fn settle(&self, row: &mut PartRow, new_status: ToolStatus, new_title: Option<String>, text: String, meta: Option<serde_json::Value>) {
        if let Part::ToolCall { status, title, output, metadata, finished_at, .. } = &mut row.part {
            *status = new_status;
            *title = new_title.or(title.take());
            *output = Some(text);
            *metadata = meta;
            *finished_at = Some(id::now_ms());
        }
        if let Err(error) = self.store.save_part(row) {
            if let Part::ToolCall { status, output, .. } = &mut row.part {
                *status = ToolStatus::Error;
                *output = Some(format!("result was not persisted ({error}); treat this call as failed"));
            }
            let _ = self.store.save_part(row);
        }
        self.hub.publish(Event::PartUpdated { part: row.clone() });
    }
}

enum Step {
    Done,
    Continue,
    Retry,
    /// The request no longer fit the model's context.
    Overflow,
}

struct CallScope<'a> {
    plan: &'a Plan,
    message: &'a Message,
    files: &'a Arc<SessionFiles>,
    snapshot: &'a tokio::sync::OnceCell<Result<String, String>>,
    offered: &'a std::collections::HashSet<String>,
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

/// Identity of a prompt for replay checks: the same id must carry the same parts and model.
fn payload_hash(prompt: &Prompt) -> String {
    use sha2::Digest;
    let body = serde_json::json!({ "parts": prompt.parts, "model": prompt.model, "thinking": prompt.thinking_budget });
    sha2::Sha256::digest(body.to_string().as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn max_tokens(model: &Model, thinking_budget: Option<u32>) -> u32 {
    let cap = (model.limit.output as u32).clamp(1024, MAX_OUTPUT_TOKENS);
    cap.max(thinking_budget.unwrap_or(0) + 1024)
}

/// Prices are per million tokens.
fn cost(model: &Model, usage: Usage) -> f64 {
    let c = &model.cost;
    (usage.input as f64 * c.input + usage.output as f64 * c.output + usage.cache_read as f64 * c.cache_read + usage.cache_write as f64 * c.cache_write)
        / 1_000_000.0
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
