//! Secrets a plugin hides from the model. Every request leaves with each secret swapped for its
//! placeholder, and every tool call and reply the model makes has the placeholders swapped back, so
//! tools, files, MCP servers and the user see the real values while the model only ever sees placeholders.

use std::borrow::Cow;

use aho_corasick::{AhoCorasick, MatchKind};
use serde_json::Value;

use super::{Block, ChatMessage, Request, ToolSpec};

/// One direction of the swap: every key found, longest first, is replaced in a single pass.
struct Swap {
    finder: AhoCorasick,
    with: Vec<String>,
}

impl Swap {
    fn new(from: &[&str], to: Vec<String>) -> Option<Self> {
        let finder = AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostLongest)
            .build(from)
            .ok()?;

        Some(Self { finder, with: to })
    }

    fn apply<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if !self.finder.is_match(text) {
            return Cow::Borrowed(text);
        }

        Cow::Owned(self.finder.replace_all(text, &self.with))
    }

    fn apply_value(&self, value: &mut Value) {
        match value {
            Value::String(text) => {
                if let Cow::Owned(swapped) = self.apply(text) {
                    *text = swapped;
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|item| self.apply_value(item)),
            Value::Object(fields) => {
                let entries = std::mem::take(fields);
                for (key, mut item) in entries {
                    self.apply_value(&mut item);
                    fields.insert(self.apply(&key).into_owned(), item);
                }
            }
            _ => {}
        }
    }
}

#[derive(Default)]
pub struct Masks {
    hide: Option<Swap>,
    reveal: Option<Swap>,
    count: usize,
}

impl Masks {
    /// Pairs of secret and placeholder. An empty or unchanged pair, a repeated secret or placeholder (the model's
    /// placeholder could only ever become one of the secrets), and a placeholder that contains any secret (the model
    /// would read that secret inside it) are left out. Problems name a pair by its place, never by what it holds.
    pub fn new(pairs: Vec<(String, String)>) -> (Self, Vec<String>) {
        let mut kept: Vec<(usize, String, String)> = Vec::new();
        let mut problems = Vec::new();
        for (index, (secret, placeholder)) in pairs.into_iter().enumerate() {
            let number = index + 1;
            if secret.is_empty() || placeholder.is_empty() || secret == placeholder {
                problems.push(format!(
                    "pair {number} needs a secret and a different, non-empty placeholder"
                ));
            } else if kept.iter().any(|(_, known, _)| *known == secret) {
                problems.push(format!("pair {number} repeats a secret listed before it"));
            } else if kept.iter().any(|(_, _, known)| *known == placeholder) {
                problems.push(format!("pair {number} repeats a placeholder listed before it"));
            } else {
                kept.push((number, secret, placeholder));
            }
        }
        let secrets: Vec<String> = kept.iter().map(|(_, secret, _)| secret.clone()).collect();
        kept.retain(|(number, _, placeholder)| {
            let leaks = secrets.iter().any(|secret| placeholder.contains(secret.as_str()));
            if leaks {
                problems.push(format!(
                    "pair {number}'s placeholder contains a secret, so it was dropped"
                ));
            }
            !leaks
        });
        if kept.is_empty() {
            return (Self::default(), problems);
        }

        let secrets: Vec<&str> = kept.iter().map(|(_, secret, _)| secret.as_str()).collect();
        let placeholders: Vec<&str> = kept.iter().map(|(_, _, placeholder)| placeholder.as_str()).collect();
        let masks = Self {
            hide: Swap::new(&secrets, placeholders.iter().map(ToString::to_string).collect()),
            reveal: Swap::new(&placeholders, secrets.iter().map(ToString::to_string).collect()),
            count: kept.len(),
        };

        (masks, problems)
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn hide<'a>(&self, text: &'a str) -> Cow<'a, str> {
        self.hide.as_ref().map_or(Cow::Borrowed(text), |swap| swap.apply(text))
    }

    /// Text the model wrote as the user should read it: placeholders become the secrets.
    pub fn reveal_text<'a>(&self, text: &'a str) -> Cow<'a, str> {
        self.reveal
            .as_ref()
            .map_or(Cow::Borrowed(text), |swap| swap.apply(text))
    }

    /// A tool call's input as it should run: placeholders the model wrote become the secrets.
    pub fn reveal(&self, input: &mut Value) {
        if let Some(swap) = &self.reveal {
            swap.apply_value(input);
        }
    }

    /// The request as the model may see it. Signatures, redacted reasoning and file data are never touched.
    pub fn request<'a>(&self, request: &'a Request) -> Cow<'a, Request> {
        let Some(swap) = &self.hide else {
            return Cow::Borrowed(request);
        };

        let mut hidden = request.clone();
        hidden.system = swap.apply(&request.system).into_owned();
        hidden
            .messages
            .iter_mut()
            .for_each(|message| hide_message(swap, message));
        hidden.tools.iter_mut().for_each(|tool| hide_tool(swap, tool));

        Cow::Owned(hidden)
    }
}

fn hide_message(swap: &Swap, message: &mut ChatMessage) {
    message.blocks.iter_mut().for_each(|block| hide_block(swap, block));
}

fn hide_block(swap: &Swap, block: &mut Block) {
    let text = |text: &mut String| {
        if let Cow::Owned(hidden) = swap.apply(text) {
            *text = hidden;
        }
    };

    match block {
        Block::Signed { part, .. } => hide_block(swap, part),
        Block::Text(body)
        | Block::Reminder(body)
        | Block::Reasoning { text: body, .. }
        | Block::ToolResult { content: body, .. } => text(body),
        Block::ToolUse { input, .. } => swap.apply_value(input),
        Block::Image { .. } | Block::Pdf { .. } | Block::Stored { .. } | Block::Effort(_) => {}
    }
}

fn hide_tool(swap: &Swap, tool: &mut ToolSpec) {
    tool.description = swap.apply(&tool.description).into_owned();
    swap.apply_value(&mut tool.input_schema);
}

#[cfg(test)]
mod tests;
