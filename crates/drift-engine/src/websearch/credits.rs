//! What each built-in key has left, read from its provider's account endpoint, which spends no credit.

use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use super::providers::{BUILT_IN, Provider};
use super::{Attempt, SearchError, last_four, send};
use crate::Engine;

/// How a key answered a check.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum KeyState {
    Ok,
    Refused,
    RateLimited,
    Failed,
}

/// What a provider says a key can still spend.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Balance {
    pub left: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// When the plan's credits renew, as the provider wrote it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renews: Option<String>,
}

/// One key's check; Exa tells a plain key only that it works, so its balance is absent.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyCheck {
    pub provider: String,
    /// The key by its last four characters.
    pub key: String,
    /// Its place among the keys saved in Settings; absent for the environment's key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    pub state: KeyState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<Balance>,
}

impl Engine {
    /// Asks about every built-in key at once; a refused or spent key is set aside and one with credit is tried again.
    pub async fn check_search_keys(&self) -> Vec<KeyCheck> {
        let base = self.test_base();
        let keys: Vec<(&'static Provider, String, Option<usize>)> = BUILT_IN
            .iter()
            .flat_map(|provider| {
                let saved = self.saved_keys(provider.id).into_iter().enumerate();
                let environment = std::env::var(provider.env).ok().filter(|key| !key.trim().is_empty());
                saved
                    .map(|(index, key)| (provider, key, Some(index)))
                    .chain(environment.map(|key| (provider, key, None)))
                    .collect::<Vec<_>>()
            })
            .collect();
        let checks = keys.into_iter().map(|(provider, key, index)| {
            let base = base.clone();
            async move {
                let answer = balance(provider, &key, base.as_deref()).await;
                self.note_check(
                    Attempt::Key {
                        provider,
                        key: key.clone(),
                    },
                    &answer,
                );
                checked(provider, &key, index, answer)
            }
        });

        futures_util::future::join_all(checks).await
    }

    fn note_check(&self, attempt: Attempt, answer: &Result<Option<Balance>, SearchError>) {
        let spent = matches!(answer, Ok(Some(balance)) if balance.left <= 0);
        match answer {
            Ok(_) if spent => {
                self.search_routing
                    .refused
                    .lock()
                    .unwrap()
                    .insert(attempt.slot(), std::time::Instant::now());
            }
            Ok(_) => {
                self.search_routing.refused.lock().unwrap().remove(&attempt.slot());
                self.search_routing.cooling.lock().unwrap().remove(&attempt.slot());
            }
            Err(error) => {
                self.note_failure(&attempt, error.clone());
            }
        }
    }
}

fn checked(
    provider: &Provider,
    key: &str,
    index: Option<usize>,
    answer: Result<Option<Balance>, SearchError>,
) -> KeyCheck {
    let (state, error, balance) = match answer {
        Ok(balance) => (KeyState::Ok, None, balance),
        Err(SearchError::Refused(why)) => (KeyState::Refused, Some(why), None),
        Err(SearchError::RateLimited(wait)) => (
            KeyState::RateLimited,
            wait.map(|wait| format!("try again in {} s", wait.as_secs())),
            None,
        ),
        Err(SearchError::Failed(why)) => (KeyState::Failed, Some(why), None),
    };

    KeyCheck {
        provider: provider.id.into(),
        key: last_four(key),
        index,
        state,
        error,
        balance,
    }
}

/// The account behind `key`, as its provider reports it; Exa reports no balance to a plain key.
async fn balance(provider: &Provider, key: &str, base: Option<&str>) -> Result<Option<Balance>, SearchError> {
    let endpoint = match provider.id {
        "exa" => "websets/v0/teams/me",
        "tavily" => "usage",
        _ => "team/credit-usage",
    };
    let answer = send(provider.get(endpoint, key, base)).await?;

    let balance = match provider.id {
        "exa" => None,
        "tavily" => tavily(&answer),
        _ => firecrawl(&answer),
    };

    // Firecrawl counts an overdrawn key below zero; it has nothing left either way.
    Ok(balance.map(|balance| Balance {
        left: balance.left.max(0),
        ..balance
    }))
}

fn firecrawl(answer: &Value) -> Option<Balance> {
    let data = &answer["data"];

    Some(Balance {
        left: data["remainingCredits"].as_i64()?,
        total: data["planCredits"].as_i64(),
        plan: None,
        renews: data["billingPeriodEnd"].as_str().map(str::to_string),
    })
}

/// The plan's and pay-as-you-go credits left, capped by the key's own limit when it has one.
fn tavily(answer: &Value) -> Option<Balance> {
    let account = &answer["account"];
    let left_of = |limit: &Value, usage: &Value| limit.as_i64().map(|limit| limit - usage.as_i64().unwrap_or(0));
    let plan = left_of(&account["plan_limit"], &account["plan_usage"])?;
    let paygo = left_of(&account["paygo_limit"], &account["paygo_usage"]).unwrap_or(0);
    let total = account["plan_limit"].as_i64().unwrap_or(0) + account["paygo_limit"].as_i64().unwrap_or(0);
    let key = &answer["key"];
    let (left, total) = match (left_of(&key["limit"], &key["usage"]), key["limit"].as_i64()) {
        (Some(key_left), Some(key_limit)) => ((plan + paygo).min(key_left), total.min(key_limit)),
        _ => (plan + paygo, total),
    };

    Some(Balance {
        left,
        total: Some(total),
        plan: account["current_plan"].as_str().map(str::to_string),
        renews: None,
    })
}
