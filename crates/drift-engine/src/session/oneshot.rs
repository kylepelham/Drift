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
    /// For each attempt; a provider fault is retried as a turn's is.
    pub timeout: Duration,
    /// The conversation's, when the request opens as its turns do, so the provider can reuse their cached prefix.
    pub cache_key: Option<String>,
}

/// An action's model, with what it needs to know of the conversation it acts on.
pub(crate) struct Action {
    pub resolved: Resolved,
    pub config: Config,
    /// The conversation's own model, which reads whatever the action leaves behind.
    pub conversation: Model,
    /// The conversation's system prompt and tools, when the action runs on the conversation's model.
    pub frame: Option<(String, Vec<ToolSpec>)>,
}

/// Why a one-shot failed: a provider fault worth retrying, or anything else.
enum Failure {
    Provider(crate::llm::Error),
    Reply(String),
}

/// Thinking room a reasoning model without a token budget gets on top of a one-shot answer.
const ONE_SHOT_THINKING: u32 = 4_096;

/// What an action uses when the user has not pinned a model for it.
pub(crate) enum Fallback {
    Conversation,
    /// A small model from the conversation's provider.
    Small,
}

impl Engine {
    /// The model an action runs on: the user's pin for that action in Settings, else its fallback.
    pub(crate) async fn action_model(&self, session_id: &str, action: &str, fallback: Fallback) -> Result<Action, TurnError> {
        let plan = self.plan(session_id, &Prompt { parts: Vec::new(), model: None, variant: None, agent: None, submission_id: None }).await?;
        if let Some(agent) = plan.config.agent(action) {
            agent.usable().map_err(TurnError::Config)?;
        }
        let chosen = match (plan.config.agent_model(action), fallback) {
            (Some(pinned), _) => pinned,
            (None, Fallback::Conversation) => plan.model_ref.clone(),
            (None, Fallback::Small) => plan
                .catalog
                .small_model(&plan.model_ref)
                .or_else(|| crate::llm::openai::codex::small_model(&plan.catalog, &plan.model_ref, &plan.credential))
                .unwrap_or_else(|| plan.model_ref.clone()),
        };
        let own = chosen == plan.model_ref;
        let frame = own.then(|| plan.frame());
        let conversation = plan.model.clone();
        let resolved = if own {
            Resolved { model_ref: plan.model_ref, model: plan.model, provider: plan.provider, credential: plan.credential }
        } else {
            self.resolve(&chosen).await?
        };
        Ok(Action { resolved, config: Arc::unwrap_or_clone(plan.config), conversation, frame })
    }
    /// Everything needed to call `model_ref`, with an expired subscription token refreshed.
    pub(crate) async fn resolve(&self, model_ref: &ModelRef) -> Result<Resolved, TurnError> {
        self.resolve_from(model_ref, &self.catalog_view()).await
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
    /// A reasoning model runs at its weakest level, with room to think on top of the answer's own.
    /// A provider fault (overload, rate limit, dropped connection) is retried with a turn's backoff.
    pub(crate) async fn complete(&self, resolved: &Resolved, shot: OneShot) -> Result<String, String> {
        let mut reasoning = resolved.model.variants.first().map(|variant| variant.reasoning.clone());
        let thinking = match &reasoning {
            Some(crate::llm::catalog::Reasoning::Budget { tokens }) => *tokens,
            _ if resolved.model.reasoning => ONE_SHOT_THINKING,
            _ => 0,
        };
        let limit = u32::try_from(resolved.model.limit.output).ok().filter(|limit| *limit > 0).unwrap_or(u32::MAX);
        let mut max_tokens = shot.max_tokens.saturating_add(thinking).min(limit);
        // A budget the model's output limit cannot hold beside the answer is dropped rather than refused by the provider.
        if matches!(reasoning, Some(crate::llm::catalog::Reasoning::Budget { tokens }) if tokens >= max_tokens) {
            reasoning = None;
            max_tokens = shot.max_tokens.min(limit);
        }
        let request = Request {
            model: resolved.model.wire(&resolved.model_ref.model).to_string(),
            system: shot.system,
            messages: crate::llm::prepare_files(shot.messages, &resolved.model, |hash| self.store.blob(hash).ok().flatten()),
            tools: shot.tools,
            max_tokens,
            reasoning,
            temperature: None,
            cache_key: shot.cache_key,
            no_tool_calls: true,
            verbosity: None,
            show_thinking: false,
            top_p: None,
            top_k: None,
            mode: resolved.model.mode.clone(),
        };
        let mut retries = 0;
        loop {
            let attempt = tokio::time::timeout(shot.timeout, collect_text(&resolved.provider, &request, &resolved.credential)).await;
            let error = match attempt.map_err(|_| "the model took too long to answer".to_string())? {
                Ok(text) => return Ok(text),
                Err(Failure::Reply(why)) => return Err(why),
                Err(Failure::Provider(error)) => error,
            };
            match super::turn::Retry::from(&error).filter(|retry| retry.allowed(retries)) {
                Some(retry) => {
                    retries += 1;
                    tokio::time::sleep(retry.delay(retries)).await;
                }
                None => return Err(error.to_string()),
            }
        }
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

async fn collect_text(provider: &Provider, request: &Request, credential: &Credential) -> Result<String, Failure> {
    let mut chunks = provider.stream(request, credential).await.map_err(Failure::Provider)?;
    let mut text = String::new();
    let mut stopped = None;
    let mut called_tool = false;
    while let Some(chunk) = chunks.next().await {
        match chunk.map_err(Failure::Provider)? {
            Chunk::TextDelta(delta) => text.push_str(&delta),
            Chunk::Stop(reason) => stopped = Some(reason),
            Chunk::ToolUseStart { .. } => called_tool = true,
            _ => {}
        }
    }
    let failed = |why: &str| Err(Failure::Reply(why.into()));
    match stopped {
        Some(StopReason::EndTurn) if !called_tool => {},
        Some(StopReason::MaxTokens) => return failed("the model hit its output limit; the incomplete reply was discarded"),
        Some(StopReason::Refused) => return failed("the model refused the request; its partial reply was discarded"),
        Some(StopReason::ContextFull) => return failed("the reply exhausted its context window; its partial text was discarded"),
        Some(_) => return failed("the model did not complete the text-only request normally"),
        None => return failed("the stream ended without a terminal reason"),
    }
    if text.trim().is_empty() {
        return failed("the model returned no text");
    }
    Ok(text)
}
