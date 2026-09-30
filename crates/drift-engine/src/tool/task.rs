//! Delegation: `task` runs a subagent, waiting for it or leaving it in the background; `task_output`
//! and `task_stop` look after background ones; `read_thread` checks on a conversation branched from this one.

use serde_json::{json, Value};

use super::{required_str, Ask, Context, Output, RunFuture, Tool, ToolError};
use crate::config::AgentKind;
use crate::event::Event;
use crate::llm::ToolSpec;
use crate::session::tasks::{clip, last_attempt, resolve_mode, Attempt, Mode, TaskRecord, TaskState, MAX_WAIT};
use crate::session::turn::Prompt;
use crate::session::types::{Part, Visibility};
use crate::store::{NewSession, NewTask};

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
                    "run_in_background": { "type": "boolean", "description": "Leave it running and carry on: this call returns at once with a task id, and the result arrives in this conversation when it finishes. For long work you do not need before your next step. Default: the subagent's own setting, else wait for it." }
                },
                "required": ["description", "prompt"]
            }),
        }
    }

    fn ask(&self, _ctx: &Context, _input: &Value) -> Option<Ask> {
        None
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let description = required_str(&input, "description")?;
            let text = required_str(&input, "prompt")?;
            let agent = input["subagent_type"].as_str().unwrap_or("general");
            let parent = ctx.engine.store.session(&ctx.session_id)?.ok_or(ToolError("parent session is gone".into()))?;
            if parent.visibility == Visibility::Hidden {
                return Err(ToolError("subagents cannot delegate".into()));
            }
            let config = ctx.engine.workspace_config(&ctx.workspace);
            let background_default = match config.agent(agent) {
                Some(found) if found.kind != AgentKind::Action => found.background,
                Some(_) => return Err(ToolError(format!("{agent} is an engine action, not an agent that can take a task"))),
                None => return Err(ToolError(format!("no agent named {agent}"))),
            };
            let (mode, reason) = resolve_mode(input["run_in_background"].as_bool(), background_default, ctx.engine.background_enabled()).map_err(ToolError)?;
            // An agent pinned to a model in Settings or its definition runs on it; otherwise the parent's model.
            let model = config.agent_model(agent).or(parent.model.clone());
            let title = format!("{description} (@{agent} subagent)");
            let child = ctx.engine.store.create_session(NewSession {
                workspace_id: &parent.workspace_id,
                parent_id: Some(&parent.id),
                visibility: Visibility::Hidden,
                title: &title,
                agent,
                model: model.as_ref(),
            })?;
            let new = NewTask { parent_session_id: &parent.id, session_id: &child.id, call_id: &ctx.call_id, description, agent, mode, reason };
            let (task, created) = ctx.engine.store.create_task(new)?;
            if !created {
                return Ok(receipt(&task));
            }
            ctx.engine.hub.publish(Event::SessionCreated { session: child.clone() });
            ctx.engine.publish_task(&task.id);
            ctx.engine.permissions.inherit(&child.id, &parent.id);
            let prompt = Prompt { parts: vec![Part::Text { text: text.into() }], model, thinking_budget: None, submission_id: None };
            if mode == Mode::Background {
                ctx.engine.launch(task.clone(), prompt);
                return Ok(receipt(&task));
            }
            foreground(ctx, &task, prompt).await
        })
    }

    /// Only a reply or a launch is a result; a failed or stopped subagent is a failed call that still links to its transcript.
    fn failed(&self, output: &Output) -> bool {
        !matches!(output.metadata["outcome"].as_str(), Some("replied" | "launched"))
    }
}

/// Runs the worker under the launching call and hands its result back as the call's result.
async fn foreground(ctx: &Context, task: &TaskRecord, prompt: Prompt) -> Result<Output, ToolError> {
    let engine = &ctx.engine;
    let started = engine.submit_under(&task.session_id, prompt, Some(&ctx.abort)).await;
    if let Err(error) = started {
        engine.end_task(&task.id, TaskState::Failed, &error.to_string());
        return Err(ToolError(format!("could not start subagent: {error}")));
    }
    engine.turns.wait_idle(&task.session_id, &ctx.abort).await;
    let (state, text) = if ctx.abort.is_cancelled() { (TaskState::Stopped, "aborted".to_string()) } else { engine.worker_result(&task.session_id) };
    engine.end_task(&task.id, state, &text);
    engine.store.mark_task_delivered(&task.id)?;
    engine.publish_task(&task.id);
    if ctx.abort.is_cancelled() {
        return Err(ToolError("aborted".into()));
    }
    let metadata = json!({ "sessionId": task.session_id, "taskId": task.id, "agent": task.agent, "outcome": state.as_str(), "mode": "foreground" });
    Ok(Output { title: task.description.clone(), output: text, metadata })
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

fn describe(task: &TaskRecord) -> String {
    let head = format!("{} (@{}, {}): {}", task.id, task.agent, task.description, task.state.as_str());
    match &task.result {
        Some(result) if task.state.is_terminal() => format!("{head}\n\n{result}"),
        _ => head,
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
            let mut task = owned(ctx, &input)?;
            let wait = std::time::Duration::from_secs(input["wait_seconds"].as_u64().unwrap_or(0)).min(MAX_WAIT);
            let until = tokio::time::Instant::now() + wait;
            while !task.state.is_terminal() && tokio::time::Instant::now() < until && !ctx.abort.is_cancelled() {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                task = owned(ctx, &input)?;
            }
            // Read here, it need not arrive again as a message.
            if task.state.is_terminal() && !task.delivered {
                ctx.engine.store.mark_task_delivered(&task.id)?;
                ctx.engine.publish_task(&task.id);
            }
            Ok(Output { title: task.description.clone(), output: describe(&task), metadata: json!({ "taskId": task.id, "state": task.state.as_str(), "sessionId": task.session_id }) })
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
                Attempt::Stopped => lines.push("Its last attempt was stopped.".into()),
                Attempt::Replied(_) | Attempt::None => {}
            }
            Ok(Output { title: child.title.clone(), output: lines.join("\n"), metadata: json!({ "sessionId": id, "running": running }) })
        })
    }
}
