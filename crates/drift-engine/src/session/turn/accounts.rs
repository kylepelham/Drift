//! The account a turn sends with: what each response reports of its usage is kept, so the next choice is made from memory.

use super::*;
use crate::llm::limits::Limits;

impl Engine {
    /// Keeps the usage an account's response reported and tells the UI when it changed.
    pub(super) fn record_limits(&self, plan: &Plan, limits: Limits) {
        let Some(account) = &plan.account else {
            return;
        };

        if self.limits.record(account, limits.clone()) {
            self.hub.publish(Event::ProviderLimits {
                provider: plan.model_ref.provider.clone(),
                account: account.clone(),
                limits,
            });
        }
    }
}
