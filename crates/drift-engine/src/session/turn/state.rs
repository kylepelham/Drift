use super::*;

impl Turns {
    /// The recorded end of a subagent's last turn; each end is handed out once.
    pub fn take_end(&self, session_id: &str) -> Option<TurnEnd> {
        self.ended.lock().unwrap().remove(session_id)
    }

    /// Marks the session busy under `abort`; `false` if a turn or job already holds it.
    pub(in crate::session) fn claim(&self, session_id: &str, abort: &CancellationToken) -> bool {
        let mut active = self.active.lock().unwrap();
        if active.contains_key(session_id) {
            return false;
        }

        active.insert(session_id.into(), abort.clone());
        true
    }

    /// Frees a claim whose turn never started; anyone waiting for the session to go idle wakes.
    pub(in crate::session) fn release(&self, session_id: &str) {
        self.active.lock().unwrap().remove(session_id);
        self.finished.notify_waiters();
    }

    pub(in crate::session) fn cancellation(&self, session_id: &str) -> CancellationToken {
        self.active.lock().unwrap().get(session_id).cloned().unwrap_or_default()
    }

    /// Runs `change` only if none of `ids` is claimed, holding claims off until it returns.
    pub(in crate::session) fn while_idle<T>(&self, ids: &[String], change: impl FnOnce() -> T) -> Option<T> {
        let active = self.active.lock().unwrap();
        if ids.iter().any(|id| active.contains_key(id)) {
            return None;
        }

        let result = change();
        drop(active);
        Some(result)
    }

    /// The prompt the session's running turn began at, if a turn is running.
    pub(in crate::session) fn began(&self, session_id: &str) -> Option<String> {
        self.began.lock().unwrap().get(session_id).cloned()
    }

    /// Loads the session's persisted read record on first access.
    pub(in crate::session) fn files_for(
        &self,
        store: &Arc<crate::store::Store>,
        session_id: &str,
    ) -> Arc<SessionFiles> {
        let mut files = self.files.lock().unwrap();
        let record = files.entry(session_id.into());

        record
            .or_insert_with(|| Arc::new(SessionFiles::kept(store.clone(), session_id)))
            .clone()
    }

    /// Clears remembered check output after compaction or undo, so the next report is sent in full.
    pub(in crate::session) fn forget_checked(&self, session_id: &str) {
        self.checked.lock().unwrap().remove(session_id);
    }

    /// Records what a check said (`None` when it passed); whether the session was told exactly this last time.
    pub(super) fn repeated(&self, session_id: &str, label: &str, said: Option<&str>) -> bool {
        let mut checked = self.checked.lock().unwrap();
        let session = checked.entry(session_id.into()).or_default();

        match said {
            Some(said) => session.insert(label.into(), said.into()).as_deref() == Some(said),
            None => {
                session.remove(label);
                false
            }
        }
    }

    pub(in crate::session) fn refresh_lock(&self, provider: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.refreshing
            .lock()
            .unwrap()
            .entry(provider.into())
            .or_default()
            .clone()
    }

    pub fn is_running(&self, session_id: &str) -> bool {
        self.active.lock().unwrap().contains_key(session_id)
    }

    /// Whether any session has a job running.
    pub fn any_running(&self) -> bool {
        !self.active.lock().unwrap().is_empty()
    }

    /// Cancels whatever holds the session, if anything does.
    pub(in crate::session) fn cancel(&self, session_id: &str) -> bool {
        let active = self.active.lock().unwrap();

        active.get(session_id).inspect(|token| token.cancel()).is_some()
    }

    /// A turn is running in the session and still takes prompts sent to it.
    pub fn is_steerable(&self, session_id: &str) -> bool {
        self.steering.lock().unwrap().contains_key(session_id)
    }

    /// Resolves once the session has no turn in flight, or the caller is aborted.
    pub async fn wait_idle(&self, session_id: &str, abort: &CancellationToken) {
        loop {
            let notified = self.finished.notified();
            if !self.is_running(session_id) {
                return;
            }

            tokio::select! {
                () = notified => {}
                () = abort.cancelled() => return,
            }
        }
    }
}
