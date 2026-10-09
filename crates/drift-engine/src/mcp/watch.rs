use std::path::Path;
use std::sync::{Arc, Weak};
use std::time::Duration;

use super::{
    Connecting, Error, FIRST_RETRY, IDLE, IDLE_SWEEP, Key, Live, MAX_RETRY, STABLE, Start, WATCH_INTERVAL, Watch,
};
use crate::event::Event;

/// Resets reconnect backoff after a stable connection; short-lived connections keep the accumulated wait.
pub(super) fn after_loss(lived: Duration, carried: Duration) -> Duration {
    if lived >= STABLE { FIRST_RETRY } else { carried }
}

impl crate::Engine {
    /// [`Self::connect_mcp_in`] without a workspace, for remote servers or existing stdio connections.
    pub async fn connect_mcp(self: &Arc<Self>, name: &str) -> Result<(), Error> {
        self.connect_mcp_in(name, None).await
    }

    /// Connects a remote server's shared connection or a stdio server's workspace connections concurrently.
    /// The active workspace joins every workspace where that stdio server already runs.
    /// With no workspace available, stdio is refused with [`super::NEEDS_WORKSPACE`].
    pub async fn connect_mcp_in(self: &Arc<Self>, name: &str, workspace: Option<&Path>) -> Result<(), Error> {
        let stdio = self
            .store
            .mcp_server(name)
            .ok()
            .flatten()
            .is_some_and(|row| !row.config.is_remote());
        if !stdio {
            return self.connect_mcp_at(&Key::shared(name), Start::User, FIRST_RETRY).await;
        }

        let mut keys = self.mcp.lock().keys_of(name);
        keys.extend(workspace.map(|workspace| Key::of(name, Some(workspace))));
        keys.retain(|key| key.workspace.is_some());
        keys.sort();
        keys.dedup();
        if keys.is_empty() {
            return Err(Error::NeedsWorkspace);
        }

        let connected = futures_util::future::join_all(
            keys.iter()
                .map(|key| self.connect_mcp_at(key, Start::User, FIRST_RETRY)),
        )
        .await;

        connected.into_iter().find(Result::is_err).unwrap_or(Ok(()))
    }

    /// `backoff` is the wait before reconnecting if this connection drops before it proves stable.
    async fn connect_mcp_at(self: &Arc<Self>, key: &Key, start: Start, backoff: Duration) -> Result<(), Error> {
        let live = self.mcp.connect(key, &self.store, &self.hub, start).await?;
        self.watch_if_open(key, &live, backoff);

        Ok(())
    }

    fn watch_if_open(self: &Arc<Self>, key: &Key, live: &Arc<Live>, backoff: Duration) {
        if !live.holds_nothing_open() {
            self.watch_mcp(key.clone(), Arc::downgrade(live), backoff);
        }
    }

    /// Rechecks stateless clients after failed calls and replaces a connection that ended.
    pub(super) fn recheck_mcp(self: &Arc<Self>, key: &Key, live: &Arc<Live>) {
        if let Watch::Lost { generation, lived } = self.mcp.check(key, &Arc::downgrade(live)) {
            self.lost_mcp(&key.server);
            let reconnect = self
                .clone()
                .reconnect_mcp(key.clone(), generation, after_loss(lived, FIRST_RETRY));
            tokio::spawn(reconnect);
        }
    }

    /// Begins each enabled remote server independently; `wait_ready` observes every attempt immediately.
    /// Stdio starts when a workspace turn is planned.
    /// [`Self::start_workspace_mcp`] handles that workspace-specific startup.
    pub fn connect_all_mcp(self: &Arc<Self>) {
        let Ok(rows) = self.store.mcp_servers() else { return };

        for row in rows
            .into_iter()
            .filter(|row| row.on_anywhere() && row.config.is_remote())
        {
            self.begin_mcp(Key::shared(&row.name));
        }
    }

    /// Starts each enabled stdio server's connection for `workspace` that is not live, connecting or failed.
    pub(crate) fn start_workspace_mcp(self: &Arc<Self>, workspace: &Path) {
        let Ok(rows) = self.store.mcp_servers() else { return };

        for row in rows
            .into_iter()
            .filter(|row| !row.config.is_remote() && row.on_in(workspace))
        {
            self.begin_mcp(Key::of(&row.name, Some(workspace)));
        }
    }

    /// Stops the stdio servers running in a workspace the user removed; using it again starts them again.
    pub fn stop_workspace_mcp(&self, workspace_id: &str) {
        if let Ok(Some(workspace)) = self.store.workspace(workspace_id) {
            let directory = crate::tool::canonical(Path::new(&workspace.path));
            self.mcp.stop_workspace(&directory, &self.store, &self.hub);
        }
    }

    /// Every `IDLE_SWEEP`, stops workspace connections left unused for `IDLE`, until the engine is gone.
    pub async fn stop_idle_mcp(self: Arc<Self>) {
        let engine = Arc::downgrade(&self);
        drop(self);
        let mut interval = tokio::time::interval(IDLE_SWEEP);

        loop {
            interval.tick().await;
            let Some(engine) = engine.upgrade() else { return };
            engine.mcp.stop_idle(IDLE, &engine.store, &engine.hub);
        }
    }

    fn begin_mcp(self: &Arc<Self>, key: Key) {
        let Ok((row, attempt)) = self.mcp.begin(&key, &self.store, Start::Startup) else {
            return;
        };

        let engine = self.clone();
        tokio::spawn(async move {
            let connecting = Connecting {
                key: key.clone(),
                attempt,
            };
            if let Ok(live) = engine.mcp.complete(row, connecting, &engine.store, &engine.hub).await {
                engine.watch_if_open(&key, &live, FIRST_RETRY);
            }
        });
    }

    /// Reconnects a connection that ends by itself; once it is no longer the connection at `key`, the watch ends.
    fn watch_mcp(self: &Arc<Self>, key: Key, live: Weak<Live>, backoff: Duration) {
        let engine = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(WATCH_INTERVAL).await;
                let Some(engine) = engine.upgrade() else { return };

                match engine.mcp.check(&key, &live) {
                    Watch::Holding => {}
                    Watch::Gone => return,
                    Watch::Lost { generation, lived } => {
                        engine.lost_mcp(&key.server);
                        return engine.reconnect_mcp(key, generation, after_loss(lived, backoff)).await;
                    }
                }
            }
        });
    }

    fn lost_mcp(&self, name: &str) {
        if let Ok(Some(row)) = self.store.mcp_server(name) {
            self.hub.publish(Event::McpUpdated {
                server: self.mcp.status_of(row),
            });
        }
    }

    /// Tries again with growing waits until it connects, someone else connects it, or its generation changes.
    async fn reconnect_mcp(self: Arc<Self>, key: Key, generation: u64, mut wait: Duration) {
        let engine = Arc::downgrade(&self);
        drop(self);

        loop {
            tokio::time::sleep(wait).await;
            let Some(engine) = engine.upgrade() else { return };
            if engine.mcp.generation_of(&key) != generation || engine.mcp.is_live(&key) {
                return;
            }

            wait = (wait * 2).min(MAX_RETRY);
            if engine
                .connect_mcp_at(&key, Start::Reconnect(generation), wait)
                .await
                .is_ok()
            {
                return;
            }
        }
    }
}
