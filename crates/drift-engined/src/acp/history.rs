//! `session/load`: a stored conversation replayed as the updates that would have streamed it.

use std::path::Path;

use drift_engine::Engine;
use drift_engine::session::types::{Part, PartRow, Role};
use serde_json::{Value, json};

use super::rpc::{Peer, RpcError};
use super::{chunk, thought, tool};

pub(super) fn replay(engine: &Engine, peer: &Peer, session_id: &str, cwd: &Path) -> Result<(), RpcError> {
    let send = |update: Value| peer.notify("session/update", json!({ "sessionId": session_id, "update": update }));
    let messages = engine.store.transcript(session_id).map_err(RpcError::internal)?;

    // A compaction summary is the model's view of what came before; the editor shows the conversation itself.
    for message in messages.iter().filter(|message| !message.info.summary) {
        for row in &message.parts {
            if let Some(update) = replayed(message.info.role, row, cwd) {
                send(update);
            }
        }
    }

    let todos = engine.store.todos(session_id).map_err(RpcError::internal)?;
    if !todos.is_empty() {
        send(tool::plan(&todos));
    }

    Ok(())
}

fn replayed(role: Role, row: &PartRow, cwd: &Path) -> Option<Value> {
    match (&row.part, role) {
        (Part::Text { text }, _) if text.is_empty() => None,
        (Part::Text { text }, Role::User) => Some(chunk("user_message_chunk", &row.message_id, text)),
        (Part::Text { text }, Role::Assistant) => Some(chunk("agent_message_chunk", &row.message_id, text)),
        (Part::Reasoning { text, .. }, Role::Assistant) if !text.is_empty() => Some(thought(&row.message_id, text)),
        (Part::ToolCall { .. }, _) => tool::update(row, cwd, true),
        _ => None,
    }
}
