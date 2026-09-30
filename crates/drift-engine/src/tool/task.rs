//! Delegation: `task` runs a hidden child to completion, `spawn_thread` opens a sibling the user can see.

use serde_json::{json, Value};

use super::{required_str, Ask, Context, Output, RunFuture, Tool, ToolError};
use crate::event::Event;
use crate::llm::ToolSpec;
use crate::session::turn::Prompt;
use crate::session::types::{Part, Role, Visibility};
use crate::store::NewSession;

/// How much of a child's final reply comes back to the parent verbatim.
const RESULT_CHARS: usize = 20_000;
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
                    "subagent_type": { "type": "string", "description": "An agent from the workspace config. Default: build." }
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
            let agent = input["subagent_type"].as_str().unwrap_or("build");
            let parent = ctx.engine.store.session(&ctx.session_id)?.ok_or(ToolError("parent session is gone".into()))?;
            let title = format!("{description} (@{agent} subagent)");
            let child = ctx.engine.store.create_session(NewSession {
                workspace_id: &parent.workspace_id,
                parent_id: Some(&parent.id),
                visibility: Visibility::Hidden,
                title: &title,
                agent,
                model: parent.model.as_ref(),
            })?;
            ctx.engine.hub.publish(Event::SessionCreated { session: child.clone() });
            let prompt = Prompt { parts: vec![Part::Text { text: text.into() }], model: parent.model.clone(), thinking_budget: None, submission_id: None };
            ctx.engine.submit(&child.id, prompt).await.map_err(|e| ToolError(format!("could not start subagent: {e}")))?;
            ctx.engine.turns.wait_idle(&child.id, &ctx.abort).await;
            if ctx.abort.is_cancelled() {
                return Err(ToolError("aborted".into()));
            }
            let reply = final_reply(&ctx.engine.store, &child.id)?;
            Ok(Output { title: description.into(), output: clip(&reply, RESULT_CHARS), metadata: json!({ "sessionId": child.id, "agent": agent }) })
        })
    }
}

pub struct SpawnThread;

impl Tool for SpawnThread {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "spawn_thread".into(),
            description: include_str!("prompts/spawn_thread.txt").trim().into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Short title for the new thread." },
                    "task": { "type": "string", "description": "What the thread should do." },
                    "summary": { "type": "string", "description": "The context it needs from this conversation, in your words." },
                    "context": { "type": "string", "description": "Verbatim excerpts worth carrying over." }
                },
                "required": ["title", "task", "summary"]
            }),
        }
    }

    fn ask(&self, _ctx: &Context, _input: &Value) -> Option<Ask> {
        None
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let title = required_str(&input, "title")?;
            let task = required_str(&input, "task")?;
            let summary = required_str(&input, "summary")?;
            let parent = ctx.engine.store.session(&ctx.session_id)?.ok_or(ToolError("parent session is gone".into()))?;
            let child = ctx.engine.store.create_session(NewSession {
                workspace_id: &parent.workspace_id,
                parent_id: Some(&parent.id),
                visibility: Visibility::Sibling,
                title,
                agent: &parent.agent,
                model: parent.model.as_ref(),
            })?;
            ctx.engine.hub.publish(Event::SessionCreated { session: child.clone() });
            let mut text = format!("# Context from the parent thread\n\n{summary}\n");
            if let Some(excerpts) = input["context"].as_str().filter(|c| !c.trim().is_empty()) {
                text.push_str(&format!("\n## Excerpts\n\n{excerpts}\n"));
            }
            text.push_str(&format!("\n# Task\n\n{task}"));
            let prompt = Prompt { parts: vec![Part::Text { text }], model: parent.model.clone(), thinking_budget: None, submission_id: None };
            ctx.engine.submit(&child.id, prompt).await.map_err(|e| ToolError(format!("could not start thread: {e}")))?;
            Ok(Output {
                title: title.into(),
                output: format!("Started thread \"{title}\" ({}). It runs on its own; use read_thread to check on it.", child.id),
                metadata: json!({ "sessionId": child.id }),
            })
        })
    }
}

pub struct ReadThread;

impl Tool for ReadThread {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_thread".into(),
            description: "Reads a thread this conversation spawned: whether it is still running, what it is waiting on, and its latest reply.".into(),
            input_schema: json!({
                "type": "object",
                "properties": { "id": { "type": "string", "description": "The thread id returned by spawn_thread." } },
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
                return Err(ToolError("that thread was not spawned by this conversation".into()));
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
            let reply = final_reply(&ctx.engine.store, id)?;
            if !reply.is_empty() {
                lines.push(format!("Latest reply:\n{}", clip(&reply, SUMMARY_CHARS)));
            }
            Ok(Output { title: child.title.clone(), output: lines.join("\n"), metadata: json!({ "sessionId": id, "running": running }) })
        })
    }
}

/// Text of the last completed assistant message, which is what a subagent hands back.
fn final_reply(store: &crate::store::Store, session_id: &str) -> Result<String, ToolError> {
    let transcript = store.transcript(session_id)?;
    let last = transcript.iter().rev().find(|m| m.info.role == Role::Assistant && m.info.status == crate::session::types::MessageStatus::Done);
    let Some(last) = last else {
        let error = transcript.iter().rev().find_map(|m| m.info.error.clone());
        return Ok(error.map_or(String::new(), |e| format!("The subagent failed: {e}")));
    };
    Ok(last.parts.iter().filter_map(|row| match &row.part { Part::Text { text } => Some(text.as_str()), _ => None }).collect::<Vec<_>>().join("\n"))
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.into();
    }
    format!("{}\n\n(truncated)", text.chars().take(max).collect::<String>())
}
