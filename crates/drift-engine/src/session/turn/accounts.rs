//! The account a turn sends with. What each response reports of its usage, and which models each account was
//! refused, is kept in memory, so choosing the next account is a lookup: the first in the user's order that has
//! usage left and can serve the turn's model.

use super::*;
use crate::llm::catalog::Model;
use crate::llm::limits::Limits;

/// How long an account refused for usage, without saying until when, is passed over.
const UNNAMED_RESET: std::time::Duration = std::time::Duration::from_secs(5 * 60 * 60);
/// How long an account refused a model its plan does not include is passed over for that model.
const UNSERVED_FOR: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

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

    /// Whether `account` can take a request for `model` now: it has usage left, was not refused the model,
    /// and, for a ChatGPT sign-in, its own model list offers the model's Daybreak program and speed tier.
    pub(in crate::session) fn account_serves(&self, account: &str, model: &Model) -> bool {
        if !self.usable_account(account) || self.unserved.refused(account, &model.id, id::now_ms()) {
            return false;
        }

        // Only a mode can be missing from one account's list; reading the credential is skipped otherwise.
        if model.mode.is_none() {
            return true;
        }
        let offered = self
            .credentials
            .account(account)
            .and_then(|credential| self.codex_offers.serves(&credential, model));

        offered != Some(false)
    }

    /// An account's credential for callers outside a turn, renewed first when it has expired.
    pub(crate) async fn account_credential(&self, provider: &str, account: &str) -> Option<Credential> {
        let stored = self.credentials.account(account)?;
        if !stored.is_expired() {
            return Some(stored);
        }

        self.renew(provider, Some(account), stored).await.ok()
    }

    /// The account requests for `model` should go to now: the first in order that can serve it.
    fn preferred_account(&self, provider: &str, model: &Model) -> Option<String> {
        self.credentials
            .accounts(provider)
            .into_iter()
            .find(|account| self.account_serves(&account.key, model))
            .map(|account| account.key)
    }

    /// Moves the turn onto the preferred account before a request is built, so its history is signed for it, and
    /// fits the request to that account. A turn on a credential from the environment stays where it is, and so
    /// does one no account can serve.
    pub(super) async fn follow_account(&self, plan: &mut Plan) {
        self.switch_to_preferred(plan).await;
        self.codex_offers.fit(&mut plan.model, &plan.credential);
    }

    async fn switch_to_preferred(&self, plan: &mut Plan) {
        let Some(current) = plan.account.clone() else {
            return;
        };
        let provider = plan.model_ref.provider.clone();
        let Some(preferred) = self
            .preferred_account(&provider, &plan.model)
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

    /// After a refusal another account may not have, records it against the account: spent usage, or a model its
    /// plan does not include. `true` when another account can take the request.
    pub(super) fn leave_refused_account(&self, plan: &Plan, error: &llm::Error) -> bool {
        let Some(account) = &plan.account else {
            return false;
        };

        if error.plan_lacks_model() {
            let until = id::now_ms().saturating_add(millis(UNSERVED_FOR));
            self.unserved.refuse(account, &plan.model.id, until);
        } else if error.limit_reached() {
            let llm::Error::Api { retry_after, .. } = error else {
                return false;
            };
            let wait = retry_after.unwrap_or(UNNAMED_RESET);
            let limits = self.limits.refuse(account, id::now_ms().saturating_add(millis(wait)));
            self.publish_limits(&plan.model_ref.provider, account, limits);
        } else {
            return false;
        }

        self.preferred_account(&plan.model_ref.provider, &plan.model)
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

fn millis(duration: std::time::Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}
