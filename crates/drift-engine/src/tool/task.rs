//! Delegation: `task` runs a subagent, waiting for it or leaving it in the background; `task_output`
//! and `task_stop` look after background ones; `read_thread` checks on a conversation branched from this one.

use serde_json::{json, Value};

use super::{required_str, Ask, Context, Output, RunFuture, Tool, ToolError};
use crate::config::AgentKind;
use crate::event::Event;
use crate::llm::ToolSpec;
use crate::session::tasks::{clip, last_attempt, resolve_mode, Attempt, Claimant, Mode, TaskRecord, TaskState, MAX_WAIT};
use crate::session::turn::Prompt;
use crate::session::types::{Part, Visibility};
use crate::store::{Launch, NewSession, NewTask};

/// Tools a subagent is never offered: delegation stays one level deep and branches belong to conversations.
pub const DELEGATION: [&str; 4] = ["task", "task_output", "task_stop", "read_thread"];

const SUMMARY_CHARS: usize = 4_000;

pub struct Task;

impl Tool for Task {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "task".into(),
            description: include_str!("prompts/task.txt").trim().into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "description": { "type": "string", "description": "Three to six words naming the job, shown to the user." },
                    "prompt": { "type": "string", "description": "Everything the subagent needs: the goal, what to return, constraints. It sees none of this conversation." },
                    "subagent_type": { "type": "string", "description": "A subagent from the list in the system prompt, such as explore for read-only searching. Default: general." },
                    "run_in_background": { "type": "boolean", "description": "Leave it running and carry on: this call returns at once with a task id, and the result arrives in this conversation when it finishes. For long work you do not need before your next step. Default: the subagent's own setting, else wait for it." },
                    "task_id": { "type": "string", "description": "Continue a finished subagent from this conversation instead of starting a new one: it keeps everything it saw and did, so `prompt` only needs the follow-up. Its agent stays as it was." }
                },
                "required": ["description", "prompt"]
            }),
        }
    }

    fn ask(&self, ctx: &Context, input: &Value) -> Option<Ask> {
        let resumed = input["task_id"].as_str().and_then(|id| ctx.engine.store.task(id).ok().flatten()).map(|task| task.agent);
        let agent = resumed.unwrap_or_else(|| input["subagent_type"].as_str().unwrap_or("general").to_string());
        Some(Ask::new("task", &agent, format!("Delegate to {agent}")).allow_by_default())
    }

    /// From a read-only agent, only a read-only subagent may take the job (a resumed one as it was).
    fn stays_read_only(&self, ctx: &Context, input: &Value) -> bool {
        let resumed = input["task_id"].as_str().and_then(|id| ctx.engine.store.task(id).ok().flatten()).map(|task| task.agent);
        let agent = resumed.unwrap_or_else(|| input["subagent_type"].as_str().unwrap_or("general").to_string());
        ctx.config.agent(&agent).is_some_and(|agent| agent.read_only)
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let description = required_str(&input, "description")?;
            let text = required_str(&input, "prompt")?;
            let parent = ctx.engine.store.session(&ctx.session_id)?.ok_or(ToolError("parent session is gone".into()))?;
            if parent.visibility == Visibility::Hidden {
                return Err(ToolError("subagents cannot delegate".into()));
            }
            let resumed = resumable(ctx, &input)?;
            // A resumed subagent stays the agent it was, whatever this call names.
            let agent = resumed.as_ref().map_or_else(|| input["subagent_type"].as_str().unwrap_or("general"), |earlier| earlier.agent.as_str());
            let config = &ctx.config;
            let background_default = match config.agent(agent) {
                Some(found) if found.kind != AgentKind::Action => found.usable().map_err(ToolError)?.background,
                Some(_) => return Err(ToolError(format!("{agent} is an engine action, not an agent that can take a task"))),
                None => return Err(ToolError(format!("no agent named {agent}"))),
            };
            let (mode, reason) = resolve_mode(input["run_in_background"].as_bool(), background_default, ctx.engine.background_enabled()).map_err(ToolError)?;
            // A user's command may choose the model; else the agent's pin from Settings or its definition; else the parent's model.
            let model = ctx.command_model.clone().or_else(|| config.agent_model(agent)).or(parent.model.clone());
            let title = format!("{description} (@{agent} subagent)");
            let (scope, generation) = ctx.engine.worker_scope(&parent.id);
            let new = NewTask { parent_session_id: &parent.id, call_id: &ctx.call_id, description, agent, mode, reason, generation };
            let Launch { task, child, created } = match &resumed {
                Some(earlier) => ctx.engine.store.resume_task(new, &earlier.session_id)?,
                None => {
                    let child = NewSession { workspace_id: &parent.workspace_id, parent_id: Some(&parent.id), visibility: Visibility::Hidden, title: &title, agent, model: model.as_ref() };
                    ctx.engine.store.launch_task(new, child)?
                }
            };
            if !created {
                return replay(ctx, task).await;
            }
            if resumed.is_none() {
                ctx.engine.hub.publish(Event::SessionCreated { session: child.clone() });
            }
            ctx.engine.publish_task(&task.id);
            ctx.engine.permissions.inherit(&child.id, &parent.id);
            // The parent's reasoning level carries over; a model that does not offer it runs at its default.
            let prompt = Prompt { parts: vec![Part::Text { text: text.into() }], model, variant: Some(parent.variant.clone()), agent: None, submission_id: None };
            // Its own stop: a background worker's descends from its owner's scope, a foreground one's from this call.
            let token = if mode == Mode::Background { scope.child_token() } else { ctx.abort.child_token() };
            match ctx.engine.admit_worker(&task, prompt, token).await {
                Ok(None) => Ok(receipt(&task)),
                Ok(Some(outcome)) => own_result(ctx, &task.id, outcome),
                // It could not start: this call's failed result is how the parent hears of it.
                Err(_) => own_result(ctx, &task.id, "failed"),
            }
        })
    }

    /// A foreground worker is waited for here even when Stop comes, so it ends, and is recorded, before the call does.
    fn stops_itself(&self) -> bool {
        true
    }

    /// Only a reply or a launch is a result; a failed or stopped subagent is a failed call that still links to its transcript.
    fn failed(&self, output: &Output) -> bool {
        !matches!(output.metadata["outcome"].as_str(), Some("replied" | "launched"))
    }
}

/// The earlier task a call asks to continue: one of this conversation's, finished.
fn resumable(ctx: &Context, input: &Value) -> Result<Option<TaskRecord>, ToolError> {
    if input["task_id"].as_str().is_none() {
        return Ok(None);
    }
    let earlier = owned(ctx, input)?;
    if !earlier.state.is_terminal() {
        return Err(ToolError(format!("{} is still running; wait for its result, or stop it, before continuing it", earlier.id)));
    }
    Ok(Some(earlier))
}

/// The same call again gets what it launched, in its own mode; nothing new is recorded or started.
async fn replay(ctx: &Context, task: TaskRecord) -> Result<Output, ToolError> {
    if task.mode == Mode::Background {
        return Ok(receipt(&task));
    }
    let mut task = task;
    while !task.state.is_terminal() {
        tokio::select! {
            () = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
            () = ctx.abort.cancelled() => return Err(ToolError("aborted".into())),
        }
        task = ctx.engine.store.task(&task.id)?.ok_or_else(|| ToolError("the task is gone".into()))?;
    }
    let outcome = match task.state {
        TaskState::Replied => "replied",
        TaskState::Stopped => "stopped",
        TaskState::Interrupted => "interrupted",
        _ => "failed",
    };
    own_result(ctx, &task.id, outcome)
}

/// A finished worker's result as this launching call's own, claimed for the write that saves it, stopped or not.
fn own_result(ctx: &Context, task_id: &str, outcome: &str) -> Result<Output, ToolError> {
    let engine = &ctx.engine;
    let task = engine.store.task(task_id)?.ok_or_else(|| ToolError("the task is gone".into()))?;
    let claimed = !task.delivered && engine.workers.claim(&task.id, Claimant::call(&ctx.session_id, &ctx.call_id));
    let said = if task.delivered { "This subagent's result was already handed over.".to_string() } else { task.result.clone().unwrap_or_default() };
    let output = format!("{said}\n\n(task_id: {}; pass it to task to continue this subagent's conversation)", task.id);
    let mut metadata = json!({ "sessionId": task.session_id, "taskId": task.id, "agent": task.agent, "outcome": outcome, "mode": task.mode.as_str() });
    if claimed {
        metadata["delivers"] = json!(task.id);
    }
    Ok(Output { title: task.description.clone(), output, metadata })
}

/// What a background launch returns at once: the task, never its result.
fn receipt(task: &TaskRecord) -> Output {
    let output = format!(
        "Started {} in the background as {} (@{}). Carry on with other work: its result will arrive in this conversation when it finishes. Use task_output only if you cannot continue without it.",
        task.description, task.id, task.agent
    );
    let metadata = json!({ "sessionId": task.session_id, "taskId": task.id, "agent": task.agent, "outcome": "launched", "mode": task.mode.as_str(), "reason": task.reason });
    Output { title: task.description.clone(), output, metadata }
}

/// A task this conversation launched, or a refusal naming why not.
fn owned(ctx: &Context, input: &Value) -> Result<TaskRecord, ToolError> {
    let id = required_str(input, "task_id")?;
    let task = ctx.engine.store.task(id)?.ok_or_else(|| ToolError(format!("no task {id}")))?;
    if task.parent_session_id != ctx.session_id {
        return Err(ToolError(format!("{id} was not launched from this conversation")));
    }
    Ok(task)
}

/// A background task of this conversation; a foreground result belongs to the call that launched it.
fn owned_background(ctx: &Context, input: &Value) -> Result<TaskRecord, ToolError> {
    let task = owned(ctx, input)?;
    if task.mode == Mode::Foreground {
        return Err(ToolError(format!("{} ran in the foreground; its result is the result of the task call that launched it", task.id)));
    }
    Ok(task)
}

/// The task's state, and its result only when this call is the one handing it over.
fn describe(task: &TaskRecord, hands_over: bool) -> String {
    let head = format!("{} (@{}, {}): {}", task.id, task.agent, task.description, task.state.as_str());
    match &task.result {
        Some(result) if hands_over => format!("{head}\n\n{result}"),
        _ if !task.state.is_terminal() => head,
        _ if task.delivered => format!("{head}\n\nIts result was already handed to this conversation."),
        _ => format!("{head}\n\nIts result is arriving in this conversation as a message."),
    }
}

pub struct TaskOutput;

impl Tool for TaskOutput {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "task_output".into(),
            description: "Checks on a background task this conversation started: whether it is still running, and its result once it has one. Results arrive by themselves when a task finishes, so use this only when you cannot go on without the result; do not poll it in a loop.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "The id the task call returned." },
                    "wait_seconds": { "type": "integer", "description": "Wait up to this long for it to finish, at most 120. Default 0: answer at once." }
                },
                "required": ["task_id"]
            }),
        }
    }

    fn ask(&self, _ctx: &Context, _input: &Value) -> Option<Ask> {
        None
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let mut task = owned_background(ctx, &input)?;
            // Claimed while it waits, so a result finishing meanwhile comes here and not also as a message.
            let claimed = !task.delivered && ctx.engine.workers.claim(&task.id, Claimant::call(&ctx.session_id, &ctx.call_id));
            let wait = std::time::Duration::from_secs(input["wait_seconds"].as_u64().unwrap_or(0)).min(MAX_WAIT);
            let until = tokio::time::Instant::now() + wait;
            while !task.state.is_terminal() && tokio::time::Instant::now() < until && !ctx.abort.is_cancelled() {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                task = owned(ctx, &input)?;
            }
            let hands_over = claimed && task.state.is_terminal() && !task.delivered;
            let mut metadata = json!({ "taskId": task.id, "state": task.state.as_str(), "sessionId": task.session_id });
            if hands_over {
                metadata["delivers"] = json!(task.id);
            }
            Ok(Output { title: task.description.clone(), output: describe(&task, hands_over), metadata })
        })
    }
}

pub struct TaskStop;

impl Tool for TaskStop {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "task_stop".into(),
            description: "Stops a background task this conversation started, and only that one.".into(),
            input_schema: json!({
                "type": "object",
                "properties": { "task_id": { "type": "string", "description": "The id the task call returned." } },
                "required": ["task_id"]
            }),
        }
    }

    fn ask(&self, _ctx: &Context, _input: &Value) -> Option<Ask> {
        None
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let task = owned(ctx, &input)?;
            if task.state.is_terminal() {
                return Ok(Output::new(task.description.clone(), format!("{} had already ended: {}.", task.id, task.state.as_str())));
            }
            let task = ctx.engine.stop_task(&task.id).map_err(|e| ToolError(e.to_string()))?;
            Ok(Output::new(task.description.clone(), format!("Stopping {}.", task.id)))
        })
    }
}

pub struct ReadThread;

impl Tool for ReadThread {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_thread".into(),
            description: "Reads a conversation the user branched off this one: whether it is still running, what it is waiting on, and its latest reply. Use it only when the user asks about that conversation.".into(),
            input_schema: json!({
                "type": "object",
                "properties": { "id": { "type": "string", "description": "The branched conversation's session id." } },
                "required": ["id"]
            }),
        }
    }

    fn ask(&self, _ctx: &Context, _input: &Value) -> Option<Ask> {
        None
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let id = required_str(&input, "id")?;
            let child = ctx.engine.store.session(id)?.ok_or_else(|| ToolError(format!("no thread {id}")))?;
            if child.parent_id.as_deref() != Some(ctx.session_id.as_str()) {
                return Err(ToolError("that conversation was not branched from this one".into()));
            }
            let running = ctx.engine.turns.is_running(id);
            let asks = ctx.engine.permissions.pending().into_iter().filter(|p| p.session_id == id).count()
                + ctx.engine.questions.pending().into_iter().filter(|q| q.session_id == id).count();
            let mut lines = vec![format!("Thread: {}", child.title), format!("Status: {}", if running { "running" } else { "idle" })];
            if asks > 0 {
                lines.push(format!("Waiting on the user: {asks} pending request(s)"));
            }
            let todos = ctx.engine.store.todos(id)?;
            if !todos.is_empty() {
                lines.push(format!("Todos: {}", serde_json::to_string(&todos).unwrap()));
            }
            match last_attempt(&ctx.engine.store, id) {
                Attempt::Replied(reply) if !reply.is_empty() => lines.push(format!("Latest reply:\n{}", clip(&reply, SUMMARY_CHARS))),
                Attempt::Failed(error) => lines.push(format!("Its last attempt failed: {error}")),
                Attempt::Incomplete(partial) => lines.push(format!("Its latest reply stopped at the output limit, unfinished:\n{}", clip(&partial, SUMMARY_CHARS))),
                Attempt::Limited(write_up) => lines.push(format!("It reached its step or repeat limit and wrote up where it got to:\n{}", clip(&write_up, SUMMARY_CHARS))),
                Attempt::Refused(_) => lines.push("Its latest reply was ended by the provider's safety filter.".into()),
                Attempt::Stopped => lines.push("Its last attempt was stopped.".into()),
                Attempt::Replied(_) | Attempt::None => {}
            }
            Ok(Output { title: child.title.clone(), output: lines.join("\n"), metadata: json!({ "sessionId": id, "running": running }) })
        })
    }
}
