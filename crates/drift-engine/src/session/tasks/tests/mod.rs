use crate::llm::Chunk;
use crate::permission::{Reply, ReplyBody};
use crate::session::turn::tests::{Harness, harness, prompt, text, tool, tool_call, until_idle};
use crate::session::types::{MessageWithParts, ToolStatus};
use crate::store::{NewSession, NewTask};
use crate::tool::Tool;
use serde_json::json;
use std::time::Duration;

use super::*;

mod delivery;
mod endings;
mod launch;
mod recovery;
mod scheduling;

fn background(description: &str, child_prompt: &str) -> serde_json::Value {
    json!({ "description": description, "prompt": child_prompt, "run_in_background": true })
}

/// One assistant message launching every task given, in order.
fn launches(tasks: &[serde_json::Value]) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    for (index, input) in tasks.iter().enumerate() {
        chunks.push(Chunk::ToolUseStart {
            id: format!("launch_{index}"),
            name: "task".into(),
        });
        chunks.push(Chunk::ToolInputDelta(input.to_string()));
        chunks.push(Chunk::BlockStop);
    }
    chunks.push(Chunk::Stop(crate::llm::StopReason::ToolUse));

    chunks
}

async fn until(what: &str, done: impl Fn() -> bool) {
    for _ in 0..600 {
        if done() {
            return;
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("never: {what}");
}

fn tasks(h: &Harness) -> Vec<TaskRecord> {
    h.engine.store.tasks_of(&h.session.id).unwrap()
}

fn delivered_results(transcript: &[MessageWithParts]) -> Vec<(String, String)> {
    transcript
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|row| match &row.part {
            Part::TaskResult { description, text, .. } => Some((description.clone(), text.clone())),
            _ => None,
        })
        .collect()
}

fn context(h: &Harness, session_id: &str, call_id: &str) -> crate::tool::Context {
    crate::tool::Context {
        agent: "build".into(),
        workspace: h._dir.join("ws"),
        session_id: session_id.into(),
        message_id: "msg".into(),
        call_id: call_id.into(),
        files: Default::default(),
        abort: CancellationToken::new(),
        engine: h.engine.clone(),
        config: Arc::new(h.engine.workspace_config(&h._dir.join("ws"))),
        progress: Default::default(),
        command_model: None,
    }
}

/// A worker of the harness session recorded as launched by `call_id`, at the owner's current Stop count.
fn recorded(h: &Harness, call_id: &str, mode: Mode) -> TaskRecord {
    let (_, generation) = h.engine.worker_scope(&h.session.id);
    let new = NewTask {
        generation,
        ..crate::store::tasks::tests::new_task(&h.session.id, call_id, mode)
    };

    h.engine
        .store
        .launch_task(new, crate::store::tasks::tests::child(&h.session))
        .unwrap()
        .task
}

/// A request whose conversation starts with `text`: a worker's own, not its parent's quoting the launch.
fn opens_with(request: &crate::llm::Request, text: &str) -> bool {
    let first = request.messages.first().and_then(|message| message.blocks.first());

    matches!(first, Some(crate::llm::Block::Text(first)) if first == text)
}

fn with_model(h: &Harness) {
    h.engine
        .store
        .update_session(&h.session.id, None, Some(&crate::session::turn::tests::model()), None)
        .unwrap();
}

/// A running parent task_output call whose result can later be saved by the turn.
fn call_row(h: &Harness, call_id: &str) -> PartRow {
    let message = h
        .engine
        .store
        .create_message(
            &h.session.id,
            Role::Assistant,
            Some(&crate::session::turn::tests::model()),
        )
        .unwrap();
    let call = Part::ToolCall {
        call_id: call_id.into(),
        name: "task_output".into(),
        input: json!({}),
        status: ToolStatus::Running,
        title: None,
        output: None,
        metadata: None,
        started_at: Some(1),
        finished_at: None,
    };

    h.engine.store.add_part(&message.id, &h.session.id, call).unwrap()
}

fn settlement(output: &crate::tool::Output) -> crate::session::turn::Settlement {
    crate::session::turn::Settlement::new(
        ToolStatus::Done,
        None,
        output.output.clone(),
        Some(output.metadata.clone()),
    )
}
