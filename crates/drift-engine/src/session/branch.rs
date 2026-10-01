//! Branching: a new conversation with its own goal, seeded from a reviewed summary of its source.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{compaction, convert};
use super::oneshot::{Fallback, OneShot};
use super::turn::{Prompt, TurnError};
use super::types::{MessageStatus, Part, Session, Visibility};
use crate::event::Event;
use crate::llm::{self, Block};
use crate::store::NewSession;
use crate::Engine;

const DRAFT_TIMEOUT: Duration = Duration::from_secs(120);
const DRAFT_MAX_TOKENS: u32 = 4096;
const TITLE_WORDS: usize = 6;

/// What the user reviews before a branch exists. Nothing is stored until they confirm.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BranchDraft {
    pub goal: String,
    pub title: String,
    pub summary: String,
    pub excerpts: String,
    /// The last source message the summary covers; absent for a conversation with no finished replies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cutoff: Option<String>,
}

#[derive(Debug)]
pub enum BranchError {
    NoSession,
    /// Subagents are workers on their parent's goal; only a conversation can branch.
    FromSubagent,
    BadCutoff,
    EmptyGoal,
    Turn(TurnError),
    Draft(String),
    Store(rusqlite::Error),
}

impl From<rusqlite::Error> for BranchError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl Engine {
    /// Asks the source's model for a handoff. One request, no tools run, nothing written.
    pub async fn draft_branch(self: &Arc<Self>, source_id: &str, goal: &str) -> Result<BranchDraft, BranchError> {
        let goal = goal.trim();
        if goal.is_empty() {
            return Err(BranchError::EmptyGoal);
        }
        let source = self.branchable(source_id)?;
        let (resolved, config) = self.action_model(source_id, "handoff", Fallback::Conversation).await.map_err(BranchError::Turn)?;
        let instructions = config.agent("handoff").map(|agent| agent.prompt.clone()).unwrap_or_default();
        let mut transcript = self.store.transcript(source_id)?;
        // What an undo hid is not part of the conversation being handed off.
        if let Some(revert) = &source.revert {
            transcript.retain(|m| m.info.id < revert.message_id);
        }
        let end = transcript.iter().rposition(|m| m.info.status == MessageStatus::Done);
        let cutoff = end.map(|i| transcript[i].info.id.clone());
        // The same view a turn would send: a compacted conversation hands off from its summary.
        let mut messages = compaction::request_messages(&transcript[..end.map_or(0, |i| i + 1)], &resolved.model_ref);
        convert::push(&mut messages, llm::Role::User, vec![Block::Text(format!("{instructions}\n\nGoal for the new conversation:\n{goal}"))]);
        let shot = OneShot { system: String::new(), messages, tools: self.tool_specs(resolved.model.profile), max_tokens: DRAFT_MAX_TOKENS, timeout: DRAFT_TIMEOUT };
        let text = self.complete(&resolved, shot).await.map_err(BranchError::Draft)?;
        Ok(parse_draft(goal, &text, cutoff))
    }

    /// Creates the branch from a reviewed draft and starts it. It shares nothing with its source after this.
    pub async fn branch(self: &Arc<Self>, source_id: &str, draft: BranchDraft) -> Result<Session, BranchError> {
        let goal = draft.goal.trim();
        if goal.is_empty() {
            return Err(BranchError::EmptyGoal);
        }
        let source = self.branchable(source_id)?;
        if let Some(cutoff) = &draft.cutoff {
            let owned = self.store.message(cutoff)?.is_some_and(|m| m.session_id == source.id);
            if !owned {
                return Err(BranchError::BadCutoff);
            }
        }
        let title = if draft.title.trim().is_empty() { title_from(goal) } else { draft.title.trim().to_string() };
        let session = self.store.create_branch(
            NewSession {
                workspace_id: &source.workspace_id,
                parent_id: Some(&source.id),
                visibility: Visibility::Sibling,
                title: &title,
                agent: &source.agent,
                model: source.model.as_ref(),
            },
            draft.cutoff.as_deref(),
        )?;
        self.hub.publish(Event::SessionCreated { session: session.clone() });
        let prompt = Prompt { parts: vec![Part::Text { text: seed(goal, &draft) }], model: source.model.clone(), thinking_budget: None, submission_id: None };
        self.submit(&session.id, prompt).await.map_err(BranchError::Turn)?;
        Ok(session)
    }

    fn branchable(&self, source_id: &str) -> Result<Session, BranchError> {
        let source = self.store.session(source_id)?.ok_or(BranchError::NoSession)?;
        if source.visibility == Visibility::Hidden {
            return Err(BranchError::FromSubagent);
        }
        Ok(source)
    }
}

/// Reads the `TITLE:` / `SUMMARY:` / `EXCERPTS:` sections; anything unlabelled becomes the summary.
fn parse_draft(goal: &str, text: &str, cutoff: Option<String>) -> BranchDraft {
    let mut title = String::new();
    let mut sections: [Vec<&str>; 2] = [Vec::new(), Vec::new()];
    let mut current: Option<usize> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("TITLE:") {
            title = rest.trim().to_string();
            current = None;
        } else if let Some(rest) = trimmed.strip_prefix("SUMMARY:") {
            current = Some(0);
            sections[0].push(rest);
        } else if let Some(rest) = trimmed.strip_prefix("EXCERPTS:") {
            current = Some(1);
            sections[1].push(rest);
        } else if let Some(index) = current {
            sections[index].push(line);
        }
    }
    let [summary, excerpts] = sections.map(|lines| lines.join("\n").trim().to_string());
    let summary = if summary.is_empty() && excerpts.is_empty() { text.trim().to_string() } else { summary };
    let excerpts = if excerpts.eq_ignore_ascii_case("none") { String::new() } else { excerpts };
    BranchDraft { goal: goal.into(), title: if title.is_empty() { title_from(goal) } else { title }, summary, excerpts, cutoff }
}

fn title_from(goal: &str) -> String {
    goal.split_whitespace().take(TITLE_WORDS).collect::<Vec<_>>().join(" ")
}

fn seed(goal: &str, draft: &BranchDraft) -> String {
    let mut text = String::new();
    if !draft.summary.trim().is_empty() {
        text.push_str(&format!("# Context carried from another conversation\n\n{}\n\n", draft.summary.trim()));
    }
    if !draft.excerpts.trim().is_empty() {
        text.push_str(&format!("## Excerpts\n\n{}\n\n", draft.excerpts.trim()));
    }
    text.push_str(&format!("# Goal\n\n{goal}"));
    text
}

#[cfg(test)]
mod tests;
