use std::path::Path;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use super::{Attempt, Key, Live, Shown, Slot, Slots, Transport};

impl Slot {
    pub(super) fn current(&self) -> Option<Arc<Live>> {
        self.current.lock().unwrap().clone()
    }

    pub(super) fn touch(&self) {
        *self.used.lock().unwrap() = Some(Instant::now());
    }

    pub(super) fn idle_for(&self, limit: Duration) -> bool {
        self.used.lock().unwrap().is_none_or(|used| used.elapsed() >= limit)
    }

    pub(super) fn publish(&self, live: Arc<Live>) {
        let mut served = self.served.lock().unwrap();
        served.retain(|client| client.strong_count() > 0);
        served.push(Arc::downgrade(&live));
        drop(served);

        *self.current.lock().unwrap() = Some(live);
        self.touch();
        self.published.notify_waiters();
    }

    pub(super) fn take(&self) -> Option<Arc<Live>> {
        self.current.lock().unwrap().take()
    }

    pub(super) fn close(&self) {
        self.closed.cancel();
        self.take();
        let served: Vec<_> = self.served.lock().unwrap().drain(..).collect();

        for live in served.iter().filter_map(Weak::upgrade) {
            live.close();
        }
    }

    pub(super) fn is_closed(&self) -> bool {
        self.closed.is_cancelled()
    }

    pub(super) async fn closing(&self) {
        self.closed.cancelled().await;
    }

    /// Uses the current connection only if it still serves the definition the turn was given.
    pub(super) fn client_for(&self, pinned: &Arc<Live>) -> Arc<Live> {
        self.touch();

        self.current()
            .filter(|current| current.hash == pinned.hash && current.is_open())
            .unwrap_or_else(|| pinned.clone())
    }

    /// Waits, at most `limit`, for a client of the same definition to take over from `lost`.
    pub(super) async fn replacement(&self, lost: &Arc<Live>, limit: Duration) -> Option<Arc<Live>> {
        let deadline = tokio::time::Instant::now() + limit;

        loop {
            // Register before checking the slot so publication cannot slip between the check and wait.
            let published = self.published.notified();
            if let Some(next) = self
                .current()
                .filter(|next| next.hash == lost.hash && !Arc::ptr_eq(next, lost) && next.is_open())
            {
                return Some(next);
            }

            tokio::select! {
                () = published => {}
                () = self.closed.cancelled() => return None,
                () = tokio::time::sleep_until(deadline) => return None,
            }
        }
    }
}

impl Slots {
    pub(super) fn generation_of(&self, key: &Key) -> u64 {
        self.generation.get(key).copied().unwrap_or(0)
    }

    pub(super) fn is_current(&self, key: &Key, attempt: &Attempt) -> bool {
        self.attempts.get(key).is_some_and(|current| current.id == attempt.id)
            && self.generation_of(key) == attempt.generation
    }

    pub(super) fn live(&self, key: &Key) -> Option<Arc<Live>> {
        self.servers.get(key).and_then(|slot| slot.current())
    }

    /// Every connection of `server`, including live, reconnecting, connecting and failed ones.
    pub(super) fn keys_of(&self, server: &str) -> Vec<Key> {
        let mut keys: Vec<Key> = self
            .servers
            .keys()
            .chain(self.attempts.keys())
            .chain(self.transient.keys())
            .filter(|key| key.server == server)
            .cloned()
            .collect();
        keys.sort();
        keys.dedup();

        keys
    }

    /// Finds the workspace's own connection, falling back only to a remote server's shared one.
    /// A disabled server is not offered; finding an allowed connection counts as a use.
    pub(super) fn live_for(&self, server: &str, workspace: Option<&Path>, shown: &Shown) -> Option<(Key, Arc<Live>)> {
        if !shown.allows(server) {
            return None;
        }

        let own = workspace.map(|workspace| Key::of(server, Some(workspace)));
        own.into_iter().chain([Key::shared(server)]).find_map(|key| {
            let slot = self.servers.get(&key)?;
            let live = slot
                .current()
                .filter(|live| key.workspace.is_some() || live.transport != Transport::Stdio)?;
            slot.touch();

            Some((key, live))
        })
    }

    /// Each server's live connection for `workspace`, by name.
    pub(super) fn lives_for(&self, workspace: Option<&Path>, shown: &Shown) -> Vec<(Key, Arc<Slot>, Arc<Live>)> {
        let mut names: Vec<&String> = self.servers.keys().map(|key| &key.server).collect();
        names.sort();
        names.dedup();

        names
            .into_iter()
            .filter_map(|name| {
                let (key, live) = self.live_for(name, workspace, shown)?;
                Some((key.clone(), self.servers.get(&key)?.clone(), live))
            })
            .collect()
    }
}
