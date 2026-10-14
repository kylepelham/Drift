//! The account a turn sends with. What each response reports of its usage, which models each account was
//! refused, and which conversations its safety filter refused, is kept in memory, so choosing the next account
//! is a lookup: the first in the user's order that has usage left and can serve the turn's model and conversation.

use super::*;
use crate::event::SwitchReason;
use crate::llm::catalog::Model;
use crate::llm::limits::Limits;
use crate::session::types::Refusal;

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

    /// [`Engine::account_serves`], and the account has not refused this conversation under its safety filter.
    pub(in crate::session) fn account_suits(&self, account: &str, session: &str, model: &Model) -> bool {
        self.account_serves(account, model) && !self.unserved.refused_conversation(account, session)
    }

    /// The account this conversation's requests for `model` should go to now: the first in order that suits it.
    fn preferred_account(&self, provider: &str, session: &str, model: &Model) -> Option<String> {
        self.credentials
            .accounts(provider)
            .into_iter()
            .find(|account| self.account_suits(&account.key, session, model))
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
            .preferred_account(&provider, &plan.session.id, &plan.model)
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

        self.another_account(plan, account)
    }

    /// After the safety filter refused the reply, passes the account over for this conversation when another
    /// account may judge it differently; `true` when one can take the request. Accounts differ on cybersecurity
    /// (Anthropic's Cyber Verification, OpenAI's Daybreak), so a cyber refusal, or one naming no policy area, moves
    /// on; a refusal in any other area stands.
    pub(super) fn leave_refusing_account(&self, plan: &Plan, refusal: Option<&Refusal>) -> bool {
        let Some(account) = &plan.account else {
            return false;
        };
        let category = refusal.and_then(|refusal| refusal.category.as_deref());
        let accounts_differ = category.is_none_or(|category| category.to_ascii_lowercase().contains("cyber"));
        if !accounts_differ {
            return false;
        }

        self.unserved.refuse_conversation(account, &plan.session.id);

        self.another_account(plan, account)
    }

    /// Whether an account other than `account` suits the turn now.
    fn another_account(&self, plan: &Plan, account: &str) -> bool {
        self.preferred_account(&plan.model_ref.provider, &plan.session.id, &plan.model)
            .is_some_and(|preferred| preferred != account)
    }

    /// Why the turn is leaving `from`: whatever now keeps it from suiting the turn, else an earlier account's reset.
    fn switch_reason(&self, plan: &Plan, from: &str) -> SwitchReason {
        if !self.usable_account(from) {
            return SwitchReason::Limit;
        }
        if self.unserved.refused_conversation(from, &plan.session.id) {
            return SwitchReason::Refusal;
        }
        if !self.account_serves(from, &plan.model) {
            return SwitchReason::Model;
        }

        SwitchReason::Reset
    }

    fn announce_switch(&self, plan: &Plan, from: String, to: String) {
        let accounts = self.credentials.accounts(&plan.model_ref.provider);
        let position = accounts.iter().position(|account| account.key == to).unwrap_or(0);

        self.hub.publish(Event::ProviderSwitched {
            session_id: plan.session.id.clone(),
            provider: plan.model_ref.provider.clone(),
            reason: self.switch_reason(plan, &from),
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
