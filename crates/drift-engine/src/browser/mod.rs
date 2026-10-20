//! One browser per conversation, owned by the engine: started by the `browser` tool, streamed into the pane,
//! and ended by Stop, archiving, quitting or 30 idle minutes. `docs/browser.md` has the design.

mod agent;
mod cdp;
mod input;
mod instance;
mod launch;
mod page;
mod profiles;
mod view;
pub(crate) mod viewport;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::Engine;
use crate::event::Event;
pub(crate) use input::parse_chord;
pub use input::{KeyInput, MouseInput};
pub(crate) use instance::{Instance, go, with_scheme};
pub(crate) use page::{Page, Settled};
pub use view::{Frame, ViewInput};

/// Settings: whether agents get the `browser` tool, and how its browsers start.
pub const SETTINGS_KEY: &str = "browser";

/// A browser no tool has used for this long, with no pane watching, is ended.
const IDLE_LIMIT: Duration = Duration::from_secs(30 * 60);
const IDLE_CHECK: Duration = Duration::from_secs(60);
/// The longest a new browser waits for the conversation's last one to finish ending.
const ENDING_WAIT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = BrowserKind)]
pub enum Kind {
    #[default]
    Chrome,
    Edge,
}

/// Whether a conversation's profile is its own or its workspace's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = BrowserStorage)]
pub enum Storage {
    #[default]
    Isolated,
    Shared,
}

/// Headless, or a real window placed off-screen for sites that check how a page renders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = BrowserWindow)]
pub enum Window {
    #[default]
    Hidden,
    Offscreen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserState {
    Running,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSettings {
    pub enabled: bool,
    pub engine: Kind,
    pub storage: Storage,
    pub window: Window,
}

impl Default for BrowserSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            engine: Kind::Chrome,
            storage: Storage::Isolated,
            window: Window::Hidden,
        }
    }
}

/// An alert, confirm or prompt waiting on a page; nothing else runs on it until it is answered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogRecord {
    pub kind: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_prompt: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PageRecord {
    pub id: String,
    pub url: String,
    pub title: String,
    pub active: bool,
    /// Opened by a page (a popup or a link to a new tab), not by the agent.
    pub popup: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dialog: Option<DialogRecord>,
}

/// A conversation's browser as the pane and the API show it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserRecord {
    pub session_id: String,
    pub engine: Kind,
    pub storage: Storage,
    pub window: Window,
    pub state: BrowserState,
    pub started_at: i64,
    pub pages: Vec<PageRecord>,
}

impl Kind {
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Edge => "edge",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Chrome => "Chrome",
            Self::Edge => "Edge",
        }
    }

    fn other(self) -> Self {
        match self {
            Self::Chrome => Self::Edge,
            Self::Edge => Self::Chrome,
        }
    }
}

/// The address of the page `page` names in a browser, or of its active page.
pub(crate) fn instance_url(instance: &Instance, page: Option<&str>) -> Option<String> {
    instance.page(page).ok().map(|page| page.url())
}

/// The first of Chrome and Edge this machine has.
pub fn installed() -> Option<Kind> {
    [Kind::Chrome, Kind::Edge]
        .into_iter()
        .find(|kind| launch::executable(*kind).is_some())
}

/// The running browsers, one per conversation at most.
#[derive(Default)]
pub struct Browsers {
    all: Mutex<HashMap<String, Arc<Instance>>>,
    /// Held while a browser starts, so two calls never start two for one conversation.
    starting: tokio::sync::Mutex<()>,
}

impl Browsers {
    pub(crate) fn get(&self, session_id: &str) -> Option<Arc<Instance>> {
        self.all.lock().unwrap().get(session_id).cloned()
    }

    pub fn record(&self, session_id: &str) -> Option<BrowserRecord> {
        self.get(session_id).map(|instance| instance.record())
    }

    /// Ends a conversation's browser, as when it is archived or its subagent run ends.
    pub fn stop_session(&self, session_id: &str) {
        if let Some(instance) = self.get(session_id) {
            instance.stop();
        }
    }

    /// Ends every browser now, as Drift quits; the job objects would end them anyway on Windows.
    pub fn stop_all(&self) {
        for instance in self.all.lock().unwrap().values() {
            instance.kill();
        }
    }

    fn running_sessions(&self) -> HashSet<String> {
        self.all.lock().unwrap().keys().cloned().collect()
    }

    /// Forgets an instance that ended, unless a newer one already took its place.
    fn remove(&self, instance: &Arc<Instance>) {
        let mut all = self.all.lock().unwrap();
        if all
            .get(&instance.session_id)
            .is_some_and(|current| Arc::ptr_eq(current, instance))
        {
            all.remove(&instance.session_id);
        }
    }
}

impl Engine {
    pub fn browser_settings(&self) -> BrowserSettings {
        self.store.setting(SETTINGS_KEY).ok().flatten().unwrap_or_default()
    }

    pub fn set_browser_settings(&self, settings: &BrowserSettings) -> rusqlite::Result<()> {
        self.store.set_setting(SETTINGS_KEY, settings)
    }

    /// The conversation's browser, started now if it has none; the note says when it is not quite what Settings
    /// asked for (another engine, or an isolated profile while the shared one is in use).
    pub(crate) async fn open_browser(&self, session_id: &str) -> Result<(Arc<Instance>, Option<String>), String> {
        let _starting = self.browsers.starting.lock().await;
        if let Some(instance) = self.running_browser(session_id).await {
            return Ok((instance, None));
        }

        let settings = self.browser_settings();
        let mut notes = Vec::new();
        let kind = self.installed_kind(settings.engine, &mut notes)?;
        let (storage, profile) = self.profile_for(session_id, kind, settings.storage, &mut notes);

        let spec = instance::Spec {
            session_id: session_id.into(),
            kind,
            storage,
            window: settings.window,
            profile,
        };
        let instance = Instance::start(self.me.clone(), spec).await?;
        self.browsers
            .all
            .lock()
            .unwrap()
            .insert(session_id.into(), instance.clone());
        instance.mark_listed();
        self.publish_browser(&instance);

        let note = (!notes.is_empty()).then(|| notes.join(" "));

        Ok((instance, note))
    }

    /// The conversation's browser if it is running; one still ending is waited out, so its replacement starts clean.
    async fn running_browser(&self, session_id: &str) -> Option<Arc<Instance>> {
        let deadline = tokio::time::Instant::now() + ENDING_WAIT;
        loop {
            let instance = self.browsers.get(session_id)?;
            if !instance.is_ended() || tokio::time::Instant::now() >= deadline {
                return Some(instance);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// The engine Settings chose, or the other one when it is not installed.
    fn installed_kind(&self, wanted: Kind, notes: &mut Vec<String>) -> Result<Kind, String> {
        if launch::executable(wanted).is_some() {
            return Ok(wanted);
        }

        let other = wanted.other();
        if launch::executable(other).is_none() {
            return Err("Neither Chrome nor Edge is installed, so there is no browser to open.".into());
        }
        notes.push(format!(
            "{} is not installed, so this browser is {}.",
            wanted.label(),
            other.label()
        ));

        Ok(other)
    }

    /// A shared profile in use by another conversation cannot open twice, so this one gets its own.
    fn profile_for(
        &self,
        session_id: &str,
        kind: Kind,
        wanted: Storage,
        notes: &mut Vec<String>,
    ) -> (Storage, std::path::PathBuf) {
        let isolated = (Storage::Isolated, profiles::isolated(&self.data_dir, session_id, kind));
        if wanted == Storage::Isolated {
            return isolated;
        }

        let workspace = self
            .store
            .session(session_id)
            .ok()
            .flatten()
            .and_then(|session| self.store.workspace(&session.workspace_id).ok().flatten());
        let Some(workspace) = workspace else {
            return isolated;
        };

        let shared = profiles::shared(&self.data_dir, &workspace, kind);
        let in_use = self
            .browsers
            .all
            .lock()
            .unwrap()
            .values()
            .any(|instance| instance.profile == shared);
        if in_use {
            notes.push(
                "Another conversation of this workspace has the shared profile open, so this browser has its own and \
                 starts signed out."
                    .into(),
            );
            return isolated;
        }

        (Storage::Shared, shared)
    }

    pub(crate) fn publish_browser(&self, instance: &Instance) {
        self.hub.publish(Event::BrowserUpdated {
            browser: instance.record(),
        });
    }

    /// An instance's browser has gone: forget it, tell clients, and tidy its profile.
    /// It stays listed until its profile is tidied, so a new browser never starts on a folder being deleted.
    pub(crate) async fn browser_ended(&self, instance: &Arc<Instance>) {
        match instance.storage {
            Storage::Isolated => profiles::remove(&instance.profile).await,
            Storage::Shared => profiles::trim(&instance.profile).await,
        }

        self.browsers.remove(instance);
        self.publish_browser(instance);
    }

    /// Ends idle browsers once a minute and, at start, deletes profiles a crash left behind.
    pub(crate) async fn watch_browsers(self: Arc<Self>) {
        let engine = Arc::downgrade(&self);
        if let Ok(workspaces) = self.store.workspaces() {
            profiles::sweep(&self.data_dir, &self.browsers.running_sessions(), &workspaces);
        }
        drop(self);

        loop {
            tokio::time::sleep(IDLE_CHECK).await;
            let Some(engine) = engine.upgrade() else { return };

            let idle: Vec<_> = engine
                .browsers
                .all
                .lock()
                .unwrap()
                .values()
                .filter(|instance| instance.idle_for() > IDLE_LIMIT && !instance.watched())
                .cloned()
                .collect();
            for instance in idle {
                instance.stop();
            }
        }
    }
}
