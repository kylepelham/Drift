use super::*;

/// Output cap when the model allows more; keeps a runaway response from burning the budget.
pub(super) const MAX_OUTPUT_TOKENS: u32 = 32_000;
/// What a thinking budget always leaves for the answer itself.
pub(super) const MIN_ANSWER_TOKENS: u32 = 1024;
/// The smallest thinking budget providers accept.
pub(super) const MIN_THINKING_TOKENS: u32 = 1024;
/// How long a provider keeps a prompt cache it was last asked for: five minutes, or an hour when asked for that.
const CACHE_MS: i64 = 5 * 60 * 1000;
const HOUR_CACHE_MS: i64 = 60 * 60 * 1000;

impl Engine {
    /// Builds the same request as a turn step, including its cached prefix and newest prompt id.
    /// A closing instruction is the last user message and disables tool calls.
    /// Compaction uses this request shape when summarising on the conversation's model.
    pub(in crate::session) fn step_request(
        &self,
        plan: &Plan,
        mut transcript: Vec<MessageWithParts>,
        started: Option<&str>,
        closing: Option<String>,
    ) -> (Request, Option<String>) {
        branch::frame_spawned(&plan.session, &mut transcript);
        let mut lead = prompt::remind_agents(&plan.config, &plan.session.agent, &mut transcript);
        lead.extend(self.plans_note(&plan.session.workspace_id));
        let system = self.instruction_system(plan, &mut transcript);
        convert::drop_earlier_reasoning(&mut transcript, started);
        let answered = transcript.iter().rev().find(|message| message.info.role == Role::User);
        let answered = answered.map(|message| message.info.id.clone());

        let provider = plan.model_ref.provider.as_str();
        let reasoning = plan
            .reasoning()
            .or_else(|| catalog::default_reasoning(provider, &plan.model));
        let reasoning = self.top_level_effort(plan, reasoning, &mut transcript);
        let (max_tokens, reasoning) = budgets(&plan.model, reasoning);
        let sampling = catalog::sampling(&plan.model);
        let target = convert::OnCatalog {
            model: &plan.model_ref,
            catalog: &plan.catalog,
            account: plan.account.as_deref(),
        };
        let mut messages = compaction::request_messages(&transcript, &target, &lead);
        let effort_marks = super::effort::lift_marks(&mut messages);
        let no_tool_calls = closing.is_some();
        if let Some(text) = closing {
            convert::push(&mut messages, llm::Role::User, vec![llm::Block::Text(text)]);
        }
        let deferred_tools = plan.offer.deferred_specs();
        let tool_references = if deferred_tools.is_empty() {
            Default::default()
        } else {
            crate::tool::tool_search::references(&transcript)
        };

        let request = Request {
            model: plan.model.wire(&plan.model_ref.model).to_string(),
            system,
            messages: llm::prepare_files(messages, &plan.model, |hash| self.store.blob(hash).ok().flatten()),
            tools: plan.offer.specs(),
            max_tokens,
            reasoning,
            temperature: sampling.temperature,
            cache_key: Some(
                self.store
                    .cache_root(&plan.session.id)
                    .unwrap_or_else(|_| plan.session.id.clone()),
            ),
            connection: Some(plan.session.id.clone()),
            effort_marks,
            claude: plan.model.claude,
            cache_hour: self.hour_cache(plan),
            cache_conversation: true,
            deferred_tools,
            tool_references,
            no_tool_calls,
            verbosity: catalog::verbosity(provider, &plan.model),
            show_thinking: catalog::shows_thinking(provider, &plan.model),
            top_p: sampling.top_p,
            top_k: sampling.top_k,
            mode: plan.model.mode.clone(),
            context_window: Some(plan.model.limit.context).filter(|window| *window > 0),
        };

        (request, answered)
    }

    /// Whether a request keeps its prompt cache for an hour, as Claude Code does: a conversation (never a worker) on
    /// a Claude subscription within its limits, or on an API key with Settings' one-hour cache on.
    pub(in crate::session) fn hour_cache(&self, plan: &Plan) -> bool {
        if plan.session.visibility == crate::session::types::Visibility::Hidden {
            return false;
        }

        match &plan.credential {
            Credential::ApiKey { .. } => self.prompt_cache().hour_cache,
            Credential::OAuth { .. } => !self.in_overage(plan.account.as_deref()),
            Credential::Ambient { .. } => false,
        }
    }

    /// Whether the account's last response said it is drawing on paid extra usage.
    fn in_overage(&self, account: Option<&str>) -> bool {
        account
            .and_then(|account| self.limits.get(account))
            .is_some_and(|limits| limits.overage)
    }

    /// Whether the next request will find nothing of this conversation in the provider's prompt cache: nothing was
    /// sent yet, or the last reply came from another model or account, or finished longer ago than the cache lives.
    /// Only then is changing the request's start, or its account, free.
    pub(in crate::session) fn cache_cold(&self, plan: &Plan) -> bool {
        let Ok(Some(last)) = self.store.last_cached_reply(&plan.session.id) else {
            return true;
        };
        let lifetime = if self.hour_cache(plan) { HOUR_CACHE_MS } else { CACHE_MS };

        let elsewhere = last.model.as_ref() != Some(&plan.model_ref) || last.account != plan.account;
        let expired = last
            .finished_at
            .is_none_or(|finished| id::now_ms() - finished > lifetime);

        elsewhere || expired
    }

    /// The request history starts at the latest summary's kept tail, or at the conversation's beginning.
    /// History already summarised is not loaded.
    pub(in crate::session) fn request_window(&self, session_id: &str) -> Option<Vec<MessageWithParts>> {
        let Some(start) = self.store.view_start(session_id).ok()? else {
            return self.store.transcript(session_id).ok();
        };

        let mut window = self.store.messages_from(session_id, &start).ok()?;
        if compaction::view(&window).summary.is_none() {
            return self.store.transcript(session_id).ok();
        }

        // A tail starting inside a turn must carry that turn's original prompt.
        if window.first().is_some_and(|first| first.info.role == Role::Assistant)
            && let Ok(Some(prompt)) = self.store.prompt_before(session_id, &start)
        {
            window.insert(0, prompt);
        }

        Some(window)
    }
}

/// The output limit and thinking budget stay within the model's output limit.
/// A thinking budget can raise the usual output cap, but leaves [`MIN_ANSWER_TOKENS`] for the answer.
/// A budget that cannot fit is reduced, or dropped if the provider's minimum cannot fit.
pub(super) fn budgets(model: &Model, requested: Option<Reasoning>) -> (u32, Option<Reasoning>) {
    // Unknown output limits use a quarter of the context window to leave room for the prompt.
    let unknown = u32::try_from(model.reply_room())
        .unwrap_or(MAX_OUTPUT_TOKENS)
        .max(MIN_ANSWER_TOKENS);
    let half = u32::try_from(model.limit.context / 2)
        .ok()
        .filter(|half| *half > 0)
        .unwrap_or(u32::MAX)
        .max(MIN_ANSWER_TOKENS);
    let model_limit = u32::try_from(model.limit.output)
        .ok()
        .filter(|limit| *limit > 0)
        .map_or(unknown, |limit| limit.min(half));

    let wanted = match requested.filter(|_| model.reasoning) {
        Some(Reasoning::Budget { tokens }) => tokens,
        effort => return (model_limit.min(MAX_OUTPUT_TOKENS), effort),
    };
    let max_tokens = model_limit.min(MAX_OUTPUT_TOKENS.max(wanted.saturating_add(MIN_ANSWER_TOKENS)));
    let room = max_tokens.saturating_sub(MIN_ANSWER_TOKENS);
    let thinking = (room >= MIN_THINKING_TOKENS).then(|| Reasoning::Budget {
        tokens: wanted.clamp(MIN_THINKING_TOKENS, room),
    });

    (max_tokens, thinking)
}

/// Prices are per million tokens.
/// What a request cost, at the prices for its prompt's length (all of its input, cached or not).
pub(in crate::session) fn cost(model: &Model, usage: Usage) -> f64 {
    let (input, output, cache_read, cache_write) = model.cost.at(usage.input + usage.cache_read + usage.cache_write);
    let price = usage.input as f64 * input
        + usage.output as f64 * output
        + usage.cache_read as f64 * cache_read
        + usage.cache_write as f64 * cache_write;

    price / 1_000_000.0
}
