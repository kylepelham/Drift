//! A running browser: its process and job, the DevTools connection, its pages, and the stream a pane watches.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::{Notify, broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use super::agent::{self, Agents};
use super::cdp::{Connection, Event};
use super::launch::{self, Launched};
use super::page::Page;
use super::view::{Frame, ViewInput};
use super::{BrowserRecord, BrowserState, Kind, Storage, Window};
use crate::Engine;
use crate::platform::process::Tree;

/// How long a new tab may take to appear after it was asked for.
const PAGE_WAIT: Duration = Duration::from_secs(10);
/// How long a browser asked to close may take before it is ended.
const CLOSE_WAIT: Duration = Duration::from_secs(5);
/// How often tab titles are read again while a pane watches.
const TITLE_CHECK: Duration = Duration::from_secs(2);
/// How often a first picture for a new watcher is tried again.
const FIRST_FRAME_RETRIES: usize = 8;
/// Frames waiting for a slow pane; older ones are skipped, never queued.
const FRAMES_KEPT: usize = 2;

pub(crate) struct Spec {
    pub(crate) session_id: String,
    pub(crate) kind: Kind,
    pub(crate) storage: Storage,
    pub(crate) window: Window,
    pub(crate) profile: PathBuf,
}

pub(crate) struct Instance {
    pub(crate) session_id: String,
    pub(crate) kind: Kind,
    pub(crate) storage: Storage,
    pub(crate) window: Window,
    pub(crate) profile: PathBuf,
    pub(crate) started_at: i64,
    pub(crate) agents: Agents,
    cdp: Arc<Connection>,
    tree: Tree,
    engine: Weak<Engine>,
    pages: Mutex<Vec<Arc<Page>>>,
    active: Mutex<Option<String>>,
    /// The page the stream shows, while a pane watches.
    streaming: Mutex<Option<String>>,
    used: Mutex<Instant>,
    viewers: AtomicUsize,
    frames: broadcast::Sender<Arc<Frame>>,
    stopping: CancellationToken,
    ended: AtomicBool,
    /// Set once the engine lists it, so no client hears of a browser the API cannot yet return.
    listed: AtomicBool,
    page_added: Notify,
}

/// A pane watching the stream; dropping it lets the stream stop.
pub(crate) struct Viewer {
    instance: Arc<Instance>,
    pub(crate) frames: broadcast::Receiver<Arc<Frame>>,
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.instance.viewers.fetch_sub(1, Ordering::SeqCst);
        self.instance.restream();
    }
}

impl Instance {
    /// Launches the browser, connects, and waits for its first tab.
    pub(crate) async fn start(engine: Weak<Engine>, spec: Spec) -> Result<Arc<Self>, String> {
        let Launched { child, tree, port, url } = launch::launch(spec.kind, &spec.profile, spec.window)
            .await
            .map_err(|error| error.to_string())?;
        let (cdp, events) = Connection::connect(&url, port)
            .await
            .map_err(|error| format!("could not connect to {}: {error}", spec.kind.label()))?;
        let agents = agent::agents(&cdp, spec.kind, spec.window == Window::Hidden).await;

        let instance = Arc::new(Self {
            session_id: spec.session_id,
            kind: spec.kind,
            storage: spec.storage,
            window: spec.window,
            profile: spec.profile,
            started_at: crate::id::now_ms(),
            agents,
            cdp: cdp.clone(),
            tree,
            engine,
            pages: Mutex::default(),
            active: Mutex::default(),
            streaming: Mutex::default(),
            used: Mutex::new(Instant::now()),
            viewers: AtomicUsize::new(0),
            frames: broadcast::channel(FRAMES_KEPT).0,
            stopping: CancellationToken::new(),
            ended: AtomicBool::new(false),
            listed: AtomicBool::new(false),
            page_added: Notify::new(),
        });
        tokio::spawn(instance.clone().run(events, child));

        let waiting = instance.page_added.notified();
        let attach = json!({
            "autoAttach": true,
            "waitForDebuggerOnStart": true,
            "flatten": true,
            "filter": [{ "type": "page" }, { "exclude": true }],
        });
        let started = async {
            cdp.call("Target.setDiscoverTargets", json!({ "discover": true }), None)
                .await?;
            cdp.call("Target.setAutoAttach", attach, None).await
        };
        if let Err(error) = started.await {
            instance.kill();
            return Err(format!("{} did not accept the connection: {error}", spec.kind.label()));
        }
        let _ = tokio::time::timeout(PAGE_WAIT, waiting).await;

        Ok(instance)
    }

    pub(crate) fn record(&self) -> BrowserRecord {
        let active = self.active.lock().unwrap().clone();
        let pages = self
            .pages
            .lock()
            .unwrap()
            .iter()
            .map(|page| page.record(active.as_deref() == Some(page.target.as_str())))
            .collect();
        let state = if self.ended.load(Ordering::SeqCst) {
            BrowserState::Stopped
        } else {
            BrowserState::Running
        };

        BrowserRecord {
            session_id: self.session_id.clone(),
            engine: self.kind,
            storage: self.storage,
            window: self.window,
            state,
            started_at: self.started_at,
            pages,
        }
    }

    pub(crate) fn pages(&self) -> Vec<Arc<Page>> {
        self.pages.lock().unwrap().clone()
    }

    /// The page `id` names, or the active one.
    pub(crate) fn page(&self, id: Option<&str>) -> Result<Arc<Page>, String> {
        let wanted = id.map(str::to_string).or_else(|| self.active.lock().unwrap().clone());
        let pages = self.pages.lock().unwrap();

        match wanted {
            Some(id) => pages
                .iter()
                .find(|page| page.target == id)
                .cloned()
                .ok_or_else(|| format!("no page `{id}`; `pages` lists them")),
            None => Err("the browser has no page open; `open` one".into()),
        }
    }

    pub(crate) fn is_active(&self, page: &Page) -> bool {
        self.active.lock().unwrap().as_deref() == Some(page.target.as_str())
    }

    /// Opens a tab, makes it the active one, and loads `url` in it.
    pub(crate) async fn new_page(self: &Arc<Self>, url: &str) -> Result<Arc<Page>, String> {
        let created = self
            .cdp
            .call("Target.createTarget", json!({ "url": "about:blank" }), None)
            .await
            .map_err(|error| error.to_string())?;
        let target = created["targetId"].as_str().unwrap_or_default().to_string();

        let deadline = tokio::time::Instant::now() + PAGE_WAIT;
        let page = loop {
            let waiting = self.page_added.notified();
            if let Ok(page) = self.page(Some(&target)) {
                break page;
            }
            if tokio::time::timeout_at(deadline, waiting).await.is_err() {
                return Err("the new tab did not open in time".into());
            }
        };
        self.select(&page.target);

        if url != "about:blank" {
            page.navigate(url).await?;
        }

        Ok(page)
    }

    /// Makes a page the active one: the browser fronts it, and the pane's stream follows it.
    pub(crate) fn select(self: &Arc<Self>, target: &str) {
        *self.active.lock().unwrap() = Some(target.to_string());
        self.cdp
            .send("Target.activateTarget", json!({ "targetId": target }), None);
        self.restream();
        self.changed();
    }

    pub(crate) async fn close_page(&self, target: &str) -> Result<(), String> {
        self.cdp
            .call("Target.closeTarget", json!({ "targetId": target }), None)
            .await
            .map_err(|error| error.to_string())?;

        Ok(())
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

    /// Asks the browser to close, so a shared profile writes its cookies out, and ends it if it does not.
    pub(crate) fn stop(self: &Arc<Self>) {
        let instance = self.clone();
        tokio::spawn(async move {
            let _ = tokio::time::timeout(CLOSE_WAIT, instance.cdp.call("Browser.close", json!({}), None)).await;
            tokio::time::sleep(CLOSE_WAIT).await;
            instance.stopping.cancel();
        });
    }

    /// Whether the browser has ended or is ending.
    pub(crate) fn is_ended(&self) -> bool {
        self.stopping.is_cancelled()
    }

    /// Returns once the browser has ended or is ending.
    pub(crate) async fn stopped(&self) {
        self.stopping.cancelled().await;
    }

    /// Ends the browser and everything it started now.
    pub(crate) fn kill(&self) {
        self.tree.kill();
        self.stopping.cancel();
    }

    /// Starts watching the stream; the first viewer starts it.
    pub(crate) fn watch(self: &Arc<Self>) -> Viewer {
        let frames = self.frames.subscribe();
        let first = self.viewers.fetch_add(1, Ordering::SeqCst) == 0;
        self.restream();
        // A second watcher joins a stream already running; it gets its own first picture.
        let streamed = self.streaming.lock().unwrap().clone();
        if let (false, Ok(page)) = (first, self.page(streamed.as_deref())) {
            self.first_frame(page);
        }

        Viewer {
            instance: self.clone(),
            frames,
        }
    }

    /// Points the stream at the active page while anyone watches, and stops it when no one does.
    fn restream(self: &Arc<Self>) {
        let wanted = if self.watched() {
            self.active.lock().unwrap().clone()
        } else {
            None
        };
        let mut streaming = self.streaming.lock().unwrap();
        if *streaming == wanted {
            return;
        }

        let pages = self.pages.lock().unwrap();
        let find = |id: &Option<String>| pages.iter().find(|page| Some(&page.target) == id.as_ref()).cloned();
        if let Some(page) = find(&streaming) {
            page.send("Page.stopScreencast", json!({}));
        }
        let started = find(&wanted);
        if let Some(page) = &started {
            let params = json!({ "format": "jpeg", "quality": 70, "maxWidth": 1920, "maxHeight": 1920 });
            page.send("Page.startScreencast", params);
        }
        *streaming = wanted;
        drop(pages);
        drop(streaming);

        if let Some(page) = started {
            self.first_frame(page);
        }
    }

    /// A screencast sends nothing until the page paints again, so a new watcher or tab gets a picture at once.
    fn first_frame(self: &Arc<Self>, page: Arc<Page>) {
        let instance = self.clone();
        tokio::spawn(async move {
            // A page between documents (a navigation swapping its process) cannot be pictured for a moment.
            let mut picture = page.picture().await;
            for _ in 0..FIRST_FRAME_RETRIES {
                if picture.is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
                picture = page.picture().await;
            }
            let Ok((jpeg, size)) = picture else {
                return;
            };
            if instance.streaming.lock().unwrap().as_deref() != Some(page.target.as_str()) {
                return;
            }

            let frame = Frame {
                page: page.target.clone(),
                width: size.0,
                height: size.1,
                jpeg,
            };
            let _ = instance.frames.send(Arc::new(frame));
        });
    }

    /// Reads every tab's title and address again: the browser reports a title set by the page's own script
    /// only when asked.
    pub(crate) async fn refresh(&self) {
        let Ok(targets) = self.cdp.call("Target.getTargets", json!({}), None).await else {
            return;
        };

        let pages = self.pages();
        let mut changed = false;
        for info in targets["targetInfos"].as_array().into_iter().flatten() {
            let Some(page) = pages.iter().find(|page| info["targetId"] == page.target.as_str()) else {
                continue;
            };
            let url = info["url"].as_str().unwrap_or_default();
            let title = info["title"].as_str().unwrap_or_default();
            changed |= page.set_info(url, title);
        }

        if changed {
            self.changed();
        }
    }

    fn refresh_soon(self: &Arc<Self>) {
        let instance = self.clone();
        tokio::spawn(async move { instance.refresh().await });
    }

    /// The engine lists it from now on, so its changes may be told.
    pub(crate) fn mark_listed(&self) {
        self.listed.store(true, Ordering::SeqCst);
    }

    /// Something the pane shows changed.
    fn changed(&self) {
        if !self.listed.load(Ordering::SeqCst) {
            return;
        }
        if let Some(engine) = self.engine.upgrade() {
            engine.publish_browser(self);
        }
    }

    async fn run(self: Arc<Self>, mut events: mpsc::UnboundedReceiver<Event>, mut child: tokio::process::Child) {
        let mut titles = tokio::time::interval(TITLE_CHECK);
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    Some(event) => self.dispatch(event),
                    None => break,
                },
                _ = titles.tick() => {
                    if self.watched() {
                        self.refresh_soon();
                    }
                }
                _ = child.wait() => break,
                () = self.stopping.cancelled() => break,
            }
        }

        self.ended.store(true, Ordering::SeqCst);
        self.stopping.cancel();
        self.pages.lock().unwrap().clear();
        self.tree.kill();
        let _ = child.wait().await;
        self.tree.stop().await;

        if let Some(engine) = self.engine.upgrade() {
            engine.browser_ended(&self).await;
        }
    }

    fn dispatch(self: &Arc<Self>, event: Event) {
        let Some(session) = event.session else {
            self.browser_event(&event.method, event.params);
            return;
        };
        let page = self
            .pages
            .lock()
            .unwrap()
            .iter()
            .find(|page| page.session == session)
            .cloned();
        let Some(page) = page else { return };

        if event.method == "Page.screencastFrame" {
            self.frame(&page, &event.params);
            return;
        }
        if page.handle(&event.method, &event.params) {
            self.changed();
        }
        if matches!(
            event.method.as_str(),
            "Page.loadEventFired" | "Page.navigatedWithinDocument" | "Page.domContentEventFired"
        ) {
            self.refresh_soon();
        }
    }

    fn browser_event(self: &Arc<Self>, method: &str, params: Value) {
        match method {
            "Target.attachedToTarget" => self.attach(&params),
            "Target.targetInfoChanged" => {
                let info = &params["targetInfo"];
                let page = self.page(info["targetId"].as_str()).ok();
                let url = info["url"].as_str().unwrap_or_default();
                let title = info["title"].as_str().unwrap_or_default();
                if page.is_some_and(|page| page.set_info(url, title)) {
                    self.changed();
                }
            }
            // A session can end while its tab lives on (another client detached), so detaching goes by session.
            "Target.detachedFromTarget" => {
                let session = params["sessionId"].as_str().unwrap_or_default();
                self.forget(|page| page.session == session);
            }
            "Target.targetDestroyed" => {
                let target = params["targetId"].as_str().unwrap_or_default();
                self.forget(|page| page.target == target);
            }
            _ => {}
        }
    }

    /// A new tab: from the agent, the pane, or a page opening a popup. It is listed at once, so none of its
    /// events are missed, and set up before anything uses it. A popup becomes the active page, as a browser
    /// brings a new tab forward.
    fn attach(self: &Arc<Self>, params: &Value) {
        let info = &params["targetInfo"];
        if info["type"] != "page" {
            return;
        }
        let target = info["targetId"].as_str().unwrap_or_default().to_string();
        let session = params["sessionId"].as_str().unwrap_or_default().to_string();
        let popup = info["openerId"].as_str().is_some_and(|opener| !opener.is_empty());
        let url = info["url"].as_str().unwrap_or_default().to_string();

        let page = Page::new(target.clone(), session, popup, self.cdp.clone(), url);
        self.pages.lock().unwrap().push(page.clone());

        let instance = self.clone();
        tokio::spawn(async move {
            let ready = page.setup(instance.agents.desktop.as_ref()).await.is_ok();
            let listed = instance
                .pages
                .lock()
                .unwrap()
                .iter()
                .any(|listed| Arc::ptr_eq(listed, &page));
            if !ready || !listed {
                return;
            }

            let first = instance.active.lock().unwrap().is_none();
            if popup || first {
                instance.select(&target);
            } else {
                instance.changed();
            }
            instance.page_added.notify_waiters();
        });
    }

    fn forget(self: &Arc<Self>, gone: impl Fn(&Page) -> bool) {
        let mut pages = self.pages.lock().unwrap();
        let Some(index) = pages.iter().position(|page| gone(page)) else {
            return;
        };
        let target = pages.remove(index).target.clone();
        let still_open = pages.iter().any(|page| page.target == target);
        let fallback = pages.last().map(|page| page.target.clone());
        drop(pages);

        if !still_open {
            let mut active = self.active.lock().unwrap();
            if active.as_deref() == Some(target.as_str()) {
                *active = fallback;
            }
        }
        {
            let mut streaming = self.streaming.lock().unwrap();
            if streaming.as_deref() == Some(target.as_str()) {
                *streaming = None;
            }
        }
        self.restream();
        self.changed();
    }

    fn frame(&self, page: &Page, params: &Value) {
        use base64::Engine as _;

        page.send("Page.screencastFrameAck", json!({ "sessionId": params["sessionId"] }));
        if self.streaming.lock().unwrap().as_deref() != Some(page.target.as_str()) {
            return;
        }

        let Ok(jpeg) = base64::engine::general_purpose::STANDARD.decode(params["data"].as_str().unwrap_or_default())
        else {
            return;
        };
        let metadata = &params["metadata"];
        let frame = Frame {
            page: page.target.clone(),
            width: metadata["deviceWidth"].as_f64().unwrap_or_default(),
            height: metadata["deviceHeight"].as_f64().unwrap_or_default(),
            jpeg,
        };
        let _ = self.frames.send(Arc::new(frame));
    }

    /// The user's input from the pane, to the active page.
    pub(crate) async fn input(self: &Arc<Self>, input: ViewInput) -> Result<(), String> {
        self.touch();
        match input {
            ViewInput::Select { page } => self.select(&page),
            ViewInput::Close { page } => self.close_page(&page).await?,
            ViewInput::NewPage {} => {
                self.new_page("about:blank").await?;
            }
            other => self.page_input(other).await?,
        }

        Ok(())
    }

    async fn page_input(self: &Arc<Self>, input: ViewInput) -> Result<(), String> {
        let page = self.page(None)?;
        match input {
            ViewInput::Mouse(mouse) => {
                if let Some(event) = mouse.event() {
                    page.send("Input.dispatchMouseEvent", event);
                }
            }
            ViewInput::Key(key) => {
                let direction = if key.event == "up" { "up" } else { "down" };
                let event = super::input::Key::from_input(&key).event(direction, key.modifiers);
                page.send("Input.dispatchKeyEvent", event);
            }
            ViewInput::Text { text } => page.send("Input.insertText", json!({ "text": text })),
            ViewInput::Navigate { url } => go(&page, &url).await?,
            ViewInput::Dialog { accept, text } => page.answer_dialog(accept, text.as_deref()).await?,
            ViewInput::Select { .. } | ViewInput::Close { .. } | ViewInput::NewPage {} => {}
        }

        Ok(())
    }
}

/// An address, or `back`, `forward` or `reload`; a bare host gets `https://`.
pub(crate) async fn go(page: &Page, url: &str) -> Result<(), String> {
    match url {
        "back" => page.history(-1).await,
        "forward" => page.history(1).await,
        "reload" => page.reload().await,
        _ => page.navigate(&with_scheme(url)).await,
    }
}

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
}
