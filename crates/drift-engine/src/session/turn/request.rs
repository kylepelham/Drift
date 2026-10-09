use super::*;

/// Output cap when the model allows more; keeps a runaway response from burning the budget.
pub(super) const MAX_OUTPUT_TOKENS: u32 = 32_000;
/// What a thinking budget always leaves for the answer itself.
pub(super) const MIN_ANSWER_TOKENS: u32 = 1024;
/// The smallest thinking budget providers accept.
pub(super) const MIN_THINKING_TOKENS: u32 = 1024;

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
        let lead = prompt::remind_agents(&plan.config, &plan.session.agent, &mut transcript);
        convert::drop_earlier_reasoning(&mut transcript, started);
        let answered = transcript.iter().rev().find(|message| message.info.role == Role::User);
        let answered = answered.map(|message| message.info.id.clone());

        let provider = plan.model_ref.provider.as_str();
        let reasoning = plan
            .reasoning()
            .or_else(|| catalog::default_reasoning(provider, &plan.model));
        let (max_tokens, reasoning) = budgets(&plan.model, reasoning);
        let sampling = catalog::sampling(&plan.model);
        let target = convert::OnCatalog {
            model: &plan.model_ref,
            catalog: &plan.catalog,
        };
        let mut messages = compaction::request_messages(&transcript, &target, &lead);
        let no_tool_calls = closing.is_some();
        if let Some(text) = closing {
            convert::push(&mut messages, llm::Role::User, vec![llm::Block::Text(text)]);
        }

        let request = Request {
            model: plan.model.wire(&plan.model_ref.model).to_string(),
            system: plan.offer.system.clone(),
            messages: llm::prepare_files(messages, &plan.model, |hash| self.store.blob(hash).ok().flatten()),
            tools: plan.offer.specs(),
            max_tokens,
            reasoning,
            temperature: sampling.temperature,
            cache_key: Some(plan.session.id.clone()),
            no_tool_calls,
            verbosity: catalog::verbosity(provider, &plan.model),
            show_thinking: catalog::shows_thinking(provider, &plan.model),
            top_p: sampling.top_p,
            top_k: sampling.top_k,
            mode: plan.model.mode.clone(),
        };

        (request, answered)
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
