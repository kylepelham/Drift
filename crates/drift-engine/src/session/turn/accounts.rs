//! The account a turn sends with. What each response reports of its usage, which models each account was
//! refused, and which conversations its safety filter refused, is kept in memory, so choosing the next account
//! is a lookup: the first in the user's order that has usage left and can serve the turn's model and conversation.

use super::*;
use crate::event::SwitchReason;
use crate::llm::catalog::Model;
use crate::llm::limits::Limits;
use crate::session::oneshot::{Failure, Resolved};
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

    /// [`Engine::account_serves`], and the account has not refused this conversation under its safety filter. Once one
    /// has, only an account the user marked cyber verified suits it: another's filter would refuse it the same way.
    pub(in crate::session) fn account_suits(&self, account: &str, session: &str, model: &Model) -> bool {
        if !self.account_serves(account, model) || self.unserved.refused_conversation(account, session) {
            return false;
        }

        !self.unserved.refused_anywhere(session) || self.credentials.cyber_verified(account)
    }

    /// The account this conversation's requests for `model` should go to now: the one the user set it to while that
    /// suits it, else the first in order that does.
    fn preferred_account(&self, provider: &str, session: &str, model: &Model) -> Option<String> {
        self.preferred_accounts(provider, session, model).into_iter().next()
    }

    /// Every account that suits the request, best first: the one the chat is set to, then the user's order.
    fn preferred_accounts(&self, provider: &str, session: &str, model: &Model) -> Vec<String> {
        let pinned = self.pinned_account(provider, session, model);
        let suitable = self
            .credentials
            .accounts(provider)
            .into_iter()
            .map(|account| account.key)
            .filter(|key| Some(key) != pinned.as_ref() && self.account_suits(key, session, model));

        pinned.clone().into_iter().chain(suitable).collect()
    }

    /// The account the user set the conversation to use, while it is one of `provider`'s and suits the request now.
    /// It is read each time, so a choice made during a turn applies from its next request.
    pub(super) fn pinned_account(&self, provider: &str, session: &str, model: &Model) -> Option<String> {
        let pinned = self.store.session(session).ok().flatten()?.account?;
        let listed = self
            .credentials
            .accounts(provider)
            .iter()
            .any(|account| account.key == pinned);

        (listed && self.account_suits(&pinned, session, model)).then_some(pinned)
    }

    /// Moves the turn onto the preferred account before a request is built, so its history is signed for it, and
    /// fits the request to that account. A turn on a credential from the environment stays where it is, and so
    /// does one no account can serve.
    pub(super) async fn follow_account(&self, plan: &mut Plan) {
        self.switch_to_preferred(plan).await;
        self.codex_offers.fit(&mut plan.model, &plan.credential);
    }

    /// A new turn goes to the account the user set the conversation to use, else back to the account its last reply
    /// used while that account suits it and the cache there is still warm; the user's order decides once it has gone cold.
    pub(super) async fn stay_where_warm(&self, plan: &mut Plan) {
        if let Some(pinned) = self.pinned_account(&plan.model_ref.provider, &plan.session.id, &plan.model)
            && self.move_to_pinned(plan, pinned).await
        {
            return;
        }
        let Ok(Some(last)) = self.store.last_cached_reply(&plan.session.id) else {
            return;
        };
        let Some(account) = last.account.filter(|account| plan.account.as_ref() != Some(account)) else {
            return;
        };
        if last.model.as_ref() != Some(&plan.model_ref) || !self.account_suits(&account, &plan.session.id, &plan.model)
        {
            return;
        }
        let Some(stored) = self.credentials.account(&account) else {
            return;
        };
        let Ok(credential) = self
            .fresh_credential(&plan.model_ref.provider, Some(&account), stored)
            .await
        else {
            return;
        };

        let chosen = (
            std::mem::replace(&mut plan.credential, credential),
            plan.account.replace(account),
        );
        if self.cache_cold(plan) {
            (plan.credential, plan.account) = chosen;
            return;
        }

        self.codex_offers.fit(&mut plan.model, &plan.credential);
    }

    async fn move_to_pinned(&self, plan: &mut Plan, pinned: String) -> bool {
        if plan.account.as_ref() == Some(&pinned) {
            return true;
        }
        let Some(credential) = self.account_credential(&plan.model_ref.provider, &pinned).await else {
            return false;
        };

        plan.credential = credential;
        plan.account = Some(pinned);
        self.codex_offers.fit(&mut plan.model, &plan.credential);
        true
    }

    /// Moves to the first suitable account in the user's order. While the current one still suits and the conversation's
    /// cache on it is warm, the turn stays: moving would write the whole prompt into the other account's cache again.
    /// The account the user set the conversation to use is moved to whatever its cache, and without a notice.
    async fn switch_to_preferred(&self, plan: &mut Plan) {
        let Some(current) = plan.account.clone() else {
            return;
        };
        let to_pinned = self
            .pinned_account(&plan.model_ref.provider, &plan.session.id, &plan.model)
            .is_some_and(|pinned| pinned != current);
        if !to_pinned && self.account_suits(&current, &plan.session.id, &plan.model) && !self.cache_cold(plan) {
            return;
        }

        let provider = plan.model_ref.provider.clone();
        let pinned = self.pinned_account(&provider, &plan.session.id, &plan.model);
        let Some((preferred, credential)) = self
            .first_renewed(&provider, &plan.session.id, &plan.model, &current)
            .await
        else {
            return;
        };

        plan.credential = credential;
        plan.account = Some(preferred.clone());
        if pinned.as_ref() != Some(&preferred) {
            self.announce_switch(Switch {
                session: &plan.session.id,
                provider: &provider,
                reason: self.switch_reason(plan, &current),
                from: current,
                to: preferred,
            });
        }
    }

    /// The best suitable account ahead of `current` whose credential renews, passing over any that cannot; `None` when
    /// `current` comes first or none ahead of it renews.
    async fn first_renewed(
        &self,
        provider: &str,
        session: &str,
        model: &Model,
        current: &str,
    ) -> Option<(String, Credential)> {
        let ahead = self
            .preferred_accounts(provider, session, model)
            .into_iter()
            .take_while(|account| account != current);
        self.renewed(provider, ahead).await
    }

    /// The best suitable account other than `refusing` whose credential renews, passing over any that cannot.
    async fn renewed_other(
        &self,
        provider: &str,
        session: &str,
        model: &Model,
        refusing: &str,
    ) -> Option<(String, Credential)> {
        let others = self
            .preferred_accounts(provider, session, model)
            .into_iter()
            .filter(|account| account != refusing);
        self.renewed(provider, others).await
    }

    /// The first of `accounts` whose stored credential renews, with that credential.
    async fn renewed(&self, provider: &str, accounts: impl Iterator<Item = String>) -> Option<(String, Credential)> {
        for account in accounts {
            let Some(stored) = self.credentials.account(&account) else {
                continue;
            };
            if let Ok(credential) = self.fresh_credential(provider, Some(&account), stored).await {
                return Some((account, credential));
            }
        }
        None
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

    /// After the safety filter refused the reply, passes the account over for this conversation when an account the
    /// user marked cyber verified may judge it differently ([`accounts_differ`]), and sets the chat to the one that
    /// takes the request ([`Engine::settle_after_refusal`]); `true` when one can. With none, the refusal stands where
    /// it was made, and the conversation keeps its account and cache.
    pub(super) async fn leave_refusing_account(&self, plan: &Plan, refusal: Option<&Refusal>) -> bool {
        let Some(account) = &plan.account else {
            return false;
        };
        if !accounts_differ(refusal) {
            return false;
        }
        self.learn_verified_usage(&plan.model_ref.provider).await;
        if !self.cleared_account_for(&plan.model_ref.provider, &plan.session.id, &plan.model, account) {
            return false;
        }

        self.unserved.refuse_conversation(account, &plan.session.id);

        let provider = &plan.model_ref.provider;
        // An account whose credential cannot renew would only send the request back to the refusing one.
        let Some((next, _)) = self
            .renewed_other(provider, &plan.session.id, &plan.model, account)
            .await
        else {
            return false;
        };
        self.settle_after_refusal(provider, &plan.session.id, account, &next)
    }

    /// After the safety filter refused a request made for `session` outside its turns (a compaction summary), passes
    /// the account over for the conversation and sets the chat to the next one as [`Engine::leave_refusing_account`]
    /// does, returning that account ready to call.
    pub(crate) async fn leave_refusing_account_for(
        &self,
        session: &str,
        resolved: &Resolved,
        failure: &Failure,
    ) -> Option<Resolved> {
        let Failure::Refused(refusal, _) = failure else {
            return None;
        };
        let account = resolved.account.as_deref()?;
        let provider = &resolved.model_ref.provider;
        if !accounts_differ(refusal.as_ref()) {
            return None;
        }
        self.learn_verified_usage(provider).await;
        if !self.cleared_account_for(provider, session, &resolved.model, account) {
            return None;
        }
        self.unserved.refuse_conversation(account, session);

        let (next, credential) = self.renewed_other(provider, session, &resolved.model, account).await?;
        if !self.settle_after_refusal(provider, session, account, &next) {
            return None;
        }
        let mut model = resolved.model.clone();
        self.codex_offers.fit(&mut model, &credential);

        Some(Resolved {
            model_ref: resolved.model_ref.clone(),
            model,
            provider: resolved.provider.clone(),
            credential,
            account: Some(next),
        })
    }

    /// Sets the chat to the account that takes over from one whose filter refused it, as `/account` would: only this
    /// chat moves, it stays there across restarts, and its subagents and spawned threads start there. The user's
    /// order still decides for every other chat. `false` when the chat could not be set, so the refusal stands.
    fn settle_after_refusal(&self, provider: &str, session: &str, from: &str, to: &str) -> bool {
        let Ok(Some(updated)) = self.store.set_session_account(session, Some(to)) else {
            return false;
        };
        self.hub.publish(Event::SessionUpdated { session: updated });

        self.announce_switch(Switch {
            session,
            provider,
            reason: SwitchReason::Refusal,
            from: from.into(),
            to: to.into(),
        });
        true
    }

    /// Whether an account other than `account` suits the turn now.
    fn another_account(&self, plan: &Plan, account: &str) -> bool {
        self.preferred_account(&plan.model_ref.provider, &plan.session.id, &plan.model)
            .is_some_and(|preferred| preferred != account)
    }

    /// Whether an account other than `refusing`, marked cyber verified and not yet refused the conversation, can serve
    /// `model` now: the only kind worth sending a refused conversation to. One with its usage spent is not.
    fn cleared_account_for(&self, provider: &str, session: &str, model: &Model, refusing: &str) -> bool {
        self.credentials.accounts(provider).iter().any(|account| {
            account.key != refusing
                && account.cyber_verified
                && self.account_serves(&account.key, model)
                && !self.unserved.refused_conversation(&account.key, session)
        })
    }

    /// Asks Claude how much usage each verified account has left when none of its responses has said since the engine
    /// started, so a refused conversation is never sent to an account that would only answer that its usage is spent.
    async fn learn_verified_usage(&self, provider: &str) {
        let unknown = self
            .credentials
            .accounts(provider)
            .into_iter()
            .filter(|account| account.cyber_verified && self.limits.get(&account.key).is_none());

        for account in unknown {
            let Some(Credential::OAuth { access, .. }) = self.account_credential(provider, &account.key).await else {
                continue;
            };
            let Some(body) = llm::anthropic::oauth::usage(&llm::http::client(), &access).await else {
                continue;
            };
            if let Some(limits) = Limits::from_anthropic_usage(&body)
                && self.limits.record(&account.key, limits.clone())
            {
                self.publish_limits(provider, &account.key, limits);
            }
        }
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

    fn announce_switch(&self, switch: Switch) {
        let Switch {
            session,
            provider,
            reason,
            from,
            to,
        } = switch;
        let accounts = self.credentials.accounts(provider);
        let position = accounts.iter().position(|account| account.key == to).unwrap_or(0);

        self.hub.publish(Event::ProviderSwitched {
            session_id: session.into(),
            provider: provider.into(),
            reason,
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

/// A conversation moving from one account to another, and why.
struct Switch<'a> {
    session: &'a str,
    provider: &'a str,
    reason: SwitchReason,
    from: String,
    to: String,
}

/// Anthropic's Cyber Verification lets an account through what others refuse on cybersecurity, so a cyber refusal, or
/// one naming no policy area, may not be made by a verified account; a refusal in any other area stands. OpenAI's
/// Daybreak is a model, not an account, so its accounts are never marked and its refusals stand.
fn accounts_differ(refusal: Option<&Refusal>) -> bool {
    let category = refusal.and_then(|refusal| refusal.category.as_deref());

    category.is_none_or(|category| category.to_ascii_lowercase().contains("cyber"))
}

fn millis(duration: std::time::Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}
