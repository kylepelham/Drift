//! Requests the engine makes for itself, outside any turn: titles and compaction summaries.

use std::time::Duration;

use futures_util::StreamExt;

use super::turn::{Plan, Prompt, TurnError};
use super::types::{ModelRef, Usage};
use crate::Engine;
use crate::config::Config;
use crate::event::{Event, SessionStatus};
use crate::llm::catalog::Model;
use crate::llm::{ChatMessage, Chunk, Credential, Provider, Request, StopReason, ToolSpec};

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
    /// The session whose user is waiting on this request and sees its retries, as a turn's; none for a background title.
    pub shown_in: Option<String>,
}

/// An action's model, with what it needs to know of the conversation it acts on.
pub(crate) struct Action {
    pub resolved: Resolved,
    pub config: Config,
    /// The conversation's own model, which reads whatever the action leaves behind.
    pub conversation: Model,
    /// The conversation's plan, when the action runs on its model, so a request can be built as its turns build theirs.
    pub own: Option<Plan>,
    /// The conversation's workspace, whose MCP tools its history may call.
    pub workspace: std::path::PathBuf,
}

/// A reply's text and the tokens it used, which are paid for.
pub(crate) struct Answer {
    pub text: String,
    pub usage: Usage,
}

pub(super) struct SendOptions<'a> {
    pub timeout: Duration,
    pub shown_in: Option<&'a str>,
}

/// Why a one-shot failed.
pub(crate) enum Failure {
    /// The provider refused or failed, after any retries.
    Provider(crate::llm::Error),
    /// The model answered, but not with a usable text reply; what it used is still paid for.
    Reply(String, Usage),
    Late,
}

impl Failure {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Provider(error) => error.to_string(),
            Self::Reply(why, _) => why.clone(),
            Self::Late => "the model took too long to answer".into(),
        }
    }

    /// The tokens a reply used before it was refused; none when no reply came.
    pub(crate) fn usage(&self) -> Usage {
        match self {
            Self::Reply(_, usage) => *usage,
            _ => Usage::default(),
        }
    }

    /// Worth asking again another way: the reply was unusable, or the request was too long.
    pub(super) fn retry_another_way(&self) -> bool {
        match self {
            Self::Reply(..) => true,
            Self::Provider(error) => crate::llm::mentions_context_overflow(&error.to_string()),
            Self::Late => false,
        }
    }
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
    pub(crate) async fn action_model(
        &self,
        session_id: &str,
        action: &str,
        fallback: Fallback,
    ) -> Result<Action, TurnError> {
        let plan = self
            .plan(
                session_id,
                &Prompt {
                    parts: Vec::new(),
                    model: None,
                    variant: None,
                    agent: None,
                    submission_id: None,
                },
            )
            .await?;
        if let Some(agent) = plan.config.agent(action) {
            agent.usable().map_err(|error| TurnError::Config(error.to_string()))?;
        }
        let chosen = match (plan.config.agent_model(action), fallback) {
            (Some(pinned), _) => pinned,
            (None, Fallback::Conversation) => plan.model_ref.clone(),
            // A ChatGPT sign-in pays in plan usage, not by the API prices the catalog shows, so its own pick wins.
            (None, Fallback::Small) => {
                crate::llm::openai::codex::small_model(&plan.catalog, &plan.model_ref, &plan.credential)
                    .or_else(|| plan.catalog.small_model(&plan.model_ref))
                    .unwrap_or_else(|| plan.model_ref.clone())
            }
        };
        let own = chosen == plan.model_ref;
        let resolved = if own {
            Resolved {
                model_ref: plan.model_ref.clone(),
                model: plan.model.clone(),
                provider: plan.provider.clone(),
                credential: plan.credential.clone(),
            }
        } else {
            self.resolve(&chosen).await?
        };
        Ok(Action {
            resolved,
            config: (*plan.config).clone(),
            conversation: plan.model.clone(),
            workspace: plan.workspace.clone(),
            own: own.then_some(plan),
        })
    }
    /// Everything needed to call `model_ref`, with an expired subscription token refreshed.
    pub(crate) async fn resolve(&self, model_ref: &ModelRef) -> Result<Resolved, TurnError> {
        self.resolve_from(model_ref, &self.catalog_view()).await
    }

    pub(super) async fn resolve_from(
        &self,
        model_ref: &ModelRef,
        catalog: &crate::llm::catalog::Catalog,
    ) -> Result<Resolved, TurnError> {
        let (model, env, api) = {
            let info = catalog
                .providers
                .get(&model_ref.provider)
                .ok_or(TurnError::UnknownModel)?;
            (
                info.models
                    .get(&model_ref.model)
                    .cloned()
                    .ok_or(TurnError::UnknownModel)?,
                info.env.clone(),
                info.api.clone(),
            )
        };
        let credential = self
            .credentials
            .resolve(&model_ref.provider, &env)
            .ok_or(TurnError::NoCredentials)?;
        refuse_signin_elsewhere(&model_ref.provider, &credential, api.as_deref())?;
        let credential = self.fresh_credential(&model_ref.provider, credential).await?;
        let provider = self
            .provider_for(&model_ref.provider, api.as_deref())
            .ok_or(TurnError::UnknownModel)?;
        Ok(Resolved {
            model_ref: model_ref.clone(),
            model,
            provider,
            credential,
        })
    }

    /// The reply and what it used. The request forbids tool calls; a reply that makes one anyway is refused, never run.
    /// A reasoning model runs at its weakest level, with room to think on top of the answer's own.
    /// A provider fault (overload, rate limit, dropped connection) is retried with a turn's backoff.
    pub(crate) async fn complete(&self, resolved: &Resolved, shot: OneShot) -> Result<Answer, Failure> {
        let mut reasoning = resolved.model.variants.first().map(|variant| variant.reasoning.clone());
        let thinking = match &reasoning {
            Some(crate::llm::catalog::Reasoning::Budget { tokens }) => *tokens,
            _ if resolved.model.reasoning => ONE_SHOT_THINKING,
            _ => 0,
        };
        let limit = u32::try_from(resolved.model.limit.output)
            .ok()
            .filter(|limit| *limit > 0)
            .unwrap_or(u32::MAX);
        let mut max_tokens = shot.max_tokens.saturating_add(thinking).min(limit);
        // A budget the model's output limit cannot hold beside the answer is dropped rather than refused by the provider.
        if matches!(reasoning, Some(crate::llm::catalog::Reasoning::Budget { tokens }) if tokens >= max_tokens) {
            reasoning = None;
            max_tokens = shot.max_tokens.min(limit);
        }
        let request = Request {
            model: resolved.model.wire(&resolved.model_ref.model).to_string(),
            system: shot.system,
            messages: crate::llm::prepare_files(shot.messages, &resolved.model, |hash| {
                self.store.blob(hash).ok().flatten()
            }),
            tools: shot.tools,
            max_tokens,
            reasoning,
            temperature: None,
            cache_key: None,
            no_tool_calls: true,
            verbosity: None,
            show_thinking: false,
            top_p: None,
            top_k: None,
            mode: resolved.model.mode.clone(),
        };
        self.send(
            &resolved.provider,
            &resolved.credential,
            &request,
            SendOptions {
                timeout: shot.timeout,
                shown_in: shot.shown_in.as_deref(),
            },
        )
        .await
    }

    /// The reply to `request` and what it used. A provider fault (overload, rate limit, dropped connection)
    /// is retried with a turn's backoff and limits, announced in `shown_in` as a turn announces its own.
    pub(super) async fn send(
        &self,
        provider: &Provider,
        credential: &Credential,
        request: &Request,
        options: SendOptions<'_>,
    ) -> Result<Answer, Failure> {
        let SendOptions { timeout, shown_in } = options;
        let mut retries = 0;
        loop {
            let attempt = tokio::time::timeout(timeout, collect_text(provider, request, credential))
                .await
                .map_err(|_| Failure::Late)?;
            let error = match attempt {
                Ok(answer) => return Ok(answer),
                Err(Failure::Provider(error)) => error,
                Err(other) => return Err(other),
            };
            let Some(retry) = super::turn::Retry::from(&error).filter(|retry| retry.allowed(retries)) else {
                return Err(Failure::Provider(error));
            };
            retries += 1;
            let delay = retry.delay(retries);
            if let Some(session_id) = shown_in {
                let next_at = crate::id::now_ms() + i64::try_from(delay.as_millis()).unwrap_or(i64::MAX);
                self.hub.publish(Event::SessionRetry {
                    session_id: session_id.into(),
                    attempt: retries,
                    message: retry.message().into(),
                    next_at,
                });
            }
            tokio::time::sleep(delay).await;
            if let Some(session_id) = shown_in {
                self.hub.publish(Event::SessionStatusChanged {
                    session_id: session_id.into(),
                    status: SessionStatus::Running,
                });
            }
        }
    }
}

/// A subscription sign-in goes only to its own vendor: on a route the user pointed at a gateway, the
/// token and the identity headers sent with it would go to that gateway.
pub(super) fn refuse_signin_elsewhere(
    provider: &str,
    credential: &Credential,
    api: Option<&str>,
) -> Result<(), TurnError> {
    match (credential, api) {
        (Credential::OAuth { .. }, Some(base)) => Err(TurnError::Config(format!(
            "{provider} is pointed at {base} in your drift.json, and a subscription sign-in is only sent to {provider} itself; use an API key for that route, or remove its baseUrl"
        ))),
        _ => Ok(()),
    }
}

async fn collect_text(provider: &Provider, request: &Request, credential: &Credential) -> Result<Answer, Failure> {
    let mut chunks = provider.stream(request, credential).await.map_err(Failure::Provider)?;
    let mut text = String::new();
    let mut usage = Usage::default();
    let mut stopped = None;
    let mut called_tool = false;
    while let Some(chunk) = chunks.next().await {
        match chunk.map_err(Failure::Provider)? {
            Chunk::TextDelta(delta) => text.push_str(&delta),
            Chunk::Usage(reported) => usage.merge(reported),
            Chunk::Stop(reason) => stopped = Some(reason),
            Chunk::ToolUseStart { .. } => called_tool = true,
            _ => {}
        }
    }
    let failed = |why: &str| Err(Failure::Reply(why.into(), usage));
    match stopped {
        Some(StopReason::EndTurn) if !called_tool => {}
        Some(StopReason::MaxTokens) => {
            return failed("the model hit its output limit; the incomplete reply was discarded");
        }
        Some(StopReason::Refused) => return failed("the model refused the request; its partial reply was discarded"),
        Some(StopReason::ContextFull) => {
            return failed("the reply exhausted its context window; its partial text was discarded");
        }
        Some(_) => return failed("the model did not complete the text-only request normally"),
        None => return failed("the stream ended without a terminal reason"),
    }
    if text.trim().is_empty() {
        return failed("the model returned no text");
    }
    Ok(Answer { text, usage })
}
