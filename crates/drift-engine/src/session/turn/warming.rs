//! Prompt cache warming, off by default. After a conversation's last real request, the request its next
//! turn would send is sent again every interval with a short transient prompt and tools off, until the
//! window has passed, so the provider keeps its cached prefix. Nothing is saved to the transcript; the
//! cost goes on the reply whose cache it kept. Any real request starts both timers again.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::Plan;
use crate::Engine;
use crate::event::Event;
use crate::session::oneshot::SendOptions;
use crate::session::types::Visibility;

pub const PROMPT_CACHE_KEY: &str = "promptCache";
const DEFAULT_PROMPT: &str = "This is a keep-alive request. Do no work and use no tools. Reply with exactly: OK";
/// A keep-alive answers in a word; one that takes longer than this is given up and tried next interval.
const KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_MINUTES: u32 = 24 * 60;
#[cfg(not(test))]
const MINUTE: Duration = Duration::from_secs(60);
#[cfg(test)]
const MINUTE: Duration = Duration::from_millis(200);

/// Settings > Execution > Prompt cache, kept by the engine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct PromptCache {
    /// Keep an idle conversation's prompt cache warm.
    pub warming: bool,
    /// Minutes between keep-alive requests, 1 to 59.
    pub interval_minutes: u32,
    /// Minutes after the last real request that warming goes on for, at least one interval.
    pub window_minutes: u32,
    /// What a keep-alive request asks; empty takes the default.
    pub prompt: String,
}

impl Default for PromptCache {
    fn default() -> Self {
        Self {
            warming: false,
            interval_minutes: 4,
            window_minutes: 30,
            prompt: String::new(),
        }
    }
}

impl PromptCache {
    /// Why the settings cannot be kept, if they cannot.
    pub fn problem(&self) -> Option<&'static str> {
        if !(1..60).contains(&self.interval_minutes) {
            return Some("the interval is 1 to 59 minutes");
        }
        if self.window_minutes < self.interval_minutes || self.window_minutes > MAX_MINUTES {
            return Some("the window is at least one interval and at most a day");
        }

        None
    }

    fn prompt(&self) -> &str {
        Some(self.prompt.trim())
            .filter(|prompt| !prompt.is_empty())
            .unwrap_or(DEFAULT_PROMPT)
    }
}

/// Each session's warming loop, so a real request can replace it.
#[derive(Default)]
pub(crate) struct Warmers(Mutex<HashMap<String, tokio::task::AbortHandle>>);

impl Warmers {
    fn replace(&self, session_id: &str, task: Option<tokio::task::AbortHandle>) {
        let mut warmers = self.0.lock().unwrap();
        let old = match task {
            Some(task) => warmers.insert(session_id.to_string(), task),
            None => warmers.remove(session_id),
        };
        if let Some(old) = old {
            old.abort();
        }
    }

    fn stop_all(&self) {
        for (_, warmer) in self.0.lock().unwrap().drain() {
            warmer.abort();
        }
    }
}

impl Engine {
    pub fn prompt_cache(&self) -> PromptCache {
        self.store.setting(PROMPT_CACHE_KEY).ok().flatten().unwrap_or_default()
    }

    /// Keeps the settings; switching warming off stops every conversation's warming at once.
    pub fn set_prompt_cache(&self, cache: &PromptCache) -> rusqlite::Result<()> {
        self.store.set_setting(PROMPT_CACHE_KEY, cache)?;
        if !cache.warming {
            self.turns.warmers.stop_all();
        }

        Ok(())
    }

    /// Starts the session's timers again after a real request; workers and a switched-off setting never warm.
    pub(super) fn warm_after(self: &Arc<Self>, plan: &Plan) {
        let settings = self.prompt_cache();
        let session = plan.session.id.clone();
        if !settings.warming || plan.session.visibility == Visibility::Hidden {
            self.turns.warmers.replace(&session, None);
            return;
        }

        let engine = self.clone();
        let id = session.clone();
        let task = tokio::spawn(async move {
            let interval = MINUTE * settings.interval_minutes;
            let rounds = settings.window_minutes / settings.interval_minutes;
            for _ in 0..rounds {
                tokio::time::sleep(interval).await;
                engine.keep_alive(&id, &settings).await;
            }
        });
        self.turns.warmers.replace(&session, Some(task.abort_handle()));
    }

    async fn keep_alive(&self, session_id: &str, settings: &PromptCache) {
        let archived = self
            .store
            .session(session_id)
            .ok()
            .flatten()
            .is_none_or(|session| session.archived_at.is_some());
        if archived || self.turns.is_running(session_id) {
            return;
        }
        let prompt = super::Prompt {
            parts: Vec::new(),
            model: None,
            variant: None,
            agent: None,
            submission_id: None,
            worktree: false,
        };
        let Ok(plan) = self.plan(session_id, &prompt).await else {
            return;
        };
        let Some(window) = self.request_window(session_id) else {
            return;
        };

        let (request, _) = self.step_request(&plan, window, None, Some(settings.prompt().to_string()));
        let options = SendOptions {
            timeout: KEEP_ALIVE_TIMEOUT,
            shown_in: None,
        };
        match self.send(&plan.provider, &plan.credential, &request, options).await {
            Ok(answer) => self.charge(session_id, super::request::cost(&plan.model, answer.usage)),
            Err(failure) => eprintln!("drift: keeping {session_id}'s cache warm failed: {}", failure.message()),
        }
    }

    /// Adds a keep-alive's cost to the reply it kept warm, so the session's total includes it.
    fn charge(&self, session_id: &str, cost: f64) {
        let Ok(Some(mut reply)) = self.store.last_reply(session_id) else {
            return;
        };
        reply.info.cost += cost;
        if self.store.save_message(&reply.info).is_ok() {
            self.hub.publish(Event::MessageUpdated { message: reply.info });
        }
    }
}
