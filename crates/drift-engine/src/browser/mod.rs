//! One browser per conversation, owned by the engine: started by the `browser` tool, streamed into the pane,
//! and ended by Stop, archiving, quitting or 30 idle minutes. `docs/browser.md` has the design.

mod bidi;
mod chromium;
pub(crate) mod driver;
pub mod engines;
mod firefox;
mod ie;
pub(crate) mod input;
pub(crate) mod lighthouse;
pub(crate) mod performance;
mod profiles;
pub mod project;
mod view;
mod webdriver;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::Engine;
use crate::event::Event;
use crate::store::Workspace;
use driver::{Driver, Spec};
pub use input::{KeyInput, MouseInput};
pub use project::BrowserProject;
pub use view::{Frame, ViewInput};

/// Settings: whether agents get the `browser` tool, and how its browsers start.
pub const SETTINGS_KEY: &str = "browser";

/// A browser no tool has used for this long, with no pane watching, is ended.
const IDLE_LIMIT: Duration = Duration::from_secs(30 * 60);
const IDLE_CHECK: Duration = Duration::from_secs(60);
/// The longest a new browser waits for the conversation's last one to finish ending.
const ENDING_WAIT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = BrowserKind)]
pub enum Kind {
    #[default]
    Chrome,
    Edge,
    Firefox,
    Camoufox,
    /// Edge's Internet Explorer mode, for sites that need Trident.
    Ie,
}

/// How an engine is driven.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Family {
    /// The DevTools Protocol.
    Chromium,
    /// WebDriver BiDi.
    Gecko,
    /// Classic WebDriver through IEDriverServer.
    Ie,
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
    pub(crate) const ALL: [Kind; 5] = [Kind::Chrome, Kind::Edge, Kind::Firefox, Kind::Camoufox, Kind::Ie];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Edge => "edge",
            Self::Firefox => "firefox",
            Self::Camoufox => "camoufox",
            Self::Ie => "ie",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Chrome => "Chrome",
            Self::Edge => "Edge",
            Self::Firefox => "Firefox",
            Self::Camoufox => "Camoufox",
            Self::Ie => "IE mode",
        }
    }

    pub(crate) fn family(self) -> Family {
        match self {
            Self::Chrome | Self::Edge => Family::Chromium,
            Self::Firefox | Self::Camoufox => Family::Gecko,
            Self::Ie => Family::Ie,
        }
    }

    pub(crate) fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.id() == text)
    }
}

/// An address as typed: a bare host gets `https://`, a local one `http://`.
pub(crate) fn with_scheme(url: &str) -> String {
    let known = [
        "http://",
        "https://",
        "file://",
        "about:",
        "data:",
        "chrome://",
        "edge://",
    ];
    if known.iter().any(|scheme| url.starts_with(scheme)) {
        return url.to_string();
    }
    let local = url.starts_with("localhost") || url.starts_with("127.") || url.starts_with("[::1]");

    if local {
        format!("http://{url}")
    } else {
        format!("https://{url}")
    }
}

/// The first of Chrome and Edge this machine has.
pub fn installed() -> Option<Kind> {
    [Kind::Chrome, Kind::Edge]
        .into_iter()
        .find(|kind| engines::executable(*kind).is_some())
}

/// The running browsers, one per conversation at most.
#[derive(Default)]
pub struct Browsers {
    all: Mutex<HashMap<String, Arc<dyn Driver>>>,
    /// Held while a browser starts, so two calls never start two for one conversation.
    starting: tokio::sync::Mutex<()>,
}

impl Browsers {
    pub(crate) fn get(&self, session_id: &str) -> Option<Arc<dyn Driver>> {
        self.all.lock().unwrap().get(session_id).cloned()
    }

    pub fn record(&self, session_id: &str) -> Option<BrowserRecord> {
        self.get(session_id).map(|driver| driver.record())
    }

    /// Ends a conversation's browser, as when it is archived or its subagent run ends.
    pub fn stop_session(&self, session_id: &str) {
        if let Some(driver) = self.get(session_id) {
            driver.stop();
        }
    }

    /// Ends every browser now, as Drift quits; the job objects would end them anyway on Windows.
    pub fn stop_all(&self) {
        for driver in self.all.lock().unwrap().values() {
            driver.kill();
        }
    }

    fn running_sessions(&self) -> HashSet<String> {
        self.all.lock().unwrap().keys().cloned().collect()
    }

    fn any(&self, check: impl Fn(&dyn Driver) -> bool) -> bool {
        self.all.lock().unwrap().values().any(|driver| check(driver.as_ref()))
    }

    /// Forgets a browser that ended, unless a newer one already took its place.
    fn remove(&self, driver: &Arc<dyn Driver>) {
        let mut all = self.all.lock().unwrap();
        let session = &driver.common().session_id;
        if all.get(session).is_some_and(|current| Arc::ptr_eq(current, driver)) {
            all.remove(session);
        }
    }
}

/// What the agent asked of a new browser.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Wanted<'a> {
    pub(crate) kind: Option<Kind>,
    pub(crate) url: Option<&'a str>,
}

/// The profile a new browser asks for.
#[derive(Clone, Copy)]
struct Profiled<'a> {
    session_id: &'a str,
    workspace: Option<&'a Workspace>,
    kind: Kind,
    storage: Storage,
}

/// A browser to start and what was said about choosing it.
struct Plan {
    spec: Spec,
    notes: Vec<String>,
}

impl Engine {
    pub fn browser_settings(&self) -> BrowserSettings {
        self.store.setting(SETTINGS_KEY).ok().flatten().unwrap_or_default()
    }

    pub fn set_browser_settings(&self, settings: &BrowserSettings) -> rusqlite::Result<()> {
        self.store.set_setting(SETTINGS_KEY, settings)
    }

    /// The conversation's browser, started now if it has none or another engine was asked for. The notes say
    /// how it differs from what was asked (another engine, or its own profile while the shared one is in use).
    pub(crate) async fn open_browser(
        &self,
        session_id: &str,
        wanted: Wanted<'_>,
    ) -> Result<(Arc<dyn Driver>, Vec<String>), String> {
        let _starting = self.browsers.starting.lock().await;
        let mut notes = Vec::new();

        if let Some(running) = self.running_browser(session_id).await {
            if wanted.kind.is_none_or(|kind| kind == running.common().kind) {
                return Ok((running, notes));
            }
            notes.push(format!(
                "The {} browser was closed to open {}.",
                running.common().kind.label(),
                wanted.kind.map_or("", Kind::label)
            ));
            running.stop();
            running.common().stopped().await;
            self.running_browser(session_id).await;
        }

        let plan = self.plan_browser(session_id, wanted).await?;
        notes.extend(plan.notes);
        let driver: Arc<dyn Driver> = match plan.spec.kind.family() {
            Family::Chromium => chromium::Chromium::start(self.me.clone(), plan.spec).await?,
            Family::Gecko => firefox::Firefox::start(self.me.clone(), plan.spec).await?,
            Family::Ie => ie::Ie::start(self.me.clone(), plan.spec).await?,
        };

        self.browsers
            .all
            .lock()
            .unwrap()
            .insert(session_id.into(), driver.clone());
        driver.common().mark_listed();
        self.publish_browser(&driver.record());

        Ok((driver, notes))
    }

    /// Starts the conversation's browser for the user, at the project's base address when it has one; a running
    /// browser is left as it is.
    pub async fn start_browser(&self, session_id: &str) -> Result<BrowserRecord, String> {
        let (_, project) = self.browser_project(session_id);
        let base = project.base_url.as_deref().map(|base| project.resolve(base));
        let running = self
            .browsers
            .get(session_id)
            .is_some_and(|driver| !driver.common().is_ended());

        let wanted = Wanted {
            kind: None,
            url: base.as_deref(),
        };
        let (driver, _) = self.open_browser(session_id, wanted).await?;
        driver.common().touch();

        // The pane opens on the new browser at once and watches the page load.
        if let Some(base) = base.filter(|_| !running) {
            let tab = driver.tab(None)?;
            tokio::spawn(async move {
                let _ = tab.go(&base).await;
            });
        }

        Ok(driver.record())
    }

    /// The conversation's browser if it is running; one still ending is waited out, so its replacement starts clean.
    async fn running_browser(&self, session_id: &str) -> Option<Arc<dyn Driver>> {
        let deadline = tokio::time::Instant::now() + ENDING_WAIT;
        loop {
            let driver = self.browsers.get(session_id)?;
            if !driver.common().is_ended() || tokio::time::Instant::now() >= deadline {
                return Some(driver);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// The conversation's workspace and its project's `browser` config.
    pub(crate) fn browser_project(&self, session_id: &str) -> (Option<Workspace>, BrowserProject) {
        let workspace = self
            .store
            .session(session_id)
            .ok()
            .flatten()
            .and_then(|session| self.store.workspace(&session.workspace_id).ok().flatten());
        let project = workspace
            .as_ref()
            .map(|workspace| self.workspace_config(std::path::Path::new(&workspace.path)).browser)
            .unwrap_or_default();

        (workspace, project)
    }

    /// Engine, profile and window: what the agent asked, else the project's config, else Settings.
    async fn plan_browser(&self, session_id: &str, wanted: Wanted<'_>) -> Result<Plan, String> {
        let settings = self.browser_settings();
        let (workspace, project) = self.browser_project(session_id);
        let mut notes = Vec::new();

        let chosen = wanted
            .kind
            .or_else(|| wanted.url.and_then(|url| project.engine_for(url)))
            .or(project.engine)
            .unwrap_or(settings.engine);
        let kind = self.available_kind(chosen, &mut notes).await?;
        if kind == Kind::Ie && self.browsers.any(|driver| driver.common().kind == Kind::Ie) {
            return Err(
                "Another conversation has the IE mode browser open, and IEDriverServer can drive only one \
                        at a time. Stop that one first."
                    .into(),
            );
        }

        let wanted_profile = Profiled {
            session_id,
            workspace: workspace.as_ref(),
            kind,
            storage: project.storage.unwrap_or(settings.storage),
        };
        let (storage, profile) = self.profile_for(&wanted_profile, &mut notes);

        let program = self
            .browser_program(kind)
            .ok_or_else(|| format!("{} could not be found", kind.label()))?;
        let spec = Spec {
            session_id: session_id.into(),
            kind,
            program,
            driver: (kind == Kind::Ie).then(|| self.ie_driver()).flatten(),
            storage,
            window: project.window.unwrap_or(settings.window),
            profile,
            integrated_auth: project.integrated_auth.clone(),
        };

        Ok(Plan { spec, notes })
    }

    /// The engine asked for when it can run here; Chrome and Edge stand in for each other.
    async fn available_kind(&self, wanted: Kind, notes: &mut Vec<String>) -> Result<Kind, String> {
        let missing = match engines::ensure(self, wanted).await {
            Ok(()) => return Ok(wanted),
            Err(missing) => missing,
        };
        if wanted.family() != Family::Chromium {
            return Err(missing);
        }

        let other = if wanted == Kind::Chrome {
            Kind::Edge
        } else {
            Kind::Chrome
        };
        if self.browser_program(other).is_none() {
            return Err("Neither Chrome nor Edge is installed, so there is no browser to open.".into());
        }
        notes.push(format!(
            "{} is not installed, so this browser is {}.",
            wanted.label(),
            other.label()
        ));

        Ok(other)
    }

    /// IE mode's profile belongs to its driver. A shared profile in use by another conversation cannot open twice,
    /// so this one gets its own.
    fn profile_for(&self, wanted: &Profiled, notes: &mut Vec<String>) -> (Storage, Option<std::path::PathBuf>) {
        let Profiled {
            session_id,
            workspace,
            kind,
            storage,
        } = *wanted;
        if kind == Kind::Ie {
            return (Storage::Isolated, None);
        }
        let isolated = (
            Storage::Isolated,
            Some(profiles::isolated(&self.data_dir, session_id, kind)),
        );
        let Some(workspace) = workspace.filter(|_| storage == Storage::Shared) else {
            return isolated;
        };

        let shared = profiles::shared(&self.data_dir, workspace, kind);
        if self
            .browsers
            .any(|driver| driver.common().profile.as_ref() == Some(&shared))
        {
            notes.push(
                "Another conversation of this workspace has the shared profile open, so this browser has its own and \
                 starts signed out."
                    .into(),
            );
            return isolated;
        }

        (Storage::Shared, Some(shared))
    }

    pub(crate) fn publish_browser(&self, record: &BrowserRecord) {
        self.hub.publish(Event::BrowserUpdated {
            browser: record.clone(),
        });
    }

    /// A browser has gone: tidy its profile, forget it, tell clients. It stays listed until its profile is tidied,
    /// so a new browser never starts on a folder being deleted.
    pub(crate) async fn browser_ended(&self, driver: &Arc<dyn Driver>) {
        let common = driver.common();
        if let Some(profile) = &common.profile {
            match common.storage {
                Storage::Isolated => profiles::remove(profile).await,
                Storage::Shared => profiles::trim(profile).await,
            }
        }

        self.browsers.remove(driver);
        self.publish_browser(&driver.record());
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
                .filter(|driver| driver.common().idle_for() > IDLE_LIMIT && !driver.common().watched())
                .cloned()
                .collect();
            for driver in idle {
                driver.stop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_address_gets_a_scheme() {
        assert_eq!(with_scheme("example.com/a"), "https://example.com/a");
        assert_eq!(with_scheme("localhost:5173"), "http://localhost:5173");
        assert_eq!(with_scheme("http://x"), "http://x");
        assert_eq!(with_scheme("about:blank"), "about:blank");
    }

    #[test]
    fn engines_round_trip_through_their_ids() {
        for kind in Kind::ALL {
            assert_eq!(Kind::parse(kind.id()), Some(kind));
        }
        assert_eq!(Kind::Ie.family(), Family::Ie);
        assert_eq!(Kind::Camoufox.family(), Family::Gecko);
    }
}
