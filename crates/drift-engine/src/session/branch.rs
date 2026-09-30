//! Branching: a new conversation with its own goal, seeded from a reviewed summary of its source.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::convert;
use super::turn::{Prompt, TurnError};
use super::types::{MessageStatus, Part, Session, Visibility};
use crate::event::Event;
use crate::llm::{self, Block, ChatMessage, Chunk, Request};
use crate::store::NewSession;
use crate::Engine;

const DRAFT_TIMEOUT: Duration = Duration::from_secs(120);
const DRAFT_MAX_TOKENS: u32 = 4096;
const TITLE_WORDS: usize = 6;

const INSTRUCTIONS: &str = include_str!("prompts/branch.txt");

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
        self.branchable(source_id)?;
        let plan = self.plan(source_id, &Prompt { parts: Vec::new(), model: None, thinking_budget: None, submission_id: None }).await.map_err(BranchError::Turn)?;
        let transcript = self.store.transcript(source_id)?;
        let end = transcript.iter().rposition(|m| m.info.status == MessageStatus::Done);
        let cutoff = end.map(|i| transcript[i].info.id.clone());
        let mut messages = convert::messages(&transcript[..end.map_or(0, |i| i + 1)]);
        push_user_text(&mut messages, format!("{INSTRUCTIONS}\n\nGoal for the new conversation:\n{goal}"));
        let request = Request {
            model: plan.model_ref.model.clone(),
            system: String::new(),
            messages,
            tools: self.tools.specs(plan.model.profile),
            max_tokens: DRAFT_MAX_TOKENS,
            thinking_budget: None,
            temperature: None,
        };
        let text = tokio::time::timeout(DRAFT_TIMEOUT, collect_text(&plan.provider, &request, &plan.credential))
            .await
            .map_err(|_| BranchError::Draft("the model took too long to draft the handoff".into()))??;
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

async fn collect_text(provider: &llm::Provider, request: &Request, credential: &llm::Credential) -> Result<String, BranchError> {
    let mut chunks = provider.stream(request, credential).await.map_err(|e| BranchError::Draft(e.to_string()))?;
    let mut text = String::new();
    let mut stopped = false;
    while let Some(chunk) = chunks.next().await {
        match chunk.map_err(|e| BranchError::Draft(e.to_string()))? {
            Chunk::TextDelta(delta) => text.push_str(&delta),
            Chunk::Stop(_) => stopped = true,
            _ => {}
        }
    }
    if !stopped || text.trim().is_empty() {
        return Err(BranchError::Draft("the model returned no handoff".into()));
    }
    Ok(text)
}

/// The handoff rides on the last user turn when one is open, so roles still alternate.
fn push_user_text(messages: &mut Vec<ChatMessage>, text: String) {
    match messages.last_mut() {
        Some(last) if last.role == llm::Role::User => last.blocks.push(Block::Text(text)),
        _ => messages.push(ChatMessage { role: llm::Role::User, blocks: vec![Block::Text(text)] }),
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
