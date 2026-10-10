//! How much of a subscription's usage windows an account has spent, read from what every response already
//! carries, and kept in memory so choosing an account never waits on the network.

use std::collections::HashMap;
use std::sync::Mutex;

use ::http::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

/// The kind a request refused for spent usage is given, whatever the provider called it.
pub const LIMIT_REACHED: &str = "usage_limit_reached";

/// Windows shorter than a day are the rolling five-hour one; longer ones are weekly.
const DAY_MINUTES: f64 = 1440.0;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<Window>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weekly: Option<Window>,
    /// The plan's usage is spent until then, in ms; requests before it are refused or paid for with credits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spent_until: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub used_percent: f64,
    /// In ms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
}

impl Limits {
    /// Codex's `x-codex-*` or Anthropic's `anthropic-ratelimit-unified-*` headers; `None` when there are neither.
    pub fn from_headers(headers: &HeaderMap) -> Option<Self> {
        codex_headers(headers).or_else(|| anthropic_headers(headers))
    }

    /// The `codex.rate_limits` event a Codex WebSocket sends before each response.
    pub fn from_codex_event(event: &Value) -> Option<Self> {
        let limits = &event["rate_limits"];
        let windows = ["primary", "secondary"].map(|which| {
            let window = &limits[which];
            let resets_at = seconds_at(window["reset_at"].as_f64(), window["reset_after_seconds"].as_f64());

            window["window_minutes"]
                .as_f64()
                .zip(window["used_percent"].as_f64())
                .map(|(minutes, used)| {
                    (
                        minutes,
                        Window {
                            used_percent: used,
                            resets_at,
                        },
                    )
                })
        });
        let reached = limits["limit_reached"].as_bool() == Some(true);

        Self::of(windows.into_iter().flatten(), reached.then_some(None))
    }

    /// When usage is spent as of `now`; `None` while the account can still be used.
    pub fn spent_at(&self, now: i64) -> Option<i64> {
        self.spent_until.filter(|until| *until > now)
    }

    /// Sorts reported windows into the five-hour and weekly ones. `refused` is the provider saying usage is
    /// spent, with when it frees if it said; a full window says the same.
    fn of(windows: impl Iterator<Item = (f64, Window)>, refused: Option<Option<i64>>) -> Option<Self> {
        let mut limits = Self::default();
        for (minutes, window) in windows.filter(|(minutes, _)| *minutes > 0.0) {
            let slot = if minutes < DAY_MINUTES {
                &mut limits.five_hour
            } else {
                &mut limits.weekly
            };
            *slot = Some(window);
        }

        let full = [limits.five_hour, limits.weekly]
            .into_iter()
            .flatten()
            .filter(|window| window.used_percent >= 100.0)
            .filter_map(|window| window.resets_at)
            .max();
        limits.spent_until = match refused {
            Some(until) => until.or(full).or_else(|| latest_reset(&limits)),
            None => full,
        };

        let reported = limits.five_hour.is_some() || limits.weekly.is_some() || refused.is_some();
        reported.then_some(limits)
    }
}

/// Without a named reset, a refused account is spent until its last window resets.
fn latest_reset(limits: &Limits) -> Option<i64> {
    [limits.five_hour, limits.weekly]
        .into_iter()
        .flatten()
        .filter_map(|window| window.resets_at)
        .max()
}

fn codex_headers(headers: &HeaderMap) -> Option<Limits> {
    let number = |name: String| header(headers, &name).and_then(|text| text.parse::<f64>().ok());
    let windows = ["primary", "secondary"].map(|which| {
        let minutes = number(format!("x-codex-{which}-window-minutes"))?;
        let used = number(format!("x-codex-{which}-used-percent"))?;
        let resets_at = seconds_at(
            number(format!("x-codex-{which}-reset-at")),
            number(format!("x-codex-{which}-reset-after-seconds")),
        );

        Some((
            minutes,
            Window {
                used_percent: used,
                resets_at,
            },
        ))
    });

    Limits::of(windows.into_iter().flatten(), None)
}

/// Anthropic reports each window's use as a fraction and its reset in epoch seconds.
fn anthropic_headers(headers: &HeaderMap) -> Option<Limits> {
    let number = |name: &str| {
        header(headers, &format!("anthropic-ratelimit-unified-{name}")).and_then(|text| text.parse::<f64>().ok())
    };
    let window = |abbreviation: &str, minutes: f64| {
        let used = number(&format!("{abbreviation}-utilization"))?;
        let resets_at = seconds_at(number(&format!("{abbreviation}-reset")), None);

        Some((
            minutes,
            Window {
                used_percent: used * 100.0,
                resets_at,
            },
        ))
    };
    let rejected = header(headers, "anthropic-ratelimit-unified-status") == Some("rejected");
    let refused = rejected.then(|| seconds_at(number("reset"), None));

    Limits::of(
        [window("5h", 300.0), window("7d", 10080.0)].into_iter().flatten(),
        refused,
    )
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok()).map(str::trim)
}

/// An epoch-seconds time, else one `after` seconds from now, in ms.
fn seconds_at(at: Option<f64>, after: Option<f64>) -> Option<i64> {
    let at = at.filter(|at| *at > 0.0).map(|at| (at * 1000.0) as i64);

    at.or_else(|| after.map(|after| crate::id::now_ms() + (after * 1000.0) as i64))
}

/// Each account's last reported limits, by account key.
#[derive(Default)]
pub struct Ledger(Mutex<HashMap<String, Limits>>);

impl Ledger {
    /// Keeps what a response reported; `true` when it differs from what was known.
    pub fn record(&self, account: &str, limits: Limits) -> bool {
        self.0.lock().unwrap().insert(account.into(), limits.clone()) != Some(limits)
    }

    pub fn get(&self, account: &str) -> Option<Limits> {
        self.0.lock().unwrap().get(account).cloned()
    }

    /// When the account's usage frees, while it is spent; `None` when it can be used or nothing is known.
    pub fn spent(&self, account: &str, now: i64) -> Option<i64> {
        self.0
            .lock()
            .unwrap()
            .get(account)
            .and_then(|limits| limits.spent_at(now))
    }

    /// Marks an account spent until `until` after a refusal, keeping the windows it last reported.
    pub fn refuse(&self, account: &str, until: i64) -> Limits {
        let mut known = self.0.lock().unwrap();
        let limits = known.entry(account.into()).or_default();
        limits.spent_until = Some(until);

        limits.clone()
    }
}

#[cfg(test)]
mod tests;
