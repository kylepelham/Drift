//! The account a turn sends with. What each response reports of its usage is kept, so choosing the next
//! account is a lookup in memory: the first in the user's order that has usage left.

use super::*;
use crate::llm::limits::Limits;

/// How long an account refused for usage, without saying until when, is passed over.
const UNNAMED_RESET: std::time::Duration = std::time::Duration::from_secs(5 * 60 * 60);

impl Engine {
    /// Keeps the usage an account's response reported and tells the UI when it changed.
    pub(super) fn record_limits(&self, plan: &Plan, limits: Limits) {
        let Some(account) = &plan.account else {
            return;
        };

        if self.limits.record(account, limits.clone()) {
            self.publish_limits(&plan.model_ref.provider, account, limits);
        }
    }

    /// Whether `account` has usage left, as far as its last response said.
    pub(in crate::session) fn usable_account(&self, account: &str) -> bool {
        self.limits.spent(account, id::now_ms()).is_none()
    }

    /// The account the provider's requests should go to now: the first in order with usage left.
    fn preferred_account(&self, provider: &str) -> Option<String> {
        self.credentials
            .accounts(provider)
            .into_iter()
            .find(|account| self.usable_account(&account.key))
            .map(|account| account.key)
    }

    /// Moves the turn onto the preferred account before a request is built, so its history is signed for it.
    /// A turn on a credential from the environment, or with every account spent, stays where it is.
    pub(super) async fn follow_account(&self, plan: &mut Plan) {
        let Some(current) = plan.account.clone() else {
            return;
        };
        let provider = plan.model_ref.provider.clone();
        let Some(preferred) = self
            .preferred_account(&provider)
            .filter(|preferred| *preferred != current)
        else {
            return;
        };
        let Some(stored) = self.credentials.account(&preferred) else {
            return;
        };
        let Ok(credential) = self.fresh_credential(&provider, Some(&preferred), stored).await else {
            return;
        };

        plan.credential = credential;
        plan.account = Some(preferred.clone());
        self.announce_switch(plan, current, preferred);
    }

    /// After a refusal for spent usage, marks the account spent; `true` when another account can take the request.
    pub(super) fn leave_spent_account(&self, plan: &Plan, error: &llm::Error) -> bool {
        let (true, Some(account)) = (error.limit_reached(), &plan.account) else {
            return false;
        };
        let llm::Error::Api { retry_after, .. } = error else {
            return false;
        };

        let wait = retry_after.unwrap_or(UNNAMED_RESET);
        let until = id::now_ms().saturating_add(i64::try_from(wait.as_millis()).unwrap_or(i64::MAX));
        let limits = self.limits.refuse(account, until);
        self.publish_limits(&plan.model_ref.provider, account, limits);

        self.preferred_account(&plan.model_ref.provider)
            .is_some_and(|preferred| preferred != *account)
    }

    fn announce_switch(&self, plan: &Plan, from: String, to: String) {
        let accounts = self.credentials.accounts(&plan.model_ref.provider);
        let position = accounts.iter().position(|account| account.key == to).unwrap_or(0);

        self.hub.publish(Event::ProviderSwitched {
            session_id: plan.session.id.clone(),
            provider: plan.model_ref.provider.clone(),
            limited: !self.usable_account(&from),
            from,
            to,
            label: accounts.get(position).and_then(|account| account.label.clone()),
            position: position + 1,
        });
    }

    fn publish_limits(&self, provider: &str, account: &str, limits: Limits) {
        self.hub.publish(Event::ProviderLimits {
            provider: provider.into(),
            account: account.into(),
            limits,
        });
    }
}
