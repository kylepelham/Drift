//! Stored transcript to model messages. Tool results become the user turn that follows each call.

use crate::llm::catalog::Catalog;
use crate::llm::{Block, ChatMessage, Role as LlmRole};
use crate::session::types::{MessageStatus, MessageWithParts, ModelRef, Part, Role, ToolStatus};

/// Only what the provider actually finished is replayed; failed and still-streaming attempts are audit history.
fn replayable(message: &MessageWithParts) -> bool {
    message.info.role == Role::User || matches!(message.info.status, MessageStatus::Done | MessageStatus::Aborted)
}

/// The model a request's history is for: reasoning signatures validate only with the model that made them.
pub trait Target {
    fn wrote(&self, model: &ModelRef) -> bool;
}

/// Exactly this entry.
impl Target for ModelRef {
    fn wrote(&self, model: &ModelRef) -> bool {
        self == model
    }
}

/// This entry or any that runs the same model, a mode and its base (`Catalog::same_model`).
pub struct OnCatalog<'a> {
    pub model: &'a ModelRef,
    pub catalog: &'a Catalog,
}

impl Target for OnCatalog<'_> {
    fn wrote(&self, model: &ModelRef) -> bool {
        self.catalog.same_model(self.model, model)
    }
}

/// Converts `transcript` onto `out`, merging with what is already there, for `target`.
pub fn append<'a>(
    out: &mut Vec<ChatMessage>,
    transcript: impl IntoIterator<Item = &'a MessageWithParts>,
    target: &impl Target,
) {
    let mut used = std::collections::HashSet::new();
    for message in transcript.into_iter().filter(|m| replayable(m)) {
        match message.info.role {
            Role::User => push(out, LlmRole::User, user_blocks(message)),
            Role::Assistant => {
                let mut calls = assistant_blocks(
                    message,
                    message.info.model.as_ref().is_some_and(|model| target.wrote(model)),
                );
                let mut results = result_blocks(message);
                unique_ids(&mut used, &mut calls, &mut results);
                push(out, LlmRole::Assistant, calls);
                push(out, LlmRole::User, results);
            }
        }
    }
}

/// Sessions stored before ids were made unique can hold one id twice (Gemini's `call_1` in every
/// step), which Anthropic refuses; a repeat goes out suffixed, on its call and its result alike.
fn unique_ids(used: &mut std::collections::HashSet<String>, calls: &mut [Block], results: &mut [Block]) {
    let mut renamed: Vec<(String, String)> = Vec::new();
    for block in calls.iter_mut() {
        let Block::ToolUse { id, .. } = unsigned_mut(block) else {
            continue;
        };
        if used.insert(id.clone()) {
            continue;
        }
        let fresh = (2..)
            .map(|n| format!("{id}_{n}"))
            .find(|candidate| !used.contains(candidate))
            .unwrap_or_default();
        used.insert(fresh.clone());
        renamed.push((std::mem::replace(id, fresh.clone()), fresh));
    }
    for block in results.iter_mut() {
        let Block::ToolResult { call_id, .. } = block else {
            continue;
        };
        if let Some(at) = renamed.iter().position(|(old, _)| old == call_id) {
            *call_id = renamed.remove(at).1;
        }
    }
}

fn unsigned_mut(block: &mut Block) -> &mut Block {
    match block {
        Block::Signed { part, .. } => unsigned_mut(part),
        other => other,
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
            Part::Text { text } | Part::Nudge { text } if !text.is_empty() => Some(Block::Text(text.clone())),
            Part::Context { plugin, text } => Some(Block::Text(format!("<system-reminder>\nFrom the {plugin} plugin:\n{text}\n</system-reminder>"))),
            Part::File { mime, url, .. } if mime.starts_with("image/") => {
                url.split_once(",").map(|(_, data)| Block::Image { mime: mime.clone(), base64: data.to_string() })
            }
            Part::File { mime, url, .. } if mime.starts_with("text/") => super::attach::data_text(url).map(Block::Text),
            Part::File { mime, url, .. } if mime.eq_ignore_ascii_case(crate::tool::image::PDF) => url.split_once(",").map(|(_, data)| Block::Pdf { base64: data.to_string() }),
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
    let answers: Vec<String> = items
        .iter()
        .map(|item| format!("{}\nAnswer: {}", item.question, item.answers.join(", ")))
        .collect();
    format!(
        "<question-answer id=\"{request_id}\">\nThe user answered the question you asked earlier.\n\n{}\n</question-answer>",
        answers.join("\n\n")
    )
}

/// Unsigned reasoning (Chat Completions `reasoning_content`) goes back only within the turn that
/// wrote it: Kimi, GLM and DeepSeek want it through their tool loop and ignore or refuse it from
/// earlier turns. The turn is known here, not from the wire, where a prompt steered in mid-loop and
/// one sent after a Stop both land beside the last tool results. `started` is the turn's own prompt.
pub(super) fn drop_earlier_reasoning(transcript: &mut [MessageWithParts], started: Option<&str>) {
    let Some(started) = started else { return };
    for message in transcript
        .iter_mut()
        .filter(|m| m.info.role == Role::Assistant && m.info.id.as_str() < started)
    {
        message.parts.retain(|row| {
            row.provider_signature.is_some()
                || !matches!(
                    row.part,
                    Part::Reasoning {
                        signature: None,
                        redacted: None,
                        ..
                    }
                )
        });
    }
}

/// Reasoning goes back as reasoning only to the model that wrote it: signed or redacted always,
/// unsigned (Chat Completions `reasoning_content`) only from a finished reply; each adapter keeps what
/// its wire takes. Another model reads a finished thought as plain text, as opencode sends it, since it
/// cannot check the signature.
fn assistant_blocks(message: &MessageWithParts, same_model: bool) -> Vec<Block> {
    let finished = message.info.status == MessageStatus::Done;
    message
        .parts
        .iter()
        .filter_map(|row| {
            let signed = row.provider_signature.is_some();
            let block = match &row.part {
                // An empty signed text part matters only to the model that signed it; others refuse empty text.
                Part::Text { text } if !text.is_empty() || (same_model && signed) => Some(Block::Text(text.clone())),
                Part::Reasoning {
                    text,
                    signature,
                    redacted,
                } if same_model
                    && (signed || signature.is_some() || redacted.is_some() || (finished && !text.is_empty())) =>
                {
                    Some(Block::Reasoning {
                        text: text.clone(),
                        signature: signature.clone(),
                        redacted: redacted.clone(),
                    })
                }
                // A signature arrives when the thought ends, so a signed one is whole even in a reply cut off later.
                Part::Reasoning { text, signature, .. }
                    if !same_model && !text.trim().is_empty() && (finished || signed || signature.is_some()) =>
                {
                    Some(Block::Text(text.clone()))
                }
                Part::ToolCall {
                    metadata: Some(metadata),
                    ..
                } if metadata.engine_command.is_some() => None,
                Part::ToolCall {
                    call_id,
                    name,
                    input,
                    status,
                    output,
                    ..
                } => replayed_input(input, *status, output.as_deref()).map(|input| Block::ToolUse {
                    id: call_id.clone(),
                    name: name.clone(),
                    input,
                }),
                _ => None,
            }?;
            Some(match row.provider_signature.as_ref().filter(|_| same_model) {
                Some(signature) => Block::Signed {
                    part: Box::new(block),
                    signature: signature.clone(),
                },
                None => block,
            })
        })
        .collect()
}

/// A call's input as replayed. One whose arguments never parsed goes back as `{}` once the engine has
/// answered it with the parse error, so the model reads why; one cut off mid-stream is left out.
fn replayed_input(input: &serde_json::Value, status: ToolStatus, output: Option<&str>) -> Option<serde_json::Value> {
    match input {
        serde_json::Value::Object(_) => Some(input.clone()),
        _ if status == ToolStatus::Error && output.is_some() => Some(serde_json::json!({})),
        _ => None,
    }
}

/// Every replayed call needs a result or the provider rejects the transcript; unfinished ones say so.
/// Calls whose arguments never parsed were not replayed, so they get no result either. Images a
/// call returned follow all the results, since providers want results first in the turn.
fn result_blocks(message: &MessageWithParts) -> Vec<Block> {
    let mut results = Vec::new();
    let mut images = Vec::new();
    for row in &message.parts {
        let Part::ToolCall {
            call_id,
            name,
            status,
            output,
            input,
            metadata,
            ..
        } = &row.part
        else {
            continue;
        };
        if replayed_input(input, *status, output.as_deref()).is_none() {
            continue;
        }
        let (content, is_error) = match (status, output) {
            (ToolStatus::Done, Some(output)) => (output.clone(), false),
            (ToolStatus::Error | ToolStatus::Denied, Some(output)) => (output.clone(), true),
            (ToolStatus::Denied, None) => ("The user denied permission for this call.".into(), true),
            _ => ("This call was interrupted before it produced a result.".into(), true),
        };
        if let Some(command) = metadata
            .as_ref()
            .and_then(|metadata| metadata.engine_command.as_deref())
        {
            let ran = input["command"]
                .as_str()
                .filter(|_| name == "bash")
                .map(|line| format!(" ran `{line}`, which"))
                .unwrap_or_default();
            results.push(Block::Text(format!(
                "The /{command} command{ran} {}:\n{content}",
                if is_error { "failed" } else { "returned" }
            )));
        } else {
            results.push(Block::ToolResult {
                call_id: call_id.clone(),
                content,
                is_error,
            });
        }
        let returned = crate::tool::image::stored(metadata.as_deref());
        if !returned.is_empty() {
            images.push(Block::Text(format!("The {name} call ({call_id}) returned this:")));
            images.extend(returned.into_iter().map(|image| Block::Stored {
                mime: image.mime,
                hash: image.hash,
            }));
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
        ModelRef {
            provider: "anthropic".into(),
            model: "claude".into(),
        }
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

    #[test]
    fn a_call_id_stored_twice_by_an_older_session_goes_out_unique_with_its_result() {
        let step = || message(Role::Assistant, vec![call(ToolStatus::Done, Some("ok"))]);
        let transcript = [
            message(Role::User, vec![Part::Text { text: "q".into() }]),
            step(),
            step(),
            step(),
        ];
        let sent = messages(&transcript, &target());
        let flat: Vec<&Block> = sent.iter().flat_map(|m| &m.blocks).collect();
        let uses: Vec<&str> = flat
            .iter()
            .filter_map(|b| match b {
                Block::ToolUse { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        let answers: Vec<&str> = flat
            .iter()
            .filter_map(|b| match b {
                Block::ToolResult { call_id, .. } => Some(call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(uses, ["c1", "c1_2", "c1_3"]);
        assert_eq!(answers, uses, "each result answers its own renamed call");
    }

    #[test]
    fn an_empty_signed_text_part_goes_only_to_the_model_that_signed_it() {
        let mut reply = message(
            Role::Assistant,
            vec![Part::Text { text: "answer".into() }, Part::Text { text: String::new() }],
        );
        reply.parts[1].provider_signature = Some("sig".into());
        let transcript = [message(Role::User, vec![Part::Text { text: "q".into() }]), reply];
        let same = messages(&transcript, &target());
        assert!(
            matches!(&same[1].blocks[..], [Block::Text(_), Block::Signed { part, .. }] if matches!(part.as_ref(), Block::Text(text) if text.is_empty()))
        );
        let other = messages(
            &transcript,
            &ModelRef {
                provider: "openai".into(),
                model: "gpt".into(),
            },
        );
        assert_eq!(
            other[1].blocks,
            [Block::Text("answer".into())],
            "no empty text reaches a model that refuses it"
        );
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
            message(
                Role::Assistant,
                vec![Part::Text { text: "ok".into() }, call(ToolStatus::Done, Some("1: x"))],
            ),
            message(Role::Assistant, vec![Part::Text { text: "done".into() }]),
        ];
        let out = messages(&transcript, &target());
        assert_eq!(out.len(), 4);
        assert_eq!(out[1].role, LlmRole::Assistant);
        assert!(matches!(&out[1].blocks[1], Block::ToolUse { id, .. } if id == "c1"));
        assert_eq!(
            out[2].blocks,
            vec![Block::ToolResult {
                call_id: "c1".into(),
                content: "1: x".into(),
                is_error: false
            }]
        );
        assert_eq!(out[3].blocks, vec![Block::Text("done".into())]);
    }

    #[test]
    fn returned_images_follow_every_result_of_the_turn() {
        let mut with_image = call(ToolStatus::Done, Some("an image"));
        if let Part::ToolCall { metadata, .. } = &mut with_image {
            *metadata = Some(Box::new(
                json!({ "images": [{ "mime": "image/png", "hash": "abc" }] }).into(),
            ));
        }
        let mut second = call(ToolStatus::Done, Some("text"));
        if let Part::ToolCall { call_id, .. } = &mut second {
            *call_id = "c2".into();
        }
        let out = messages(&[message(Role::Assistant, vec![with_image, second])], &target());
        let kinds: Vec<&str> = out[1]
            .blocks
            .iter()
            .map(|b| match b {
                Block::ToolResult { .. } => "result",
                Block::Text(_) => "text",
                Block::Stored { .. } => "image",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["result", "result", "text", "image"]);
    }

    #[test]
    fn unfinished_and_denied_calls_still_produce_results() {
        let transcript = vec![
            message(Role::Assistant, vec![call(ToolStatus::Running, None)]),
            message(Role::Assistant, vec![call(ToolStatus::Denied, None)]),
        ];
        let out = messages(&transcript, &target());
        let Block::ToolResult { is_error, content, .. } = &out[1].blocks[0] else {
            panic!()
        };
        assert!(*is_error && content.contains("interrupted"));
        assert!(matches!(&out[3].blocks[0], Block::ToolResult { content, .. } if content.contains("denied")));
    }

    #[test]
    fn signed_reasoning_goes_back_signed_only_to_the_model_that_made_it_and_as_text_to_another() {
        let signed = Part::Reasoning {
            text: "hm".into(),
            signature: Some("sig".into()),
            redacted: None,
        };
        let transcript = vec![
            message(Role::User, vec![Part::Text { text: "a".into() }]),
            message(Role::Assistant, vec![signed, Part::Text { text: "x".into() }]),
        ];
        assert!(matches!(
            &messages(&transcript, &target())[1].blocks[0],
            Block::Reasoning { signature: Some(_), .. }
        ));
        let other = ModelRef {
            provider: "openai".into(),
            model: "gpt-5".into(),
        };
        assert_eq!(
            messages(&transcript, &other)[1].blocks,
            vec![Block::Text("hm".into()), Block::Text("x".into())],
            "read, not checked"
        );
        let cut_off = vec![message_with(
            Role::Assistant,
            MessageStatus::Aborted,
            vec![Part::Reasoning {
                text: "whole".into(),
                signature: Some("sig".into()),
                redacted: None,
            }],
        )];
        assert_eq!(
            messages(&cut_off, &other)[0].blocks,
            vec![Block::Text("whole".into())],
            "a signed thought was whole before the reply was cut"
        );
        let hidden = vec![message(
            Role::Assistant,
            vec![
                Part::Reasoning {
                    text: String::new(),
                    signature: None,
                    redacted: Some("opaque".into()),
                },
                Part::Text { text: "x".into() },
            ],
        )];
        assert_eq!(
            messages(&hidden, &other)[0].blocks,
            vec![Block::Text("x".into())],
            "a redacted thought has nothing to read"
        );
    }

    #[test]
    fn a_mode_and_its_base_take_each_others_signed_reasoning() {
        let catalog = crate::llm::catalog::Catalog::bundled();
        let base = ModelRef {
            provider: "anthropic".into(),
            model: "claude-opus-5-5".into(),
        };
        let fast = ModelRef {
            provider: "anthropic".into(),
            model: "claude-opus-5-5-fast".into(),
        };
        let mut reply = message(
            Role::Assistant,
            vec![Part::Reasoning {
                text: "hm".into(),
                signature: Some("sig".into()),
                redacted: None,
            }],
        );
        reply.info.model = Some(fast.clone());
        let mut out = Vec::new();
        append(
            &mut out,
            [&reply],
            &OnCatalog {
                model: &base,
                catalog: &catalog,
            },
        );
        assert!(
            matches!(&out[0].blocks[0], Block::Reasoning { signature: Some(_), .. }),
            "the same model, run fast"
        );
        let mut sibling = Vec::new();
        append(
            &mut sibling,
            [&reply],
            &OnCatalog {
                model: &ModelRef {
                    provider: "anthropic".into(),
                    model: "claude-opus-5".into(),
                },
                catalog: &catalog,
            },
        );
        assert_eq!(
            sibling[0].blocks,
            vec![Block::Text("hm".into())],
            "another model in the family reads it as text"
        );
    }

    #[test]
    fn unsigned_reasoning_from_turns_already_over_is_dropped_wherever_the_new_prompt_lands() {
        let thought = |id: &str| {
            let mut reply = message(
                Role::Assistant,
                vec![
                    Part::Reasoning {
                        text: format!("thought {id}"),
                        signature: None,
                        redacted: None,
                    },
                    Part::Reasoning {
                        text: "signed".into(),
                        signature: Some("s".into()),
                        redacted: None,
                    },
                ],
            );
            reply.info.id = id.into();
            reply
        };
        let mut prompt = message(
            Role::User,
            vec![Part::Text {
                text: "after a stop".into(),
            }],
        );
        prompt.info.id = "msg_2".into();
        let mut transcript = vec![thought("msg_1"), prompt, thought("msg_3")];
        drop_earlier_reasoning(&mut transcript, Some("msg_2"));
        let kept = |m: &MessageWithParts| {
            m.parts
                .iter()
                .filter(|p| matches!(&p.part, Part::Reasoning { signature: None, .. }))
                .count()
        };
        assert_eq!(
            (kept(&transcript[0]), kept(&transcript[2])),
            (0, 1),
            "the stopped turn's unsigned thought goes; this turn's stays"
        );
        assert_eq!(
            transcript[0].parts.len(),
            1,
            "signed reasoning is the adapter's to judge, never dropped here"
        );
    }

    #[test]
    fn unsigned_reasoning_goes_back_only_from_a_finished_reply_and_adjacent_users_merge() {
        let thought = || Part::Reasoning {
            text: "hm".into(),
            signature: None,
            redacted: None,
        };
        let transcript = vec![
            message(Role::User, vec![Part::Text { text: "a".into() }]),
            message(Role::User, vec![Part::Text { text: "b".into() }]),
            message(Role::Assistant, vec![thought(), Part::Text { text: "x".into() }]),
        ];
        let out = messages(&transcript, &target());
        assert_eq!(out[0].blocks, vec![Block::Text("a".into()), Block::Text("b".into())]);
        assert_eq!(
            out[1].blocks,
            vec![
                Block::Reasoning {
                    text: "hm".into(),
                    signature: None,
                    redacted: None
                },
                Block::Text("x".into())
            ],
            "for wires that take reasoning_content"
        );
        let other = ModelRef {
            provider: "openai".into(),
            model: "gpt-5".into(),
        };
        assert_eq!(
            messages(&transcript, &other)[1].blocks,
            vec![Block::Text("hm".into()), Block::Text("x".into())],
            "another model reads it as text"
        );
        let aborted = vec![message_with(
            Role::Assistant,
            MessageStatus::Aborted,
            vec![thought(), Part::Text { text: "x".into() }],
        )];
        assert_eq!(
            messages(&aborted, &target())[0].blocks,
            vec![Block::Text("x".into())],
            "a cut-off thought is not replayed"
        );
        assert_eq!(
            messages(&aborted, &other)[0].blocks,
            vec![Block::Text("x".into())],
            "not even as text"
        );
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
            message_with(
                Role::Assistant,
                MessageStatus::Error,
                vec![Part::Text { text: "half".into() }],
            ),
            message_with(
                Role::Assistant,
                MessageStatus::Streaming,
                vec![Part::Text { text: "ghost".into() }],
            ),
            message_with(
                Role::Assistant,
                MessageStatus::Aborted,
                vec![Part::Text { text: "partial".into() }],
            ),
            message_with(
                Role::Assistant,
                MessageStatus::Done,
                vec![Part::Text { text: "final".into() }],
            ),
        ];
        let out = messages(&transcript, &target());
        let texts: Vec<String> = out
            .iter()
            .flat_map(|m| m.blocks.iter())
            .filter_map(|b| match b {
                Block::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
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
                Part::Reasoning {
                    text: "cut off".into(),
                    signature: None,
                    redacted: None,
                },
                Part::Text { text: "partial".into() },
                broken,
            ],
        )];
        let out = messages(&transcript, &target());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].blocks, vec![Block::Text("partial".into())]);
    }

    #[test]
    fn a_settled_call_with_broken_arguments_goes_back_with_its_parse_error() {
        let refused = Part::ToolCall {
            call_id: "c_bad".into(),
            name: "read".into(),
            input: serde_json::Value::String("{\"path".into()),
            status: ToolStatus::Error,
            title: None,
            output: Some("The arguments were not valid JSON (EOF while parsing)".into()),
            metadata: None,
            started_at: None,
            finished_at: None,
        };
        let out = messages(
            &[message_with(Role::Assistant, MessageStatus::Done, vec![refused])],
            &target(),
        );
        assert_eq!(
            out[0].blocks,
            vec![Block::ToolUse {
                id: "c_bad".into(),
                name: "read".into(),
                input: serde_json::json!({})
            }]
        );
        assert!(
            matches!(&out[1].blocks[0], Block::ToolResult { call_id, content, is_error: true } if call_id == "c_bad" && content.contains("not valid JSON"))
        );
    }
}
