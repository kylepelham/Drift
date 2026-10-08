use super::*;

pub(in crate::session) struct Retry {
    /// The provider's words, for the UI.
    pub(super) message: String,
    /// The wait the provider asked for, if it named one.
    pub(super) after: Option<Duration>,
}

impl Retry {
    pub(in crate::session) fn from(error: &llm::Error) -> Option<Self> {
        match error {
            llm::Error::Api {
                retryable: true,
                retry_after,
                ..
            } => Some(Self {
                message: error.to_string(),
                after: *retry_after,
            }),
            llm::Error::Transport(_) => Some(Self {
                message: error.to_string(),
                after: None,
            }),
            _ => None,
        }
    }

    /// Worth waiting for: attempts remain and the provider did not ask for longer than we will wait.
    pub(in crate::session) fn message(&self) -> &str {
        &self.message
    }

    pub(in crate::session) fn allowed(&self, retries: u32) -> bool {
        retries < MAX_RETRIES && self.after.is_none_or(|after| after <= MAX_REQUESTED_WAIT)
    }

    /// The provider's wait when it named one, else doubling backoff with jitter, capped.
    pub(in crate::session) fn delay(&self, attempt: u32) -> Duration {
        if let Some(after) = self.after {
            return after;
        }

        let doubled = RETRY_BASE.saturating_mul(1 << attempt.saturating_sub(1).min(16));
        let mut byte = [0u8; 1];
        let _ = getrandom::fill(&mut byte);
        let jitter = 0.8 + 0.4 * f64::from(byte[0]) / 255.0;

        doubled.mul_f64(jitter).min(MAX_BACKOFF)
    }
}

impl Engine {
    /// Waits out a retry backoff, which the UI shows, unless the user switches the turn to another
    /// model first; then it retries at once on that model.
    pub(super) async fn wait_to_retry(
        &self,
        session_id: &str,
        attempt: u32,
        retry: &Retry,
        abort: &CancellationToken,
    ) -> Wait {
        let delay = retry.delay(attempt);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.turns.retry_waits.lock().unwrap().insert(session_id.into(), sender);

        let next_at = id::now_ms() + delay.as_millis() as i64;
        self.hub.publish(Event::SessionRetry {
            session_id: session_id.into(),
            attempt,
            message: retry.message.clone(),
            next_at,
        });
        let wait = tokio::select! {
            () = tokio::time::sleep(delay) => Wait::Elapsed,
            Ok(switch) = receiver => Wait::Switched(Box::new(switch)),
            () = abort.cancelled() => Wait::Stopped,
        };

        self.turns.retry_waits.lock().unwrap().remove(session_id);
        self.hub.publish(Event::SessionStatusChanged {
            session_id: session_id.into(),
            status: SessionStatus::Running,
        });
        wait
    }

    /// Moves the turn onto a model the user switched to, and makes it, and any variant chosen with it, the session's from now on.
    pub(super) fn adopt(&self, plan: &mut Plan, switch: Switch) {
        let Switch { resolved, variant } = switch;
        if let Some(variant) = variant {
            let _ = self.store.set_session_variant(&plan.session.id, variant.as_deref());
            plan.variant = variant.or_else(|| {
                plan.config
                    .agent(&plan.session.agent)
                    .and_then(|agent| agent.variant.clone())
            });
        }

        plan.model_ref = resolved.model_ref;
        plan.model = resolved.model;
        if let Some(running) = self.turns.steering.lock().unwrap().get_mut(&plan.session.id) {
            *running = Steering::of(plan);
        }
        plan.provider = resolved
            .provider
            .with_timeouts(plan.config.route_timeouts(&plan.model_ref.provider));
        plan.credential = resolved.credential;

        // A different model may need a different set of tools.
        plan.offer = self.offer(plan);
        if let Ok(Some(session)) = self
            .store
            .update_session(&plan.session.id, None, Some(&plan.model_ref), None)
        {
            self.hub.publish(Event::SessionUpdated { session });
        }
    }

    /// Switches a turn that is waiting to retry onto `model`. The model and its credential are checked
    /// here, so a bad choice fails for the caller instead of inside the turn.
    pub async fn switch_retry_model(
        &self,
        session_id: &str,
        model: &ModelRef,
        variant: Option<Option<String>>,
    ) -> Result<(), TurnError> {
        if !self.turns.retry_waits.lock().unwrap().contains_key(session_id) {
            return Err(TurnError::NotRetrying);
        }

        let running = self
            .turns
            .steering
            .lock()
            .unwrap()
            .get(session_id)
            .cloned()
            .ok_or(TurnError::NotRetrying)?;
        let resolved = self.resolve_from(model, &running.catalog).await?;
        let waiting = self
            .turns
            .retry_waits
            .lock()
            .unwrap()
            .remove(session_id)
            .ok_or(TurnError::NotRetrying)?;

        waiting
            .send(Switch { resolved, variant })
            .map_err(|_| TurnError::NotRetrying)
    }
}
