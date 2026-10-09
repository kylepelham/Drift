use serde_json::json;

use super::*;
use crate::session::types::{Message, MessageStatus, PartRow, Usage};

mod eligibility;
mod reasoning;
mod tools;

fn target() -> ModelRef {
    ModelRef {
        provider: "anthropic".into(),
        model: "claude".into(),
    }
}

fn messages(transcript: &[MessageWithParts], target: &impl Target) -> Vec<ChatMessage> {
    let mut output = Vec::new();
    append(&mut output, transcript, target);
    output
}

fn message_with(role: Role, status: MessageStatus, parts: Vec<Part>) -> MessageWithParts {
    MessageWithParts {
        info: Message {
            id: "m".into(),
            session_id: "s".into(),
            role,
            status,
            model: Some(target()),
            agent: None,
            usage: Usage::default(),
            cost: 0.0,
            error: None,
            created_at: 0,
            finished_at: None,
            summary: false,
            ending: None,
        },
        parts: parts
            .into_iter()
            .map(|part| PartRow {
                id: "p".into(),
                message_id: "m".into(),
                session_id: "s".into(),
                provider_signature: None,
                part,
            })
            .collect(),
    }
}

fn message(role: Role, parts: Vec<Part>) -> MessageWithParts {
    message_with(role, MessageStatus::Done, parts)
}

fn call(status: ToolStatus, output: Option<&str>) -> Part {
    Part::ToolCall {
        call_id: "c1".into(),
        name: "read".into(),
        input: json!({ "path": "a" }),
        status,
        title: None,
        output: output.map(Into::into),
        metadata: None,
        started_at: None,
        finished_at: None,
    }
}
