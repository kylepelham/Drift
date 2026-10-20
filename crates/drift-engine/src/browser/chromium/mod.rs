//! Chrome and Edge over the DevTools Protocol: the process and its job, the connection, its pages, and the
//! screencast a pane watches.

mod agent;
mod cdp;
pub(crate) mod launch;
mod page;
mod tab;
mod trace;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};

use super::driver::{Common, Driver, Spec, Tab};
use super::view::{Frame, ViewInput, pane_viewport};
use super::{Window, input};
use crate::Engine;
use crate::platform::process::Tree;
use agent::Agents;
use cdp::{Connection, Event};
use launch::Launched;
use page::{Page, PaneFit, Shared};

/// How long a new tab may take to appear after it was asked for.
const PAGE_WAIT: Duration = Duration::from_secs(10);
/// How long a browser asked to close may take before it is ended.
const CLOSE_WAIT: Duration = Duration::from_secs(5);
/// How often tab titles are read again while a pane watches.
const TITLE_CHECK: Duration = Duration::from_secs(2);
/// How often a first picture for a new watcher is tried again.
const FIRST_FRAME_RETRIES: usize = 8;

pub(crate) struct Chromium {
    common: Common,
    me: Weak<Chromium>,
    agents: Arc<Agents>,
    cdp: Arc<Connection>,
    port: u16,
    tree: Tree,
    pages: Mutex<Vec<Arc<Page>>>,
    active: Mutex<Option<String>>,
    /// The page the stream shows, while a pane watches.
    streaming: Mutex<Option<String>>,
    page_added: Notify,
    pane_fit: PaneFit,
    /// The newest pane pointer position whose cursor has not been looked up yet.
    pointer: Mutex<Option<(f64, f64)>>,
    /// A cursor lookup is running; it takes each newest position until none is left.
    pointing: AtomicBool,
}

impl Chromium {
    /// Launches the browser, connects, and waits for its first tab.
    pub(crate) async fn start(engine: Weak<Engine>, spec: Spec) -> Result<Arc<Self>, String> {
        let kind = spec.kind;
        let profile = spec.profile.clone().ok_or("Chrome and Edge need a profile folder")?;
        let options = launch::Options {
            window: spec.window,
            integrated_auth: &spec.integrated_auth,
        };
        let Launched { child, tree, port, url } = launch::launch(kind, &spec.program, &profile, &options)
            .await
            .map_err(|error| error.to_string())?;
        let (cdp, events) = Connection::connect(&url, port)
            .await
            .map_err(|error| format!("could not connect to {}: {error}", kind.label()))?;
        let agents = Arc::new(agent::agents(&cdp, kind, spec.window == Window::Hidden).await);

        let chromium = Arc::new_cyclic(|me| Self {
            common: Common::new(engine, spec),
            me: me.clone(),
            agents,
            cdp: cdp.clone(),
            port,
            tree,
            pages: Mutex::default(),
            active: Mutex::default(),
            streaming: Mutex::default(),
            page_added: Notify::new(),
            pane_fit: PaneFit::default(),
            pointer: Mutex::default(),
            pointing: AtomicBool::new(false),
        });
        tokio::spawn(chromium.clone().run(events, child));

        let waiting = chromium.page_added.notified();
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
            chromium.kill();
            return Err(format!("{} did not accept the connection: {error}", kind.label()));
        }
        let _ = tokio::time::timeout(PAGE_WAIT, waiting).await;

        Ok(chromium)
    }

    fn page(&self, id: &str) -> Option<Arc<Page>> {
        self.pages
            .lock()
            .unwrap()
            .iter()
            .find(|page| page.target == id)
            .cloned()
    }

    fn as_driver(&self) -> Option<Arc<dyn Driver>> {
        self.me.upgrade().map(|me| me as Arc<dyn Driver>)
    }

    fn changed(&self) {
        if let Some(driver) = self.as_driver() {
            driver.changed();
        }
    }

    /// Makes a page the active one: the browser fronts it, and the pane's stream follows it.
    fn make_active(&self, target: &str) {
        *self.active.lock().unwrap() = Some(target.to_string());
        self.cdp
            .send("Target.activateTarget", json!({ "targetId": target }), None);
        self.restream();
        self.changed();
    }

    /// Points the stream at the active page while anyone watches, and stops it when no one does.
    fn restream(&self) {
        let wanted = if self.common.watched() {
            self.active.lock().unwrap().clone()
        } else {
            None
        };
        let mut streaming = self.streaming.lock().unwrap();
        if *streaming == wanted {
            return;
        }

        if let Some(page) = streaming.as_deref().and_then(|id| self.page(id)) {
            page.send("Page.stopScreencast", json!({}));
        }
        let started = wanted.as_deref().and_then(|id| self.page(id));
        if let Some(page) = &started {
            let params = json!({ "format": "jpeg", "quality": 70, "maxWidth": 1920, "maxHeight": 1920 });
            page.send("Page.startScreencast", params);
        }
        *streaming = wanted;
        drop(streaming);

        if let Some(page) = started {
            self.first_frame(page);
        }
    }

    /// A screencast sends nothing until the page paints again, so a new watcher or tab, or a page laid out to a new
    /// size, gets a picture at once.
    fn first_frame(&self, page: Arc<Page>) {
        let Some(chromium) = self.me.upgrade() else {
            return;
        };

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
            if chromium.streaming.lock().unwrap().as_deref() != Some(page.target.as_str()) {
                return;
            }

            chromium.common.send_frame(Frame {
                page: page.target.clone(),
                width: size.0,
                height: size.1,
                jpeg,
            });
        });
    }

    fn refresh_soon(&self) {
        let Some(chromium) = self.me.upgrade() else {
            return;
        };

        tokio::spawn(async move { chromium.refresh().await });
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
                    if self.common.watched() {
                        self.refresh_soon();
                    }
                }
                _ = child.wait() => break,
                () = self.common.stopping.cancelled() => break,
            }
        }

        self.common.mark_ended();
        self.pages.lock().unwrap().clear();
        self.tree.kill();
        let _ = child.wait().await;
        self.tree.stop().await;

        if let (Some(engine), Some(driver)) = (self.common.engine.upgrade(), self.as_driver()) {
            engine.browser_ended(&driver).await;
        }
    }

    fn dispatch(&self, event: Event) {
        let Some(session) = event.session else {
            self.browser_event(&event.method, &event.params);
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

    fn browser_event(&self, method: &str, params: &Value) {
        match method {
            "Target.attachedToTarget" => self.attach(params),
            "Target.targetInfoChanged" => {
                let info = &params["targetInfo"];
                let page = info["targetId"].as_str().and_then(|id| self.page(id));
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
    fn attach(&self, params: &Value) {
        let info = &params["targetInfo"];
        if info["type"] != "page" {
            return;
        }
        let Some(chromium) = self.me.upgrade() else {
            return;
        };
        let target = info["targetId"].as_str().unwrap_or_default().to_string();
        let session = params["sessionId"].as_str().unwrap_or_default().to_string();
        let popup = info["openerId"].as_str().is_some_and(|opener| !opener.is_empty());
        let url = info["url"].as_str().unwrap_or_default();

        let page = Arc::new(Page::new(
            target.clone(),
            session,
            popup,
            &Shared {
                cdp: self.cdp.clone(),
                agents: self.agents.clone(),
                pane_fit: self.pane_fit.clone(),
            },
        ));
        page.set_info(url, "");
        self.pages.lock().unwrap().push(page.clone());

        tokio::spawn(async move {
            let ready = page.setup(chromium.agents.desktop.as_ref()).await.is_ok();
            let listed = chromium
                .pages
                .lock()
                .unwrap()
                .iter()
                .any(|listed| Arc::ptr_eq(listed, &page));
            if !ready || !listed {
                return;
            }

            let first = chromium.active.lock().unwrap().is_none();
            if popup || first {
                chromium.make_active(&target);
            } else {
                chromium.changed();
            }
            chromium.page_added.notify_waiters();
        });
    }

    fn forget(&self, gone: impl Fn(&Page) -> bool) {
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

        let data = params["data"].as_str().unwrap_or_default();
        let Ok(jpeg) = base64::engine::general_purpose::STANDARD.decode(data) else {
            return;
        };
        let metadata = &params["metadata"];
        self.common.send_frame(Frame {
            page: page.target.clone(),
            width: metadata["deviceWidth"].as_f64().unwrap_or_default(),
            height: metadata["deviceHeight"].as_f64().unwrap_or_default(),
            jpeg,
        });
    }

    async fn page_input(&self, input: ViewInput) -> Result<(), String> {
        if let ViewInput::Fit { width, height } = input {
            self.fit(width, height);
            return Ok(());
        }

        let id = self.active().ok_or("the browser has no page open")?;
        let page = self.page(&id).ok_or("the active page has closed")?;
        match input {
            ViewInput::Mouse(mouse) => {
                if let Some(event) = mouse.event() {
                    page.send("Input.dispatchMouseEvent", event);
                }
                self.follow_pointer(mouse.x, mouse.y);
            }
            ViewInput::Key(key) => {
                let direction = if key.event == "up" { "up" } else { "down" };
                let event = input::Key::from_input(&key).event(direction, key.modifiers);
                page.send("Input.dispatchKeyEvent", event);
            }
            ViewInput::Text { text } => page.send("Input.insertText", json!({ "text": text })),
            ViewInput::Navigate { url } => page.go_to(&url).await?,
            ViewInput::Dialog { accept, text } => page.answer(accept, text.as_deref()).await?,
            ViewInput::Select { .. } | ViewInput::Close { .. } | ViewInput::NewPage {} | ViewInput::Fit { .. } => {}
        }

        Ok(())
    }

    /// Lays every page out to fill the pane; pages opened later are laid out the same as they start.
    fn fit(&self, width: f64, height: f64) {
        let Some(viewport) = pane_viewport(width, height) else {
            return;
        };
        {
            let mut fit = self.pane_fit.lock().unwrap();
            if *fit == Some(viewport) {
                return;
            }
            *fit = Some(viewport);
        }

        for page in self.pages.lock().unwrap().iter() {
            page.fit_pane();
        }

        // A still page may not paint again at its new size, so the pane gets a picture of it.
        let streamed = self.streaming.lock().unwrap().clone();
        if let Some(page) = streamed.and_then(|id| self.page(&id)) {
            self.first_frame(page);
        }
    }

    /// Looks up the cursor under the pane's pointer for the pane to show. Moves come faster than lookups, so
    /// one lookup at a time runs and takes the newest position each time.
    fn follow_pointer(&self, x: f64, y: f64) {
        *self.pointer.lock().unwrap() = Some((x, y));
        if self.pointing.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(chromium) = self.me.upgrade() else {
            return;
        };

        tokio::spawn(async move {
            loop {
                let next = chromium.pointer.lock().unwrap().take();
                if let Some((x, y)) = next {
                    // A dialog holds the page, so a lookup would wait until it closes.
                    let page = chromium.active().and_then(|id| chromium.page(&id));
                    let page = page.filter(|page| page.current_dialog().is_none());
                    if let Some(page) = page
                        && let Ok(cursor) = page.cursor_at(x, y).await
                    {
                        chromium.common.set_cursor(&cursor);
                    }
                    continue;
                }

                // A position that came in as this lookup finished is taken by it, not dropped.
                chromium.pointing.store(false, Ordering::SeqCst);
                let waiting = chromium.pointer.lock().unwrap().is_some();
                if !waiting || chromium.pointing.swap(true, Ordering::SeqCst) {
                    return;
                }
            }
        });
    }
}

#[async_trait]
impl Driver for Chromium {
    fn common(&self) -> &Common {
        &self.common
    }

    fn tabs(&self) -> Vec<Arc<dyn Tab>> {
        self.pages
            .lock()
            .unwrap()
            .iter()
            .map(|page| page.clone() as Arc<dyn Tab>)
            .collect()
    }

    fn active(&self) -> Option<String> {
        self.active.lock().unwrap().clone()
    }

    /// Opens a tab, makes it the active one, and loads `url` in it.
    async fn open_tab(&self, url: &str) -> Result<Arc<dyn Tab>, String> {
        let created = self
            .cdp
            .call("Target.createTarget", json!({ "url": "about:blank" }), None)
            .await
            .map_err(|error| error.to_string())?;
        let target = created["targetId"].as_str().unwrap_or_default().to_string();

        let deadline = tokio::time::Instant::now() + PAGE_WAIT;
        let page = loop {
            let waiting = self.page_added.notified();
            if let Some(page) = self.page(&target) {
                break page;
            }
            if tokio::time::timeout_at(deadline, waiting).await.is_err() {
                return Err("the new tab did not open in time".into());
            }
        };
        self.make_active(&page.target);

        if url != "about:blank" {
            page.go_to(url).await?;
        }

        Ok(page)
    }

    async fn select(&self, id: &str) -> Result<(), String> {
        if self.page(id).is_none() {
            return Err(format!("no page `{id}`; `pages` lists them"));
        }
        self.make_active(id);

        Ok(())
    }

    async fn close_tab(&self, id: &str) -> Result<(), String> {
        self.cdp
            .call("Target.closeTarget", json!({ "targetId": id }), None)
            .await
            .map_err(|error| error.to_string())?;

        Ok(())
    }

    /// The browser reports a title set by the page's own script only when asked.
    async fn refresh(&self) {
        let Ok(targets) = self.cdp.call("Target.getTargets", json!({}), None).await else {
            return;
        };

        let pages = self.pages.lock().unwrap().clone();
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

    /// The first watcher starts the stream; a later one joins it and gets its own first picture.
    fn viewers_changed(&self) {
        let before = self.streaming.lock().unwrap().clone();
        self.restream();

        let after = self.streaming.lock().unwrap().clone();
        let joined = before.is_some() && before == after && self.common.watched();
        if let Some(page) = after.as_deref().and_then(|id| self.page(id)).filter(|_| joined) {
            self.first_frame(page);
        }
    }

    /// Asks the browser to close, so a shared profile writes its cookies out, and ends it if it does not.
    fn stop(&self) {
        let Some(chromium) = self.me.upgrade() else {
            return;
        };

        tokio::spawn(async move {
            let closing = chromium.cdp.call("Browser.close", json!({}), None);
            let _ = tokio::time::timeout(CLOSE_WAIT, closing).await;
            tokio::time::sleep(CLOSE_WAIT).await;
            chromium.common.stopping.cancel();
        });
    }

    fn devtools_port(&self) -> Option<u16> {
        Some(self.port)
    }

    fn desktop_agent(&self) -> Option<String> {
        let desktop = self.agents.desktop.as_ref()?;

        desktop["userAgent"].as_str().map(str::to_string)
    }

    fn kill(&self) {
        self.tree.kill();
        self.common.stopping.cancel();
    }

    async fn input(&self, input: ViewInput) -> Result<(), String> {
        self.common.touch();
        match input {
            ViewInput::Select { page } => self.select(&page).await?,
            ViewInput::Close { page } => self.close_tab(&page).await?,
            ViewInput::NewPage {} => {
                self.open_tab("about:blank").await?;
            }
            other => self.page_input(other).await?,
        }

        Ok(())
    }
}
