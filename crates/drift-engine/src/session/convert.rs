//! Stored transcript to model messages. Tool results become the user turn that follows each call.

use crate::llm::{Block, ChatMessage, Role as LlmRole};
use crate::session::types::{MessageStatus, ModelRef, MessageWithParts, Part, Role, ToolStatus};

/// Only what the provider actually finished is replayed; failed and still-streaming attempts are audit history.
fn replayable(message: &MessageWithParts) -> bool {
    message.info.role == Role::User || matches!(message.info.status, MessageStatus::Done | MessageStatus::Aborted)
}

/// Converts `transcript` onto `out`, merging with what is already there. `target` is the model the
/// messages are for: reasoning signatures only validate with the model that made them.
pub fn append<'a>(out: &mut Vec<ChatMessage>, transcript: impl IntoIterator<Item = &'a MessageWithParts>, target: &ModelRef) {
    for message in transcript.into_iter().filter(|m| replayable(m)) {
        match message.info.role {
            Role::User => push(out, LlmRole::User, user_blocks(message)),
            Role::Assistant => {
                push(out, LlmRole::Assistant, assistant_blocks(message, message.info.model.as_ref() == Some(target)));
                push(out, LlmRole::User, result_blocks(message));
            }
        }
    }
}

/// Adjacent messages of one role merge, since providers reject two in a row.
pub fn push(out: &mut Vec<ChatMessage>, role: LlmRole, blocks: Vec<Block>) {
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
            Part::File { mime, url, .. } if mime.starts_with("text/") => super::attach::data_text(url).map(Block::Text),
            Part::TaskResult { task_id, description, outcome, text, .. } => {
                Some(Block::Text(format!("<task-result id=\"{task_id}\" description=\"{description}\" outcome=\"{outcome}\">\n{text}\n</task-result>")))
            }
            Part::Clarification { request_id, items } => Some(Block::Text(clarification_text(request_id, items))),
            _ => None,
        })
        .collect()
}

/// How the model reads an answer that arrives after it moved on.
pub(crate) fn clarification_text(request_id: &str, items: &[super::types::Clarified]) -> String {
    let answers: Vec<String> = items.iter().map(|item| format!("{}\nAnswer: {}", item.question, item.answers.join(", "))).collect();
    format!("<question-answer id=\"{request_id}\">\nThe user answered the question you asked earlier.\n\n{}\n</question-answer>", answers.join("\n\n"))
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
/// Calls whose arguments never parsed were not replayed, so they get no result either. Images a
/// call returned follow all the results, since providers want results first in the turn.
fn result_blocks(message: &MessageWithParts) -> Vec<Block> {
    let mut results = Vec::new();
    let mut images = Vec::new();
    for row in &message.parts {
        let Part::ToolCall { call_id, name, status, output, input, metadata, .. } = &row.part else { continue };
        if !input.is_object() {
            continue;
        }
        let (content, is_error) = match (status, output) {
            (ToolStatus::Done, Some(output)) => (output.clone(), false),
            (ToolStatus::Error | ToolStatus::Denied, Some(output)) => (output.clone(), true),
            (ToolStatus::Denied, None) => ("The user denied permission for this call.".into(), true),
            _ => ("This call was interrupted before it produced a result.".into(), true),
        };
        results.push(Block::ToolResult { call_id: call_id.clone(), content, is_error });
        let returned = crate::tool::image::from_metadata(metadata.as_ref());
        if !returned.is_empty() {
            images.push(Block::Text(format!("The {name} call ({call_id}) returned this:")));
            images.extend(returned.into_iter().map(|image| Block::Image { mime: image.mime, base64: image.base64 }));
        }
    }
    results.extend(images);
    results
}

#[cfg(test)]
pub(super) mod tests_support {
    use super::*;
    use crate::session::types::{Message, MessageStatus, PartRow, Usage};

    pub(super) fn target() -> ModelRef {
        ModelRef { provider: "anthropic".into(), model: "claude".into() }
    }

    pub(super) fn messages(transcript: &[MessageWithParts], target: &ModelRef) -> Vec<ChatMessage> {
        let mut out = Vec::new();
        append(&mut out, transcript, target);
        out
    }

    pub(super) fn message_with(role: Role, status: MessageStatus, parts: Vec<Part>) -> MessageWithParts {
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
    fn returned_images_follow_every_result_of_the_turn() {
        let mut with_image = call(ToolStatus::Done, Some("an image"));
        if let Part::ToolCall { metadata, .. } = &mut with_image {
            *metadata = Some(json!({ "images": [{ "mime": "image/png", "data": "AAAA" }] }));
        }
        let mut second = call(ToolStatus::Done, Some("text"));
        if let Part::ToolCall { call_id, .. } = &mut second {
            *call_id = "c2".into();
        }
        let out = messages(&[message(Role::Assistant, vec![with_image, second])], &target());
        let kinds: Vec<&str> = out[1].blocks.iter().map(|b| match b { Block::ToolResult { .. } => "result", Block::Text(_) => "text", Block::Image { .. } => "image", _ => "other" }).collect();
        assert_eq!(kinds, ["result", "result", "text", "image"]);
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
