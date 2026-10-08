//! Naming new conversations with the `title` action's model, in the background.

use std::sync::Arc;
use std::time::Duration;

use super::oneshot::{Fallback, OneShot};
use super::types::{Part, Role, Session};
use crate::Engine;
use crate::event::Event;
use crate::llm::{Block, ChatMessage};

const PLACEHOLDER_CHARS: usize = 80;
const TITLE_CHARS: usize = 60;
const INPUT_CHARS: usize = 4_000;
// The title itself; a reasoning model gets thinking room on top (`Engine::complete`).
const TITLE_MAX_TOKENS: u32 = 1_024;
const TITLE_TIMEOUT: Duration = Duration::from_secs(30);

impl Engine {
    /// Names an untitled session from its first message: the text itself at once, the model's title
    /// when it arrives. A rename in the meantime wins; any failure keeps the text.
    pub(super) fn title_untitled(self: &Arc<Self>, session: &Session) {
        if !session.title.is_empty() {
            return;
        }
        let Some(text) = self.first_prompt(&session.id) else {
            return;
        };
        let placeholder: String = text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(PLACEHOLDER_CHARS)
            .collect();
        self.rename_if(&session.id, "", &placeholder);
        let engine = self.clone();
        let id = session.id.clone();
        tokio::spawn(async move {
            if let Some(title) = engine.model_title(&id, &text).await {
                engine.rename_if(&id, &placeholder, &title);
            }
        });
    }

    async fn model_title(&self, session_id: &str, text: &str) -> Option<String> {
        let action = self.action_model(session_id, "title", Fallback::Small).await.ok()?;
        let system = action
            .config
            .agent("title")
            .map(|agent| agent.prompt.clone())
            .unwrap_or_default();
        let message = ChatMessage {
            role: crate::llm::Role::User,
            blocks: vec![Block::Text(text.chars().take(INPUT_CHARS).collect())],
        };
        let shot = OneShot {
            system,
            messages: vec![message],
            tools: Vec::new(),
            max_tokens: TITLE_MAX_TOKENS,
            timeout: TITLE_TIMEOUT,
            shown_in: None,
        };
        clean(&self.complete(&action.resolved, shot).await.ok()?.text)
    }

    fn first_prompt(&self, session_id: &str) -> Option<String> {
        let transcript = self.store.transcript(session_id).ok()?;
        let first = transcript.iter().find(|m| m.info.role == Role::User)?;
        first.parts.iter().find_map(|row| match &row.part {
            Part::Text { text } if !text.trim().is_empty() => Some(text.clone()),
            _ => None,
        })
    }

    fn rename_if(&self, session_id: &str, expected: &str, title: &str) {
        if let Ok(Some(updated)) = self.store.retitle_if(session_id, expected, title) {
            self.hub.publish(Event::SessionUpdated { session: updated });
        }
    }
}

/// First non-empty line, without quotes, a `Title:` label or trailing punctuation.
fn clean(reply: &str) -> Option<String> {
    let line = reply.lines().map(str::trim).find(|line| !line.is_empty())?;
    let line = line.strip_prefix("Title:").unwrap_or(line).trim();
    let line = line
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '*' | '#'))
        .trim_end_matches(['.', '!', '?', ':'])
        .trim();
    (!line.is_empty()).then(|| line.chars().take(TITLE_CHARS).collect())
}

#[cfg(test)]
mod tests;
