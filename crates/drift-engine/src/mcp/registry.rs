use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, MutexGuard, Weak};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use super::connect::{SignIn, open};
use super::response::tool_info;
use super::{
    Attempt, Connecting, EndConnections, Ending, Era, Error, Failure, Key, Live, ServerRow, ServerStatus, Servers,
    Shown, Slots, Start, State, Transient, Transport, Watch, WorkspaceServer, unreadable,
};
use super::{ServerView, oauth};
use crate::event::{Event, Hub};
use crate::store::Store;

/// Removes a connect attempt and wakes waiters on every exit, including cancellation mid-flight.
struct Settle<'a> {
    servers: &'a Servers,
    hub: &'a Hub,
    row: &'a ServerRow,
    key: &'a Key,
    id: u64,
}

impl Servers {
    pub fn new(sign_ins: Arc<crate::llm::credentials::Credentials>, store: Arc<Store>) -> Self {
        Self {
            sign_ins: Some(sign_ins),
            store: Some(store),
            ..Self::default()
        }
    }

    pub(super) fn shown(&self, workspace: Option<&Path>) -> Shown {
        Shown::of(self.store.as_deref(), workspace)
    }

    pub(super) fn lock(&self) -> MutexGuard<'_, Slots> {
        self.slots.lock().unwrap()
    }

    /// Lists every saved server, including failed definitions this build cannot read.
    pub fn statuses(&self, store: &Store) -> rusqlite::Result<Vec<ServerStatus>> {
        let mut statuses: Vec<ServerStatus> = store
            .mcp_servers()?
            .into_iter()
            .map(|row| self.status_of(row))
            .collect();
        statuses.extend(store.unreadable_mcp_servers()?.into_iter().map(unreadable));
        statuses.sort_by(|first, second| first.server.name.cmp(&second.server.name));

        Ok(statuses)
    }

    /// Combines connection states into one server row: connected, then connecting, then its last failure.
    pub fn status_of(&self, row: ServerRow) -> ServerStatus {
        let slots = self.lock();
        let keys = slots.keys_of(&row.name);
        let live = keys.iter().find_map(|key| slots.live(key));
        let Transient {
            state,
            error,
            needs_sign_in,
        } = if !row.on_anywhere() {
            Transient::new(State::Disabled, None)
        } else if live.is_some() {
            Transient::new(State::Connected, None)
        } else {
            let states: Vec<&Transient> = keys.iter().filter_map(|key| slots.transient.get(key)).collect();
            states
                .iter()
                .find(|transient| transient.state == State::Connecting)
                .or(states.first())
                .map_or(Transient::new(State::Disconnected, None), |transient| {
                    (*transient).clone()
                })
        };

        let protocol = live
            .as_ref()
            .and_then(|live| live.service.peer_info())
            .map(|info| info.protocol_version.to_string());
        let era = live.as_ref().map(|live| live.era);
        let tools = live
            .map(|live| live.tools().iter().map(tool_info).collect())
            .unwrap_or_default();
        let needs_sign_in = needs_sign_in && state == State::Failed;
        let signed_in = row.config.is_remote()
            && self
                .sign_ins
                .as_ref()
                .is_some_and(|store| oauth::has_sign_in(store, &row.name));

        ServerStatus {
            transport: Transport::of(&row.config),
            protocol,
            era,
            needs_sign_in,
            signed_in,
            server: ServerView::of(&row),
            state,
            error,
            tools,
            unreadable: false,
        }
    }

    /// Reports an unfinished sign-in unless the server connected or began another connect meanwhile.
    pub(super) fn sign_in_failed(&self, name: &str, store: &Store, hub: &Hub, why: &str) {
        let key = Key::shared(name);
        let mut slots = self.lock();
        if slots.live(&key).is_some() || slots.attempts.contains_key(&key) {
            return;
        }

        slots.transient.insert(
            key,
            Transient {
                state: State::Failed,
                error: Some(format!("Sign-in did not finish: {why}")),
                needs_sign_in: true,
            },
        );
        drop(slots);

        self.publish_saved(name, store, hub);
    }

    /// Connects `key` using its server's current saved definition.
    pub(super) async fn connect(&self, key: &Key, store: &Store, hub: &Hub, start: Start) -> Result<Arc<Live>, Error> {
        let (row, attempt) = self.begin(key, store, start)?;
        let connecting = Connecting {
            key: key.clone(),
            attempt,
        };

        self.complete(row, connecting, store, hub).await
    }

    /// Opens a connect attempt and publishes its result unless the saved definition changed.
    pub(super) async fn complete(
        &self,
        row: ServerRow,
        connecting: Connecting,
        store: &Store,
        hub: &Hub,
    ) -> Result<Arc<Live>, Error> {
        let Connecting { key, attempt } = &connecting;
        let _settle = Settle {
            servers: self,
            hub,
            row: &row,
            key,
            id: attempt.id,
        };
        hub.publish(Event::McpUpdated {
            server: self.status_of(row.clone()),
        });

        let sign_in = SignIn {
            server: &row.name,
            credentials: self.sign_ins.as_ref(),
        };
        let opened = tokio::select! {
            opened = open(&row.config, row.hash.clone(), sign_in, row.era, key.workspace.as_deref()) => opened,
            () = attempt.cancel.cancelled() => Err(Failure::from("server definition changed during connect".to_string())),
        };

        let finished = self.finish(&row, &connecting, hub, opened);
        remember_era(store, &row, finished.as_ref().ok().map(|live| live.era));

        finished
    }

    /// Reads the definition and generation under one lock, so a save cannot slip between them.
    /// Automatic starts leave failed servers alone; an explicit user connect may retry them.
    pub(super) fn begin(&self, key: &Key, store: &Store, start: Start) -> Result<(ServerRow, Attempt), Error> {
        let mut slots = self.lock();
        let row = store.mcp_server(&key.server)?.ok_or(Error::NotFound)?;
        if row.config.is_remote() != key.workspace.is_none() {
            return Err(if key.workspace.is_none() {
                Error::NeedsWorkspace
            } else {
                Error::SharedRemote
            });
        }

        let generation = slots.generation_of(key);
        if matches!(start, Start::Reconnect(expected) if expected != generation) {
            return Err(Error::DefinitionChanged);
        }
        if start != Start::User && (slots.attempts.contains_key(key) || slots.live(key).is_some()) {
            return Err(Error::Busy);
        }
        if start == Start::Startup
            && slots
                .transient
                .get(key)
                .is_some_and(|transient| transient.state == State::Failed)
        {
            return Err(Error::RetryInSettings);
        }

        match start {
            Start::User => drop(slots.held.remove(&key.server)),
            _ if slots.held.contains(&key.server) => return Err(Error::Disconnected),
            _ => {}
        }
        if !key
            .workspace
            .as_deref()
            .map_or(row.on_anywhere(), |workspace| row.on_in(workspace))
        {
            return Err(if row.on_anywhere() {
                Error::OffWorkspace
            } else {
                Error::Disabled
            });
        }

        let attempt = Attempt {
            id: self.next_attempt.fetch_add(1, Ordering::Relaxed),
            generation,
            cancel: CancellationToken::new(),
        };
        if let Some(earlier) = slots.attempts.insert(key.clone(), attempt.clone()) {
            earlier.cancel.cancel();
        }
        slots
            .transient
            .insert(key.clone(), Transient::new(State::Connecting, None));

        Ok((row, attempt))
    }

    /// Publishes only the current attempt; dropping a superseded result kills the process it opened.
    fn finish(
        &self,
        row: &ServerRow,
        connecting: &Connecting,
        hub: &Hub,
        opened: Result<Live, Failure>,
    ) -> Result<Arc<Live>, Error> {
        let Connecting { key, attempt } = connecting;
        let mut slots = self.lock();
        if !slots.is_current(key, attempt) {
            return Err(Error::ConnectChanged);
        }

        slots.attempts.remove(key);
        let result = match opened {
            Ok(live) => {
                let live = Arc::new(live);
                slots.servers.entry(key.clone()).or_default().publish(live.clone());
                slots.transient.remove(key);

                Ok(live)
            }
            Err(Failure { message, needs_sign_in }) => {
                slots.transient.insert(
                    key.clone(),
                    Transient {
                        state: State::Failed,
                        error: Some(message.clone()),
                        needs_sign_in,
                    },
                );
                Err(Error::Connect { message, needs_sign_in })
            }
        };
        drop(slots);

        hub.publish(Event::McpUpdated {
            server: self.status_of(row.clone()),
        });

        result
    }

    /// Writes a definition and detaches its connections atomically; a refused write changes nothing.
    fn detach<R, E>(
        &self,
        name: &str,
        store: &Store,
        ending: Ending,
        write: impl FnOnce(&Store) -> Result<R, E>,
    ) -> Result<(Vec<Arc<Live>>, R), E> {
        self.detach_where(EndConnections { name, ending }, store, |_| true, write)
    }

    /// [`Self::detach`] for the connections `ends` picks.
    fn detach_where<R, E>(
        &self,
        connections: EndConnections<'_>,
        store: &Store,
        ends: impl Fn(&Key) -> bool,
        write: impl FnOnce(&Store) -> Result<R, E>,
    ) -> Result<(Vec<Arc<Live>>, R), E> {
        let EndConnections { name, ending } = connections;
        let mut slots = self.lock();
        let written = write(store)?;
        let mut lives = Vec::new();

        for key in slots.keys_of(name).into_iter().filter(|key| ends(key)) {
            *slots.generation.entry(key.clone()).or_default() += 1;
            if let Some(attempt) = slots.attempts.remove(&key) {
                attempt.cancel.cancel();
            }
            slots.transient.remove(&key);

            let slot = if ending == Ending::Close {
                slots.servers.remove(&key)
            } else {
                slots.servers.get(&key).cloned()
            };
            lives.extend(slot.as_ref().and_then(|slot| slot.take()));
            if ending == Ending::Close {
                slot.inspect(|slot| slot.close());
            }
        }

        Ok((lives, written))
    }

    /// Saves and detaches old connections in one step; running turns retain the client they were given.
    pub async fn change<R, E>(
        &self,
        name: &str,
        store: &Store,
        hub: &Hub,
        write: impl FnOnce(&Store) -> Result<R, E>,
    ) -> Result<R, E> {
        let (lives, written) = self.detach(name, store, Ending::Keep, write)?;
        self.retire(name, store, hub, lives).await;

        Ok(written)
    }

    /// As [`Self::change`], but closes every client the server served, including ones held by running turns.
    pub async fn close<R, E>(
        &self,
        name: &str,
        store: &Store,
        hub: &Hub,
        write: impl FnOnce(&Store) -> Result<R, E>,
    ) -> Result<R, E> {
        let (lives, written) = self.detach(name, store, Ending::Close, write)?;
        self.retire(name, store, hub, lives).await;

        Ok(written)
    }

    /// Ends every connection and blocks automatic restarts until the user explicitly connects again.
    pub async fn disconnect(&self, name: &str, store: &Store, hub: &Hub) -> bool {
        self.lock().held.insert(name.into());
        let Ok((lives, ())) = self.detach(name, store, Ending::Keep, |_| Ok::<_, rusqlite::Error>(())) else {
            return false;
        };

        let was_live = !lives.is_empty();
        self.retire(name, store, hub, lives).await;

        was_live
    }

    /// Records a workspace's off switch, then detaches only its connection.
    /// A remote server's shared connection ends only if no workspace still has it enabled.
    pub async fn disconnect_in<E>(
        &self,
        server: WorkspaceServer<'_>,
        store: &Store,
        hub: &Hub,
        write: impl FnOnce(&Store) -> Result<bool, E>,
    ) -> Result<bool, E> {
        let WorkspaceServer { name, workspace } = server;
        let ends = |key: &Key| match &key.workspace {
            Some(own) => own == workspace,
            None => !store
                .mcp_server(name)
                .ok()
                .flatten()
                .is_some_and(|row| row.on_anywhere()),
        };
        let connections = EndConnections {
            name,
            ending: Ending::Keep,
        };

        let (lives, found) = self.detach_where(connections, store, ends, write)?;
        self.retire(name, store, hub, lives).await;

        Ok(found)
    }

    /// Stops an idle or removed workspace connection; a later turn may start it again.
    /// Running turns keep their client, and in-flight connects are not disturbed.
    fn stop(&self, key: &Key, store: &Store, hub: &Hub) {
        let mut slots = self.lock();
        if slots.attempts.contains_key(key) {
            return;
        }

        *slots.generation.entry(key.clone()).or_default() += 1;
        slots.transient.remove(key);
        let live = slots.servers.remove(key).and_then(|slot| slot.take());
        drop(slots);
        drop(live);

        self.publish_saved(&key.server, store, hub);
    }

    /// Stops every connection running in `workspace`.
    pub fn stop_workspace(&self, workspace: &Path, store: &Store, hub: &Hub) {
        let keys: Vec<Key> = self
            .lock()
            .servers
            .keys()
            .filter(|key| key.workspace.as_deref() == Some(workspace))
            .cloned()
            .collect();

        for key in keys {
            self.stop(&key, store, hub);
        }
    }

    /// Tracks a socket's open workspace; closing the socket clears it.
    pub fn set_open(&self, socket: u64, workspace: Option<PathBuf>) {
        let mut open = self.open.lock().unwrap();
        match workspace {
            Some(workspace) => open.insert(socket, workspace),
            None => open.remove(&socket),
        };
    }

    pub(crate) fn is_open(&self, workspace: &Path) -> bool {
        self.open.lock().unwrap().values().any(|open| open == workspace)
    }

    /// Stops workspace connections unused for `limit` and not open in any client.
    /// Remote shared connections are not subject to workspace idleness.
    pub fn stop_idle(&self, limit: Duration, store: &Store, hub: &Hub) {
        let idle: Vec<Key> = self
            .lock()
            .servers
            .iter()
            .filter(|(key, slot)| {
                key.workspace
                    .as_deref()
                    .is_some_and(|workspace| !self.is_open(workspace))
                    && slot.idle_for(limit)
            })
            .map(|(key, _)| key.clone())
            .collect();

        for key in idle {
            self.stop(&key, store, hub);
        }
    }

    /// Cancels unshared clients now; clients held by running turns close when those turns release them.
    async fn retire(&self, name: &str, store: &Store, hub: &Hub, lives: Vec<Arc<Live>>) {
        for live in lives.into_iter().filter_map(|live| Arc::try_unwrap(live).ok()) {
            let _ = live.service.cancel().await;
        }
        self.settled.notify_waiters();

        self.publish_saved(name, store, hub);
    }

    fn publish_saved(&self, name: &str, store: &Store, hub: &Hub) {
        if let Ok(Some(row)) = store.mcp_server(name) {
            hub.publish(Event::McpUpdated {
                server: self.status_of(row),
            });
        }
    }

    pub(super) fn generation_of(&self, key: &Key) -> u64 {
        self.lock().generation_of(key)
    }

    pub(super) fn is_live(&self, key: &Key) -> bool {
        self.lock().live(key).is_some()
    }

    /// Whether any connect is in flight.
    pub fn connecting(&self) -> bool {
        !self.lock().attempts.is_empty()
    }

    /// Waits, at most `limit`, until no connect is in flight.
    pub async fn wait_ready(&self, limit: Duration) {
        let deadline = tokio::time::Instant::now() + limit;

        loop {
            let settled = self.settled.notified();
            if !self.connecting() {
                return;
            }

            tokio::select! {
                () = settled => {}
                () = tokio::time::sleep_until(deadline) => return,
            }
        }
    }

    /// Checks the current client and detaches it if its transport closed without a user disconnect.
    pub(super) fn check(&self, key: &Key, live: &Weak<Live>) -> Watch {
        let mut slots = self.lock();
        let Some(slot) = slots.servers.get(key).cloned() else {
            return Watch::Gone;
        };
        let Some(current) = slot.current().filter(|current| Arc::downgrade(current).ptr_eq(live)) else {
            return Watch::Gone;
        };
        if current.is_open() {
            return Watch::Holding;
        }

        slot.take();
        slots.transient.insert(
            key.clone(),
            Transient::new(State::Connecting, Some("the connection closed; reconnecting".into())),
        );

        Watch::Lost {
            generation: slots.generation_of(key),
            lived: current.since.elapsed(),
        }
    }
}

/// Keeps the discovered era, clearing it on failure so the next connect probes again.
fn remember_era(store: &Store, row: &ServerRow, found: Option<Era>) {
    if found != row.era {
        let _ = store.set_mcp_era(&row.name, &row.config, found);
    }
}

impl Drop for Settle<'_> {
    fn drop(&mut self) {
        let mut slots = self.servers.lock();
        let abandoned = slots
            .attempts
            .get(self.key)
            .is_some_and(|attempt| attempt.id == self.id);
        if abandoned {
            slots.attempts.remove(self.key);
            slots.transient.remove(self.key);
        }
        drop(slots);

        if abandoned {
            self.hub.publish(Event::McpUpdated {
                server: self.servers.status_of(self.row.clone()),
            });
        }
        self.servers.settled.notify_waiters();
    }
}
