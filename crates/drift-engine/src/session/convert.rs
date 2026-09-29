//! Stored transcript to model messages. Tool results become the user turn that follows each call.

use crate::llm::{Block, ChatMessage, Role as LlmRole};
use crate::session::types::{MessageWithParts, Part, Role, ToolStatus};

pub fn messages(transcript: &[MessageWithParts]) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::new();
    for message in transcript {
        match message.info.role {
            Role::User => push(&mut out, LlmRole::User, user_blocks(message)),
            Role::Assistant => {
                push(&mut out, LlmRole::Assistant, assistant_blocks(message));
                push(&mut out, LlmRole::User, result_blocks(message));
            }
        }
    }
    out
}

/// Adjacent messages of one role merge, since providers reject two in a row.
fn push(out: &mut Vec<ChatMessage>, role: LlmRole, blocks: Vec<Block>) {
    if blocks.is_empty() {
        return;
    }
    match out.last_mut() {
        Some(last) if last.role == role => last.blocks.extend(blocks),
        _ => out.push(ChatMessage { role, blocks }),
    }
}

fn user_blocks(message: &MessageWithParts) -> Vec<Block> {
    message
        .parts
        .iter()
        .filter_map(|row| match &row.part {
            Part::Text { text } if !text.is_empty() => Some(Block::Text(text.clone())),
            Part::File { mime, url, .. } if mime.starts_with("image/") => {
                url.split_once(",").map(|(_, data)| Block::Image { mime: mime.clone(), base64: data.to_string() })
            }
            _ => None,
        })
        .collect()
}

fn assistant_blocks(message: &MessageWithParts) -> Vec<Block> {
    message
        .parts
        .iter()
        .filter_map(|row| match &row.part {
            Part::Text { text } if !text.is_empty() => Some(Block::Text(text.clone())),
            Part::Reasoning { text, signature, redacted } if signature.is_some() || redacted.is_some() => {
                Some(Block::Reasoning { text: text.clone(), signature: signature.clone(), redacted: redacted.clone() })
            }
            Part::ToolCall { call_id, name, input, .. } => {
                Some(Block::ToolUse { id: call_id.clone(), name: name.clone(), input: input.clone() })
            }
            _ => None,
        })
        .collect()
}

/// Every call needs a result or the provider rejects the transcript; unfinished ones say so.
fn result_blocks(message: &MessageWithParts) -> Vec<Block> {
    message
        .parts
        .iter()
        .filter_map(|row| match &row.part {
            Part::ToolCall { call_id, status, output, .. } => {
                let (content, is_error) = match (status, output) {
                    (ToolStatus::Done, Some(output)) => (output.clone(), false),
                    (ToolStatus::Error, Some(output)) => (output.clone(), true),
                    (ToolStatus::Denied, _) => ("The user denied permission for this call.".into(), true),
                    _ => ("This call was interrupted before it produced a result.".into(), true),
                };
                Some(Block::ToolResult { call_id: call_id.clone(), content, is_error })
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::types::{Message, MessageStatus, PartRow, Usage};
    use serde_json::json;

    fn message(role: Role, parts: Vec<Part>) -> MessageWithParts {
        MessageWithParts {
            info: Message {
                id: "m".into(),
                session_id: "s".into(),
                role,
                status: MessageStatus::Done,
                model: None,
                usage: Usage::default(),
                cost: 0.0,
                error: None,
                created_at: 0,
                finished_at: None,
            },
            parts: parts
                .into_iter()
                .map(|part| PartRow { id: "p".into(), message_id: "m".into(), session_id: "s".into(), part })
                .collect(),
        }
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

    #[test]
    fn tool_calls_get_results_in_the_following_user_turn() {
        let transcript = vec![
            message(Role::User, vec![Part::Text { text: "read a".into() }]),
            message(Role::Assistant, vec![Part::Text { text: "ok".into() }, call(ToolStatus::Done, Some("1: x"))]),
            message(Role::Assistant, vec![Part::Text { text: "done".into() }]),
        ];
        let out = messages(&transcript);
        assert_eq!(out.len(), 4);
        assert_eq!(out[1].role, LlmRole::Assistant);
        assert!(matches!(&out[1].blocks[1], Block::ToolUse { id, .. } if id == "c1"));
        assert_eq!(out[2].blocks, vec![Block::ToolResult { call_id: "c1".into(), content: "1: x".into(), is_error: false }]);
        assert_eq!(out[3].blocks, vec![Block::Text("done".into())]);
    }

    #[test]
    fn unfinished_and_denied_calls_still_produce_results() {
        let transcript = vec![message(Role::Assistant, vec![call(ToolStatus::Running, None)]), message(Role::Assistant, vec![call(ToolStatus::Denied, None)])];
        let out = messages(&transcript);
        let Block::ToolResult { is_error, content, .. } = &out[1].blocks[0] else { panic!() };
        assert!(*is_error && content.contains("interrupted"));
        assert!(matches!(&out[3].blocks[0], Block::ToolResult { content, .. } if content.contains("denied")));
    }

    #[test]
    fn unsigned_reasoning_is_dropped_and_adjacent_users_merge() {
        let transcript = vec![
            message(Role::User, vec![Part::Text { text: "a".into() }]),
            message(Role::User, vec![Part::Text { text: "b".into() }]),
            message(Role::Assistant, vec![Part::Reasoning { text: "hm".into(), signature: None, redacted: None }, Part::Text { text: "x".into() }]),
        ];
        let out = messages(&transcript);
        assert_eq!(out[0].blocks, vec![Block::Text("a".into()), Block::Text("b".into())]);
        assert_eq!(out[1].blocks, vec![Block::Text("x".into())]);
    }
}
