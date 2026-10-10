//! Stored transcript to model messages. Tool results become the user turn that follows each call.

use crate::llm::catalog::Catalog;
use crate::llm::{Block, ChatMessage, Role as LlmRole};
use crate::session::types::{MessageStatus, MessageWithParts, ModelRef, Part, Role, ToolStatus};

/// The model a request's history is for: reasoning signatures validate only with the model and account that made them.
pub(crate) trait Target {
    fn wrote(&self, model: &ModelRef, account: Option<&str>) -> bool;
}

/// This entry or any that runs the same model, a mode and its base (`Catalog::same_model`), sent by the same account.
pub(crate) struct OnCatalog<'a> {
    pub model: &'a ModelRef,
    pub catalog: &'a Catalog,
    pub account: Option<&'a str>,
}

/// Exactly this entry.
impl Target for ModelRef {
    fn wrote(&self, model: &ModelRef, _account: Option<&str>) -> bool {
        self == model
    }
}

impl Target for OnCatalog<'_> {
    fn wrote(&self, model: &ModelRef, account: Option<&str>) -> bool {
        // A reply from before accounts were recorded came from the provider's only account, stored under its name.
        let signer = |model: &ModelRef, account: Option<&str>| account.unwrap_or(&model.provider).to_string();

        self.catalog.same_model(self.model, model) && signer(model, account) == signer(self.model, self.account)
    }
}

/// Only what the provider actually finished is replayed; failed and still-streaming attempts are audit history.
fn replayable(message: &MessageWithParts) -> bool {
    message.info.role == Role::User || matches!(message.info.status, MessageStatus::Done | MessageStatus::Aborted)
}

/// Converts `transcript` onto `out`, merging with what is already there, for `target`.
pub(crate) fn append<'a>(
    out: &mut Vec<ChatMessage>,
    transcript: impl IntoIterator<Item = &'a MessageWithParts>,
    target: &impl Target,
) {
    let mut used = std::collections::HashSet::new();

    for message in transcript.into_iter().filter(|message| replayable(message)) {
        match message.info.role {
            Role::User => push(out, LlmRole::User, user_blocks(message)),
            Role::Assistant => {
                let account = message.info.account.as_deref();
                let same_model = message.info.model.as_ref().is_some_and(|model| target.wrote(model, account));
                let mut calls = assistant_blocks(message, same_model);
                let mut results = result_blocks(message);
                unique_ids(&mut used, &mut calls, &mut results);
                push(out, LlmRole::Assistant, calls);
                push(out, LlmRole::User, results);
            }
        }
    }
}

/// Older sessions may repeat ids that providers reject, such as Gemini's call_1 in every step.
/// Repeated ids are suffixed on the call and its corresponding result.
fn unique_ids(used: &mut std::collections::HashSet<String>, calls: &mut [Block], results: &mut [Block]) {
    let mut renamed = Vec::new();
    for block in calls.iter_mut() {
        let Block::ToolUse { id, .. } = unsigned_mut(block) else {
            continue;
        };
        if used.insert(id.clone()) {
            continue;
        }

        let fresh = (2..)
            .map(|index| format!("{id}_{index}"))
            .find(|candidate| !used.contains(candidate))
            .unwrap_or_default();
        used.insert(fresh.clone());
        renamed.push((std::mem::replace(id, fresh.clone()), fresh));
    }

    for block in results.iter_mut() {
        let Block::ToolResult { call_id, .. } = block else {
            continue;
        };
        if let Some(index) = renamed.iter().position(|(old, _)| old == call_id) {
            *call_id = renamed.remove(index).1;
        }
    }
}

/// Unwraps provider signatures while leaving the stored block and its signature intact.
fn unsigned_mut(block: &mut Block) -> &mut Block {
    match block {
        Block::Signed { part, .. } => unsigned_mut(part),
        other => other,
    }
}

/// Adjacent messages of one role merge, since providers reject two in a row.
pub(crate) fn push(out: &mut Vec<ChatMessage>, role: LlmRole, blocks: Vec<Block>) {
    if blocks.is_empty() {
        return;
    }

    match out.last_mut() {
        Some(last) if last.role == role => last.blocks.extend(blocks),
        _ => out.push(ChatMessage { role, blocks }),
    }
}

/// Maps user text, engine context and supported attachment parts into model blocks.
fn user_blocks(message: &MessageWithParts) -> Vec<Block> {
    message.parts.iter().filter_map(|row| match &row.part {
        Part::Text { text } | Part::Nudge { text } if !text.is_empty() => Some(Block::Text(text.clone())),
        Part::Context { plugin, text } => {
            let context = format!("<system-reminder>\nFrom the {plugin} plugin:\n{text}\n</system-reminder>");
            Some(Block::Text(context))
        }
        Part::File { mime, url, .. } if mime.starts_with("image/") => {
            url.split_once(',').map(|(_, data)| Block::Image {
                mime: mime.clone(),
                base64: data.to_string(),
            })
        }
        Part::File { mime, url, .. } if mime.starts_with("text/") => super::attach::data_text(url).map(Block::Text),
        Part::File { mime, url, .. } if mime.eq_ignore_ascii_case(crate::tool::image::PDF) => {
            url.split_once(',').map(|(_, data)| Block::Pdf { base64: data.to_string() })
        }
        Part::TaskResult { task_id, description, outcome, text, .. } => Some(Block::Text(format!(
            "<task-result id=\"{task_id}\" description=\"{description}\" outcome=\"{outcome}\">\n{text}\n</task-result>"
        ))),
        Part::Clarification { request_id, items } => Some(Block::Text(clarification_text(request_id, items))),
        _ => None,
    }).collect()
}

/// How the model reads an answer that arrives after it moved on.
pub(crate) fn clarification_text(request_id: &str, items: &[super::types::Clarified]) -> String {
    let answers: Vec<_> = items
        .iter()
        .map(|item| format!("{}\nAnswer: {}", item.question, item.answers.join(", ")))
        .collect();

    format!(
        "<question-answer id=\"{request_id}\">\n\
         The user answered the question you asked earlier.\n\n{}\n</question-answer>",
        answers.join("\n\n")
    )
}

/// Unsigned reasoning is replayed only within the turn that wrote it; earlier turns may be refused by providers.
/// A steered prompt and a prompt after Stop can both follow tool results, so `started` identifies the turn.
/// Provider-signed reasoning is retained for the adapter to validate.
pub(super) fn drop_earlier_reasoning(transcript: &mut [MessageWithParts], started: Option<&str>) {
    let Some(started) = started else { return };

    for message in transcript
        .iter_mut()
        .filter(|message| message.info.role == Role::Assistant && message.info.id.as_str() < started)
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

/// Signed or redacted reasoning is replayed only to the model that wrote it.
/// Unsigned reasoning requires a finished reply; another model reads finished thoughts as plain text.
/// Each provider adapter decides which reasoning fields its wire accepts.
fn assistant_blocks(message: &MessageWithParts, same_model: bool) -> Vec<Block> {
    let finished = message.info.status == MessageStatus::Done;

    message
        .parts
        .iter()
        .filter_map(|row| {
            let signed = row.provider_signature.is_some();
            let block = match &row.part {
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

/// A call with malformed arguments is replayed as {} after its parse error has been saved as a result.
/// A call cut off before a result was saved is omitted.
fn replayed_input(input: &serde_json::Value, status: ToolStatus, output: Option<&str>) -> Option<serde_json::Value> {
    match input {
        serde_json::Value::Object(_) => Some(input.clone()),
        _ if status == ToolStatus::Error && output.is_some() => Some(serde_json::json!({})),
        _ => None,
    }
}

/// Every replayed call needs a result or the provider rejects the transcript; unfinished ones say so.
/// Calls with malformed arguments that were not replayed get no result either.
/// Returned images follow all results, because providers require results first in the user turn.
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
            let ending = if is_error { "failed" } else { "returned" };
            results.push(Block::Text(format!("The /{command} command{ran} {ending}:\n{content}")));
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
mod tests;
