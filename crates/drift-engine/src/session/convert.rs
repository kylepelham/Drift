//! Stored transcript to model messages. Tool results become the user turn that follows each call.

use crate::llm::{Block, ChatMessage, Role as LlmRole};
use crate::session::types::{MessageStatus, ModelRef, MessageWithParts, Part, Role, ToolStatus};

/// Only what the provider actually finished is replayed; failed and still-streaming attempts are audit history.
fn replayable(message: &MessageWithParts) -> bool {
    message.info.role == Role::User || matches!(message.info.status, MessageStatus::Done | MessageStatus::Aborted)
}

/// `target` is the model the messages are for: reasoning signatures only validate with the model that made them.
pub fn messages(transcript: &[MessageWithParts], target: &ModelRef) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::new();
    for message in transcript.iter().filter(|m| replayable(m)) {
        match message.info.role {
            Role::User => push(&mut out, LlmRole::User, user_blocks(message)),
            Role::Assistant => {
                push(&mut out, LlmRole::Assistant, assistant_blocks(message, message.info.model.as_ref() == Some(target)));
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

fn assistant_blocks(message: &MessageWithParts, same_model: bool) -> Vec<Block> {
    message
        .parts
        .iter()
        .filter_map(|row| match &row.part {
            Part::Text { text } if !text.is_empty() => Some(Block::Text(text.clone())),
            Part::Reasoning { text, signature, redacted } if same_model && (signature.is_some() || redacted.is_some()) => {
                Some(Block::Reasoning { text: text.clone(), signature: signature.clone(), redacted: redacted.clone() })
            }
            Part::ToolCall { call_id, name, input, .. } if input.is_object() => {
                Some(Block::ToolUse { id: call_id.clone(), name: name.clone(), input: input.clone() })
            }
            _ => None,
        })
        .collect()
}

/// Every replayed call needs a result or the provider rejects the transcript; unfinished ones say so.
/// Calls whose arguments never parsed were not replayed, so they get no result either.
fn result_blocks(message: &MessageWithParts) -> Vec<Block> {
    message
        .parts
        .iter()
        .filter_map(|row| match &row.part {
            Part::ToolCall { call_id, status, output, input, .. } if input.is_object() => {
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
pub(super) mod tests_support {
    use super::*;
    use crate::session::types::{Message, MessageStatus, PartRow, Usage};

    pub(super) fn target() -> ModelRef {
        ModelRef { provider: "anthropic".into(), model: "claude".into() }
    }

    pub(super) fn message_with(role: Role, status: MessageStatus, parts: Vec<Part>) -> MessageWithParts {
        MessageWithParts {
            info: Message {
                id: "m".into(),
                session_id: "s".into(),
                role,
                status,
                model: Some(target()),
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
}

#[cfg(test)]
mod tests {
    use super::tests_support::*;
    use super::*;
    use crate::session::types::MessageStatus;
    use serde_json::json;

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

    #[test]
    fn tool_calls_get_results_in_the_following_user_turn() {
        let transcript = vec![
            message(Role::User, vec![Part::Text { text: "read a".into() }]),
            message(Role::Assistant, vec![Part::Text { text: "ok".into() }, call(ToolStatus::Done, Some("1: x"))]),
            message(Role::Assistant, vec![Part::Text { text: "done".into() }]),
        ];
        let out = messages(&transcript, &target());
        assert_eq!(out.len(), 4);
        assert_eq!(out[1].role, LlmRole::Assistant);
        assert!(matches!(&out[1].blocks[1], Block::ToolUse { id, .. } if id == "c1"));
        assert_eq!(out[2].blocks, vec![Block::ToolResult { call_id: "c1".into(), content: "1: x".into(), is_error: false }]);
        assert_eq!(out[3].blocks, vec![Block::Text("done".into())]);
    }

    #[test]
    fn unfinished_and_denied_calls_still_produce_results() {
        let transcript = vec![message(Role::Assistant, vec![call(ToolStatus::Running, None)]), message(Role::Assistant, vec![call(ToolStatus::Denied, None)])];
        let out = messages(&transcript, &target());
        let Block::ToolResult { is_error, content, .. } = &out[1].blocks[0] else { panic!() };
        assert!(*is_error && content.contains("interrupted"));
        assert!(matches!(&out[3].blocks[0], Block::ToolResult { content, .. } if content.contains("denied")));
    }

    #[test]
    fn signed_reasoning_goes_back_only_to_the_model_that_made_it() {
        let signed = Part::Reasoning { text: "hm".into(), signature: Some("sig".into()), redacted: None };
        let transcript = vec![message(Role::User, vec![Part::Text { text: "a".into() }]), message(Role::Assistant, vec![signed, Part::Text { text: "x".into() }])];
        assert_eq!(messages(&transcript, &target())[1].blocks.len(), 2);
        let other = ModelRef { provider: "openai".into(), model: "gpt-5".into() };
        assert_eq!(messages(&transcript, &other)[1].blocks, vec![Block::Text("x".into())]);
    }

    #[test]
    fn unsigned_reasoning_is_dropped_and_adjacent_users_merge() {
        let transcript = vec![
            message(Role::User, vec![Part::Text { text: "a".into() }]),
            message(Role::User, vec![Part::Text { text: "b".into() }]),
            message(Role::Assistant, vec![Part::Reasoning { text: "hm".into(), signature: None, redacted: None }, Part::Text { text: "x".into() }]),
        ];
        let out = messages(&transcript, &target());
        assert_eq!(out[0].blocks, vec![Block::Text("a".into()), Block::Text("b".into())]);
        assert_eq!(out[1].blocks, vec![Block::Text("x".into())]);
    }
}

#[cfg(test)]
mod eligibility_tests {
    use super::tests_support::*;
    use super::*;
    use crate::session::types::MessageStatus;

    #[test]
    fn failed_and_streaming_attempts_are_left_out_but_aborted_partials_stay() {
        let transcript = vec![
            message_with(Role::User, MessageStatus::Done, vec![Part::Text { text: "q".into() }]),
            message_with(Role::Assistant, MessageStatus::Error, vec![Part::Text { text: "half".into() }]),
            message_with(Role::Assistant, MessageStatus::Streaming, vec![Part::Text { text: "ghost".into() }]),
            message_with(Role::Assistant, MessageStatus::Aborted, vec![Part::Text { text: "partial".into() }]),
            message_with(Role::Assistant, MessageStatus::Done, vec![Part::Text { text: "final".into() }]),
        ];
        let out = messages(&transcript, &target());
        let texts: Vec<String> = out.iter().flat_map(|m| m.blocks.iter()).filter_map(|b| match b { Block::Text(t) => Some(t.clone()), _ => None }).collect();
        assert_eq!(texts, ["q", "partial", "final"]);
    }
}

#[cfg(test)]
mod incomplete_block_tests {
    use super::tests_support::*;
    use super::*;
    use crate::session::types::MessageStatus;

    #[test]
    fn aborted_rows_replay_only_their_valid_blocks() {
        let broken = Part::ToolCall {
            call_id: "c_bad".into(),
            name: "read".into(),
            input: serde_json::Value::String("{\"path".into()),
            status: ToolStatus::Pending,
            title: None,
            output: None,
            metadata: None,
            started_at: None,
            finished_at: None,
        };
        let transcript = vec![message_with(
            Role::Assistant,
            MessageStatus::Aborted,
            vec![
                Part::Reasoning { text: "cut off".into(), signature: None, redacted: None },
                Part::Text { text: "partial".into() },
                broken,
            ],
        )];
        let out = messages(&transcript, &target());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].blocks, vec![Block::Text("partial".into())]);
    }
}
