//! Requests the engine makes for itself, outside any turn: titles and compaction summaries.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;

use super::turn::{Prompt, TurnError};
use super::types::ModelRef;
use crate::llm::catalog::Model;
use crate::llm::{ChatMessage, Chunk, Credential, Provider, Request, StopReason, ToolSpec};
use crate::config::Config;
use crate::Engine;

/// A model ready to call: catalog entry, wire adapter and a usable credential.
pub(crate) struct Resolved {
    pub model_ref: ModelRef,
    pub model: Model,
    pub provider: Provider,
    pub credential: Credential,
}

/// One text-only request. Tools are defined only so a history with tool calls stays valid; calling one is forbidden.
pub(crate) struct OneShot {
    pub system: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: u32,
    pub timeout: Duration,
}

/// What an action uses when the user has not pinned a model for it.
pub(crate) enum Fallback {
    Conversation,
    /// A small model from the conversation's provider.
    Small,
}

impl Engine {
    /// The model an action runs on: the user's pin for that action in Settings, else its fallback.
    /// Returns the workspace config too, for the action's prompt.
    pub(crate) async fn action_model(&self, session_id: &str, action: &str, fallback: Fallback) -> Result<(Resolved, Config), TurnError> {
        let plan = self.plan(session_id, &Prompt { parts: Vec::new(), model: None, variant: None, agent: None, submission_id: None }).await?;
        if let Some(agent) = plan.config.agent(action) {
            agent.usable().map_err(TurnError::Config)?;
        }
        let chosen = match (plan.config.agent_model(action), fallback) {
            (Some(pinned), _) => pinned,
            (None, Fallback::Conversation) => plan.model_ref.clone(),
            (None, Fallback::Small) => self.catalog.read().unwrap().small_model(&plan.model_ref).unwrap_or_else(|| plan.model_ref.clone()),
        };
        let resolved = if chosen == plan.model_ref {
            Resolved { model_ref: plan.model_ref, model: plan.model, provider: plan.provider, credential: plan.credential }
        } else {
            self.resolve(&chosen).await?
        };
        Ok((resolved, Arc::unwrap_or_clone(plan.config)))
    }
    /// Everything needed to call `model_ref`, with an expired subscription token refreshed.
    pub(crate) async fn resolve(&self, model_ref: &ModelRef) -> Result<Resolved, TurnError> {
        let catalog = self.catalog.read().unwrap().clone();
        self.resolve_from(model_ref, &catalog).await
    }

    pub(super) async fn resolve_from(&self, model_ref: &ModelRef, catalog: &crate::llm::catalog::Catalog) -> Result<Resolved, TurnError> {
        let (model, env, api) = {
            let info = catalog.providers.get(&model_ref.provider).ok_or(TurnError::UnknownModel)?;
            (info.models.get(&model_ref.model).cloned().ok_or(TurnError::UnknownModel)?, info.env.clone(), info.api.clone())
        };
        let credential = self.credentials.resolve(&model_ref.provider, &env).ok_or(TurnError::NoCredentials)?;
        refuse_signin_elsewhere(&model_ref.provider, &credential, api.as_deref())?;
        let credential = self.fresh_credential(&model_ref.provider, credential).await?;
        let provider = self.provider_for(&model_ref.provider, api.as_deref()).ok_or(TurnError::UnknownModel)?;
        Ok(Resolved { model_ref: model_ref.clone(), model, provider, credential })
    }

    /// The reply's text. The request forbids tool calls; a reply that makes one anyway is refused, never run.
    pub(crate) async fn complete(&self, resolved: &Resolved, shot: OneShot) -> Result<String, String> {
        let request = Request {
            model: resolved.model_ref.model.clone(),
            system: shot.system,
            messages: crate::llm::prepare_files(shot.messages, &resolved.model, |hash| self.store.blob(hash).ok().flatten()),
            tools: shot.tools,
            max_tokens: shot.max_tokens,
            reasoning: None,
            temperature: None,
            cache_key: None,
            no_tool_calls: true,
        };
        tokio::time::timeout(shot.timeout, collect_text(&resolved.provider, &request, &resolved.credential))
            .await
            .map_err(|_| "the model took too long to answer".to_string())?
    }
}

/// A subscription sign-in goes only to its own vendor: on a route the user pointed at a gateway, the
/// token and the identity headers sent with it would go to that gateway.
pub(super) fn refuse_signin_elsewhere(provider: &str, credential: &Credential, api: Option<&str>) -> Result<(), TurnError> {
    match (credential, api) {
        (Credential::OAuth { .. }, Some(base)) => Err(TurnError::Config(format!(
            "{provider} is pointed at {base} in your drift.json, and a subscription sign-in is only sent to {provider} itself; use an API key for that route, or remove its baseUrl"
        ))),
        _ => Ok(()),
    }
}

async fn collect_text(provider: &Provider, request: &Request, credential: &Credential) -> Result<String, String> {
    let mut chunks = provider.stream(request, credential).await.map_err(|e| e.to_string())?;
    let mut text = String::new();
    let mut stopped = None;
    let mut called_tool = false;
    while let Some(chunk) = chunks.next().await {
        match chunk.map_err(|e| e.to_string())? {
            Chunk::TextDelta(delta) => text.push_str(&delta),
            Chunk::Stop(reason) => stopped = Some(reason),
            Chunk::ToolUseStart { .. } => called_tool = true,
            _ => {}
        }
    }
    match stopped {
        Some(StopReason::EndTurn) if !called_tool => {},
        Some(StopReason::MaxTokens) => return Err("the model hit its output limit; the incomplete reply was discarded".into()),
        Some(StopReason::Refused) => return Err("the model refused the request; its partial reply was discarded".into()),
        Some(StopReason::ContextFull) => return Err("the reply exhausted its context window; its partial text was discarded".into()),
        Some(_) => return Err("the model did not complete the text-only request normally".into()),
        None => return Err("the stream ended without a terminal reason".into()),
    }
    if text.trim().is_empty() {
        return Err("the model returned no text".into());
    }
    Ok(text)
}
