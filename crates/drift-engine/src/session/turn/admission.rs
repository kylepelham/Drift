use super::*;

/// How long a prompt waits for a job that is not a turn (a compaction, an undo) before it is refused.
#[cfg(not(test))]
const QUEUE_WAIT: Duration = Duration::from_secs(30);
#[cfg(test)]
const QUEUE_WAIT: Duration = Duration::from_secs(2);

struct StartingTurn<'a> {
    plan: Plan,
    abort: CancellationToken,
    payload_hash: &'a str,
    admission: Admission<'a>,
}

impl Engine {
    /// Records the prompt and starts the turn in the background; the receipt is what was recorded.
    pub async fn submit(self: &Arc<Self>, session_id: &str, prompt: Prompt) -> Result<Receipt, TurnError> {
        self.submit_under(session_id, prompt, None).await
    }

    /// A turn whose abort token descends from parent, so aborting the parent aborts it however the wait ends.
    pub async fn submit_under(
        self: &Arc<Self>,
        session_id: &str,
        prompt: Prompt,
        parent: Option<&CancellationToken>,
    ) -> Result<Receipt, TurnError> {
        self.admit(
            session_id,
            prompt,
            Admission {
                parent,
                ..Admission::default()
            },
        )
        .await
    }

    /// A replay found inside admission's own write is the original receipt, as one found before it is.
    pub(in crate::session) async fn admit(
        self: &Arc<Self>,
        session_id: &str,
        prompt: Prompt,
        how: Admission<'_>,
    ) -> Result<Receipt, TurnError> {
        match self.admit_once(session_id, prompt, how).await {
            Err(TurnError::Replayed(message_id)) => self.receipt_for(session_id, &message_id),
            other => other,
        }
    }

    async fn admit_once(
        self: &Arc<Self>,
        session_id: &str,
        prompt: Prompt,
        how: Admission<'_>,
    ) -> Result<Receipt, TurnError> {
        let payload_hash = payload_hash(&prompt);
        if let Some(id) = prompt.submission_id.as_deref()
            && let Some(receipt) = self.replayed_receipt(id, session_id, &payload_hash)?
        {
            return Ok(receipt);
        }

        // Claim the session before planning so a workspace move cannot invalidate the plan.
        let abort = how
            .parent
            .map_or_else(CancellationToken::new, CancellationToken::child_token);
        if abort.is_cancelled() {
            return Err(TurnError::Stopped);
        }
        if !self.turns.claim(session_id, &abort) {
            if !how.bootstrap.is_empty() || how.turn_only {
                return Err(TurnError::Busy);
            }
            return self.steer_or_wait(session_id, prompt, how, &payload_hash).await;
        }
        if how.steer_only {
            self.turns.release(session_id);
            return Err(TurnError::Stopped);
        }

        let planned = tokio::select! {
            planned = self.plan_for(session_id, &prompt, how.turn_only, how.config) => planned,
            () = abort.cancelled() => Err(TurnError::Stopped),
        };
        match planned {
            Ok(mut plan) => {
                plan.bootstrap = how.bootstrap.to_vec();
                let prompt = match self.hook_prompt(&plan, prompt, &how).await {
                    Ok(prompt) => prompt,
                    Err(error) => {
                        self.turns.release(session_id);
                        return Err(error);
                    }
                };
                let start = StartingTurn {
                    plan,
                    abort,
                    payload_hash: &payload_hash,
                    admission: how,
                };

                self.start(session_id, prompt, start)
            }
            Err(error) => {
                self.turns.release(session_id);
                Err(error)
            }
        }
    }

    /// Starts a turn on a queued worker's plan as admitted; only the credential is looked up afresh.
    pub(in crate::session) async fn submit_planned(
        self: &Arc<Self>,
        session_id: &str,
        prompt: Prompt,
        mut plan: Plan,
        parent: &CancellationToken,
    ) -> Result<Receipt, TurnError> {
        let abort = parent.child_token();
        loop {
            if abort.is_cancelled() {
                return Err(TurnError::Stopped);
            }
            if self.turns.claim(session_id, &abort) {
                break;
            }
            self.turns.wait_idle(session_id, &abort).await;
        }

        let ready = tokio::select! {
            ready = self.refresh_plan(&mut plan) => ready,
            () = abort.cancelled() => Err(TurnError::Stopped),
        };
        if let Err(error) = ready {
            self.turns.release(session_id);
            return Err(error);
        }

        let hash = payload_hash(&prompt);
        let start = StartingTurn {
            plan,
            abort,
            payload_hash: &hash,
            admission: Admission::default(),
        };
        self.start(session_id, prompt, start)
    }

    /// Admits the prompt into a session this call has claimed and starts its turn; releases the claim if it cannot.
    fn start(
        self: &Arc<Self>,
        session_id: &str,
        prompt: Prompt,
        start: StartingTurn<'_>,
    ) -> Result<Receipt, TurnError> {
        let StartingTurn {
            plan,
            abort,
            payload_hash,
            admission: how,
        } = start;
        let attach = Attach {
            engine: self,
            session_id,
            workspace: &plan.workspace,
            policy: &plan.config.policy(),
            agent_policy: &plan.config.agent_policy(&plan.session.agent),
            model: &plan.model,
        };
        let submission = prompt.submission_id.as_deref().map(|id| (id, payload_hash));
        let pick = Pick {
            model: &plan.model_ref,
            variant: prompt.variant.as_ref().map(Option::as_deref),
            agent: prompt.agent.as_deref(),
            sticky: !plan.turn_only,
        };

        let admitted = attach.prepare(prompt.parts).and_then(|prepared| {
            let prompt = FencedPrompt {
                pick,
                parts: prepared.parts,
                submission,
                abort: Some(&abort),
                delivery: how.delivery,
            };
            let admitted = self.admit_fenced(session_id, prompt)?;
            self.count_as_read(session_id, &prepared.read);

            Ok(admitted)
        });
        let admitted = match admitted {
            Ok(admitted) => admitted,
            Err(error) => {
                self.turns.release(session_id);
                return Err(error);
            }
        };

        let receipt = self.announce(session_id, admitted);
        self.turns
            .began
            .lock()
            .unwrap()
            .insert(session_id.into(), receipt.message.id.clone());
        self.turns
            .steering
            .lock()
            .unwrap()
            .insert(session_id.into(), Steering::of(&plan));
        let engine = self.clone();
        self.spawn_job(session_id, async move { engine.run(plan, abort).await });

        Ok(receipt)
    }

    /// Checks admission under the same fence as Stop, so a prompt lands wholly before Stop or not at all.
    pub(in crate::session) fn admit_fenced(
        &self,
        session_id: &str,
        prompt: FencedPrompt<'_>,
    ) -> Result<Admitted, TurnError> {
        let FencedPrompt {
            pick,
            parts,
            submission,
            abort,
            delivery,
        } = prompt;
        let _fence = self.workers.fence();
        if abort.is_some_and(CancellationToken::is_cancelled) {
            return Err(TurnError::Stopped);
        }

        // Claim held results until admission finishes so another delivery cannot take the same result.
        let held: Vec<_> = self
            .store
            .held_tasks(session_id)?
            .into_iter()
            .filter(|task| self.workers.claim(&task.id, Claimant::Automatic))
            .collect();
        let carried = held
            .iter()
            .map(|task| (task.id.clone(), tasks::result_part(task)))
            .collect();
        let handover = Handover {
            delivery,
            held: carried,
        };
        let admission = crate::store::Admission {
            pick,
            parts,
            submission,
            handover,
        };
        let admitted = self.store.admit_delivering(session_id, admission);
        for task in &held {
            self.workers.release_where_task(&task.id, &Claimant::Automatic);
            self.publish_task(&task.id);
        }

        match admitted? {
            Admit::New(admitted) => Ok(*admitted),
            Admit::Replayed { message_id } => Err(TurnError::Replayed(message_id)),
            Admit::Conflict | Admit::Delivered => Err(TurnError::SubmissionReused),
        }
    }

    /// A running turn takes a prompt after its current calls finish and follows any choice the prompt names.
    /// Other jobs are waited out for up to [`QUEUE_WAIT`], then a new turn starts.
    /// Steering-only admissions never start a new turn.
    async fn steer_or_wait(
        self: &Arc<Self>,
        session_id: &str,
        prompt: Prompt,
        how: Admission<'_>,
        payload_hash: &str,
    ) -> Result<Receipt, TurnError> {
        if let Some(model) = &prompt.model {
            let running = self.turns.steering.lock().unwrap().get(session_id).cloned();
            if let Some(running) = running.filter(|running| running.model != *model) {
                self.resolve_from(model, &running.catalog).await?;
            }
        }
        if let Some(receipt) = self.steer(session_id, &prompt, payload_hash, how)? {
            return Ok(receipt);
        }
        if how.steer_only {
            return Err(TurnError::Stopped);
        }

        let never = CancellationToken::new();
        let stop = how.parent.unwrap_or(&never);
        let wait = tokio::time::timeout(QUEUE_WAIT, self.turns.wait_idle(session_id, stop)).await;
        if wait.is_err() {
            return Err(TurnError::Busy);
        }
        if stop.is_cancelled() {
            return Err(TurnError::Stopped);
        }

        Box::pin(self.admit(session_id, prompt, how)).await
    }

    /// Admits a prompt into a running turn only while it still accepts prompts.
    /// Files are validated against the prompt's model, or the model the turn will request next.
    /// A concurrent model switch causes validation to restart.
    fn steer(
        &self,
        session_id: &str,
        prompt: &Prompt,
        payload_hash: &str,
        how: Admission<'_>,
    ) -> Result<Option<Receipt>, TurnError> {
        let Some(running) = self.turns.steering.lock().unwrap().get(session_id).cloned() else {
            return Ok(None);
        };

        let session = if running.turn_only {
            self.store.session(session_id)?
        } else {
            None
        };
        let (own_model, own_agent) = running.defaults(session.as_ref());
        let target = prompt.model.clone().unwrap_or(own_model);
        let model = running
            .catalog
            .providers
            .get(&target.provider)
            .and_then(|provider| provider.models.get(&target.model))
            .cloned()
            .ok_or(TurnError::UnknownModel)?;

        let config = &running.config;
        if let Some(agent) = &prompt.agent {
            pickable(config, agent)?;
        }
        let policy = config.policy();
        let agent_policy = config.agent_policy(prompt.agent.as_deref().unwrap_or(&own_agent));
        let attach = Attach {
            engine: self,
            session_id,
            workspace: &running.workspace,
            policy: &policy,
            agent_policy: &agent_policy,
            model: &model,
        };
        let prepared = attach.prepare(prompt.parts.clone())?;

        let steering = self.turns.steering.lock().unwrap();
        match steering.get(session_id) {
            None => return Ok(None),
            Some(now) if *now != running => {
                drop(steering);
                return self.steer(session_id, prompt, payload_hash, how);
            }
            Some(_) => {}
        }
        let submission = prompt.submission_id.as_deref().map(|id| (id, payload_hash));
        let pick = Pick {
            model: &target,
            variant: prompt.variant.as_ref().map(Option::as_deref),
            agent: prompt.agent.as_deref(),
            sticky: true,
        };
        let prompt = FencedPrompt {
            pick,
            parts: prepared.parts,
            submission,
            abort: how.parent,
            delivery: how.delivery,
        };
        let admitted = self.admit_fenced(session_id, prompt)?;
        drop(steering);
        self.count_as_read(session_id, &prepared.read);

        Ok(Some(self.announce(session_id, admitted)))
    }

    /// Files a prompt showed in full count as read once it is admitted, not before: a refused prompt showed nothing.
    fn count_as_read(&self, session_id: &str, paths: &[PathBuf]) {
        let files = self.turns.files_for(&self.store, session_id);
        for path in paths {
            files.mark_read(path);
        }
    }
}
