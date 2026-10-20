//! What every browser engine provides, so the tool, the pane and the API never care which one runs: a `Driver`
//! for the browser and a `Tab` for each page, with the bookkeeping they all share in `Common`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use super::input::Key;
use super::view::{Frame, ViewInput};
use super::{BrowserRecord, BrowserState, DialogRecord, Kind, PageRecord, Storage, Window};
use crate::Engine;

/// Frames waiting for a slow pane; older ones are skipped, never queued.
const FRAMES_KEPT: usize = 2;

/// What an action aims at: an element from the last snapshot, or a point in the last screenshot's pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Target {
    Ref(u64),
    Point(f64, f64),
}

/// What an action changed, for its result to say.
pub(crate) struct Settled {
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) navigated: bool,
    pub(crate) dialog: Option<DialogRecord>,
}

/// A JPEG of what a page shows, with its size in pixels.
pub(crate) struct Shot {
    pub(crate) jpeg: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// A layout size the agent asked for; `None` in its place means the browser's own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Size {
    pub(crate) width: u64,
    pub(crate) height: u64,
    pub(crate) mobile: bool,
}

/// The language a script is written in; only IE mode runs VBScript.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Language {
    JavaScript,
    VbScript,
}

/// One page of a browser.
#[async_trait]
pub(crate) trait Tab: Send + Sync {
    fn id(&self) -> String;

    fn record(&self, active: bool) -> PageRecord;

    fn url(&self) -> String {
        self.record(false).url
    }

    fn dialog(&self) -> Option<DialogRecord> {
        self.record(false).dialog
    }

    /// The page's elements, those the agent can act on tagged `[ref=N]`.
    async fn snapshot(&self, interactive: bool) -> Result<String, String>;

    /// The text the page shows, for waiting on it.
    async fn text(&self) -> Result<String, String>;

    async fn click(&self, target: Target, double: bool) -> Result<String, String>;

    async fn hover(&self, target: Target) -> Result<String, String>;

    /// Sets a field: text into an input, an option of a select, or a checkbox to `checked`.
    async fn fill(&self, reference: u64, text: &str, checked: Option<bool>) -> Result<String, String>;

    /// Scrolls an element into view, or the page by `dy` pixels.
    async fn scroll(&self, reference: Option<u64>, dy: f64) -> Result<String, String>;

    async fn press(&self, key: &Key, modifiers: u8) -> Result<(), String>;

    async fn type_text(&self, text: &str) -> Result<(), String>;

    /// Loads an address, or goes `back`, `forward`, or `reload`s.
    async fn go(&self, url: &str) -> Result<(), String>;

    /// Lets a navigation an action started finish, then says where the page is.
    async fn settle(&self, url_before: &str) -> Settled;

    async fn screenshot(&self) -> Result<Shot, String>;

    async fn evaluate(&self, script: &str, language: Language) -> Result<Value, String>;

    /// Console lines or requests, newest last; an error when the engine cannot see them.
    fn logs(&self, network: bool, matches: &dyn Fn(&str) -> bool, limit: usize) -> Result<Vec<String>, String>;

    async fn response_body(&self, request: &str) -> Result<String, String>;

    async fn answer_dialog(&self, accept: bool, text: Option<&str>) -> Result<(), String>;

    async fn viewport(&self, size: Option<Size>) -> Result<(), String>;
}

/// A running browser.
#[async_trait]
pub(crate) trait Driver: Send + Sync {
    fn common(&self) -> &Common;

    fn tabs(&self) -> Vec<Arc<dyn Tab>>;

    /// The id of the page actions use when they name none.
    fn active(&self) -> Option<String>;

    async fn open_tab(&self, url: &str) -> Result<Arc<dyn Tab>, String>;

    async fn select(&self, id: &str) -> Result<(), String>;

    async fn close_tab(&self, id: &str) -> Result<(), String>;

    /// Reads tab titles and addresses again.
    async fn refresh(&self);

    /// The number of watchers changed: start or stop sending frames.
    fn viewers_changed(&self);

    /// Asks the browser to close, and ends it if it does not.
    fn stop(&self);

    /// Ends the browser and everything it started now.
    fn kill(&self);

    /// The user's input from the pane, to the active page.
    async fn input(&self, input: ViewInput) -> Result<(), String>;

    /// A note about how this browser differs from what was asked, said once when it opens.
    fn opening_note(&self) -> Option<String> {
        None
    }
}

impl dyn Driver {
    pub(crate) fn tab(&self, id: Option<&str>) -> Result<Arc<dyn Tab>, String> {
        let wanted = id.map(str::to_string).or_else(|| self.active());
        let tabs = self.tabs();

        match wanted {
            Some(id) => tabs
                .into_iter()
                .find(|tab| tab.id() == id)
                .ok_or_else(|| format!("no page `{id}`; `pages` lists them")),
            None => Err("the browser has no page open; `open` one".into()),
        }
    }

    pub(crate) fn is_active(&self, id: &str) -> bool {
        self.active().as_deref() == Some(id)
    }

    pub(crate) fn record(&self) -> BrowserRecord {
        let common = self.common();
        let active = self.active();
        let pages = self
            .tabs()
            .iter()
            .map(|tab| tab.record(active.as_deref() == Some(tab.id().as_str())))
            .collect();
        let state = if common.ended.load(Ordering::SeqCst) {
            BrowserState::Stopped
        } else {
            BrowserState::Running
        };

        BrowserRecord {
            session_id: common.session_id.clone(),
            engine: common.kind,
            storage: common.storage,
            window: common.window,
            state,
            started_at: common.started_at,
            pages,
        }
    }

    /// Something the pane shows changed.
    pub(crate) fn changed(&self) {
        let common = self.common();
        if !common.listed.load(Ordering::SeqCst) {
            return;
        }
        if let Some(engine) = common.engine.upgrade() {
            engine.publish_browser(&self.record());
        }
    }
}

/// Starts watching a browser's frames; the first watcher starts them.
pub(crate) fn watch(driver: Arc<dyn Driver>) -> Viewer {
    let frames = driver.common().frames.subscribe();
    driver.common().viewers.fetch_add(1, Ordering::SeqCst);
    driver.viewers_changed();

    Viewer { driver, frames }
}

/// A pane watching a browser; dropping it lets the frames stop.
pub(crate) struct Viewer {
    driver: Arc<dyn Driver>,
    pub(crate) frames: broadcast::Receiver<Arc<Frame>>,
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.driver.common().viewers.fetch_sub(1, Ordering::SeqCst);
        self.driver.viewers_changed();
    }
}

/// What a browser is and the state every engine keeps the same way.
pub(crate) struct Spec {
    pub(crate) session_id: String,
    pub(crate) kind: Kind,
    pub(crate) storage: Storage,
    pub(crate) window: Window,
    /// The browser program: Chrome, Edge, Firefox or Camoufox, and Edge for IE mode.
    pub(crate) program: PathBuf,
    /// IEDriverServer, for IE mode.
    pub(crate) driver: Option<PathBuf>,
    /// The profile Drift owns, if the engine uses one (IE mode's belongs to its driver).
    pub(crate) profile: Option<PathBuf>,
    /// Hosts Windows may sign in to with the user's own account.
    pub(crate) integrated_auth: Vec<String>,
}

pub(crate) struct Common {
    pub(crate) session_id: String,
    pub(crate) kind: Kind,
    pub(crate) storage: Storage,
    pub(crate) window: Window,
    pub(crate) profile: Option<PathBuf>,
    pub(crate) started_at: i64,
    pub(crate) engine: Weak<Engine>,
    pub(crate) frames: broadcast::Sender<Arc<Frame>>,
    pub(crate) stopping: CancellationToken,
    pub(crate) ended: AtomicBool,
    used: Mutex<Instant>,
    viewers: AtomicUsize,
    /// Set once the engine lists it, so no client hears of a browser the API cannot yet return.
    listed: AtomicBool,
}

impl Common {
    pub(crate) fn new(engine: Weak<Engine>, spec: Spec) -> Self {
        Self {
            session_id: spec.session_id,
            kind: spec.kind,
            storage: spec.storage,
            window: spec.window,
            profile: spec.profile,
            started_at: crate::id::now_ms(),
            engine,
            frames: broadcast::channel(FRAMES_KEPT).0,
            stopping: CancellationToken::new(),
            ended: AtomicBool::new(false),
            used: Mutex::new(Instant::now()),
            viewers: AtomicUsize::new(0),
            listed: AtomicBool::new(false),
        }
    }

    /// Marks the browser as in use now, which keeps it from ending idle.
    pub(crate) fn touch(&self) {
        *self.used.lock().unwrap() = Instant::now();
    }

    pub(crate) fn idle_for(&self) -> Duration {
        self.used.lock().unwrap().elapsed()
    }

    pub(crate) fn watched(&self) -> bool {
        self.viewers.load(Ordering::SeqCst) > 0
    }

    pub(crate) fn mark_listed(&self) {
        self.listed.store(true, Ordering::SeqCst);
    }

    /// The browser has gone: nothing runs on it any more.
    pub(crate) fn mark_ended(&self) {
        self.ended.store(true, Ordering::SeqCst);
        self.stopping.cancel();
    }

    /// Whether the browser has ended or is ending.
    pub(crate) fn is_ended(&self) -> bool {
        self.stopping.is_cancelled()
    }

    /// Returns once the browser has ended or is ending.
    pub(crate) async fn stopped(&self) {
        self.stopping.cancelled().await;
    }

    pub(crate) fn send_frame(&self, frame: Frame) {
        let _ = self.frames.send(Arc::new(frame));
    }
}
