//! Edge's IE mode through IEDriverServer over classic WebDriver. IE mode cannot run hidden, so Edge's window is
//! moved off-screen; the pane gets screenshots while it watches, and the page is driven by script the IE way.

use std::hash::{Hash, Hasher};
use std::process::Stdio;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Method;
use serde_json::{Value, json};

use super::driver::{Common, Driver, Language, Settled, Shot, Size, Spec, Tab, Target, shot_factor};
use super::input::Key;
use super::view::{Frame, ViewInput};
use super::webdriver::{Session, WdError, wait_ready};
use super::{DialogRecord, PageRecord, with_scheme};
use crate::Engine;
use crate::platform::process::{Tree, spawn_owned};

const PAGE_JS: &str = include_str!("ie_page.js");
/// How often the pane's picture is taken while it watches.
const FRAME_EVERY: Duration = Duration::from_millis(800);
/// The longest an action waits for the page it started loading.
const LOAD_WAIT: Duration = Duration::from_secs(20);
const START_WAIT: Duration = Duration::from_secs(20);

pub(crate) struct Ie {
    common: Common,
    me: Weak<Ie>,
    session: Session,
    tree: Tree,
    /// One command at a time: the session's current window and frame are shared by every call.
    lock: tokio::sync::Mutex<()>,
    tabs: Mutex<Vec<Arc<IeTab>>>,
    active: Mutex<Option<String>>,
    /// The window the session is switched to now.
    current: Mutex<Option<String>>,
    last_frame: Mutex<u64>,
}

pub(crate) struct IeTab {
    handle: String,
    popup: bool,
    owner: Weak<Ie>,
    state: Mutex<TabState>,
}

#[derive(Default)]
struct TabState {
    url: String,
    title: String,
    dialog: Option<DialogRecord>,
    /// Screenshot pixels per CSS pixel, so a click at a screenshot point lands where it was seen.
    scale: Option<f64>,
}

fn free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|error| error.to_string())?;
    let port = listener.local_addr().map_err(|error| error.to_string())?.port();

    Ok(port)
}

/// The settings Edge needs for IEDriverServer to find its IE mode window; `docs/browser.md` says why each matters.
fn capabilities(edge: &str, port: u16) -> Value {
    json!({
        "alwaysMatch": {
            "browserName": "internet explorer",
            "unhandledPromptBehavior": "ignore",
            "se:ieOptions": {
                "ie.edgechromium": true,
                "ie.edgepath": edge,
                "ignoreProtectedModeSettings": true,
                "ignoreZoomSetting": true,
                "nativeEvents": false,
                "elementScrollBehavior": 1,
                "initialBrowserUrl": format!("http://127.0.0.1:{port}/"),
                "browserAttachTimeout": 60000,
                "ie.ignoreprocessmatch": true,
            }
        }
    })
}

impl Ie {
    pub(crate) async fn start(engine: Weak<Engine>, spec: Spec) -> Result<Arc<Self>, String> {
        let driver = spec.driver.clone().ok_or("IEDriverServer was not found")?;
        let port = free_port()?;
        let mut command = tokio::process::Command::new(&driver);
        command
            .args([
                format!("/port={port}"),
                "/host=127.0.0.1".into(),
                "/log-level=FATAL".into(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let (child, tree) = spawn_owned(&mut command)
            .await
            .map_err(|error| format!("IEDriverServer could not start: {error}"))?;

        let server = format!("http://127.0.0.1:{port}");
        wait_ready(&server, START_WAIT).await?;
        let edge = spec.program.display().to_string();
        let session = Session::create(&server, capabilities(&edge, port))
            .await
            .map_err(|error| format!("Edge did not open in IE mode: {error}"))?;
        let timeouts = json!({ "pageLoad": 30000, "script": 30000, "implicit": 0 });
        let _ = session.post("/timeouts", timeouts).await;
        // IE mode cannot run without a window, so it is kept out of sight.
        let rect = json!({ "x": -32000, "y": -32000, "width": 1280, "height": 900 });
        let _ = session.post("/window/rect", rect).await;

        let ie = Arc::new_cyclic(|me| Self {
            common: Common::new(engine, spec),
            me: me.clone(),
            session,
            tree,
            lock: tokio::sync::Mutex::new(()),
            tabs: Mutex::default(),
            active: Mutex::default(),
            current: Mutex::default(),
            last_frame: Mutex::new(0),
        });
        {
            let _held = ie.lock.lock().await;
            ie.sync_windows().await;
        }
        tokio::spawn(ie.clone().run(child));

        Ok(ie)
    }

    fn as_driver(&self) -> Option<Arc<dyn Driver>> {
        self.me.upgrade().map(|me| me as Arc<dyn Driver>)
    }

    fn changed(&self) {
        if let Some(driver) = self.as_driver() {
            driver.changed();
        }
    }

    fn tab(&self, handle: &str) -> Option<Arc<IeTab>> {
        self.tabs
            .lock()
            .unwrap()
            .iter()
            .find(|tab| tab.handle == handle)
            .cloned()
    }

    /// Switches the session to a window, at its top document; the lock must be held.
    async fn focus(&self, handle: &str) -> Result<(), String> {
        let switched = self.current.lock().unwrap().as_deref() == Some(handle);
        if !switched {
            self.session
                .post("/window", json!({ "handle": handle }))
                .await
                .map_err(String::from)?;
            *self.current.lock().unwrap() = Some(handle.to_string());
        }
        let _ = self.session.post("/frame", json!({ "id": null })).await;

        Ok(())
    }

    /// Runs one of the page helpers in a window, installing them first on a document that lacks them.
    async fn helper(&self, handle: &str, call: &str, args: Value) -> Result<Value, String> {
        let _held = self.lock.lock().await;
        self.focus(handle).await?;

        let installed = self
            .session
            .execute("return typeof window.__drift;", json!([]))
            .await
            .map_err(|error| self.blocked(handle, error))?;
        if installed != "object" {
            self.session
                .execute(PAGE_JS, json!([]))
                .await
                .map_err(|error| self.blocked(handle, error))?;
        }

        self.session
            .execute(&format!("return window.__drift.{call};"), args)
            .await
            .map_err(|error| self.blocked(handle, error))
    }

    /// A command refused because a dialog is up records the dialog, so the next action can say so.
    fn blocked(&self, handle: &str, error: WdError) -> String {
        if error.code == "unexpected alert open"
            && let Some(tab) = self.tab(handle)
        {
            let message = error.message.lines().next().unwrap_or_default().to_string();
            tab.state.lock().unwrap().dialog = Some(DialogRecord {
                kind: "dialog".into(),
                message,
                default_prompt: None,
            });
            self.changed();
            return "the page is showing a dialog; answer it with the dialog action first".into();
        }

        error.into()
    }

    /// Reads the windows again: a new one is a popup and becomes the active page; a closed one is forgotten.
    async fn sync_windows(&self) {
        let Ok(handles) = self.session.get("/window/handles").await else {
            return;
        };
        let handles: Vec<String> = handles
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|handle| handle.as_str().map(str::to_string))
            .collect();

        let mut added = None;
        {
            let mut tabs = self.tabs.lock().unwrap();
            let first = tabs.is_empty();
            tabs.retain(|tab| handles.contains(&tab.handle));
            for handle in &handles {
                if tabs.iter().any(|tab| &tab.handle == handle) {
                    continue;
                }
                tabs.push(Arc::new(IeTab {
                    handle: handle.clone(),
                    popup: !first,
                    owner: self.me.clone(),
                    state: Mutex::default(),
                }));
                added = Some(handle.clone());
            }
        }

        let active_gone = self
            .active
            .lock()
            .unwrap()
            .as_ref()
            .is_none_or(|active| !handles.contains(active));
        let next = added.or_else(|| active_gone.then(|| handles.last().cloned()).flatten());
        if let Some(handle) = next {
            *self.active.lock().unwrap() = Some(handle);
        }
        if let Some(active) = self.active() {
            let _ = self.focus(&active).await;
            self.sync_page(&active).await;
        }
        self.changed();
    }

    /// The address, title and any dialog of the window the session is on; the lock must be held.
    async fn sync_page(&self, handle: &str) {
        let Some(tab) = self.tab(handle) else {
            return;
        };

        let dialog = self.session.get("/alert/text").await.ok().map(|text| DialogRecord {
            kind: "dialog".into(),
            message: text.as_str().unwrap_or_default().to_string(),
            default_prompt: None,
        });
        let url = self.session.get("/url").await.ok();
        let title = self.session.get("/title").await.ok();

        let mut state = tab.state.lock().unwrap();
        let before = (state.url.clone(), state.title.clone(), state.dialog.is_some());
        state.dialog = dialog;
        if let Some(url) = url.as_ref().and_then(Value::as_str) {
            state.url = url.to_string();
        }
        if let Some(title) = title.as_ref().and_then(Value::as_str) {
            state.title = title.to_string();
        }
        let after = (state.url.clone(), state.title.clone(), state.dialog.is_some());
        drop(state);

        if before != after {
            self.changed();
        }
    }

    /// A screenshot of a window as a JPEG, with its size in CSS pixels for the pane's clicks.
    async fn picture(&self, handle: &str) -> Result<(Shot, (f64, f64)), String> {
        let size = self.helper(handle, "size()", json!([])).await?;
        let (css_width, css_height) = parse_size(size.as_str().unwrap_or_default());

        let _held = self.lock.lock().await;
        self.focus(handle).await?;
        let data = self.session.get("/screenshot").await.map_err(String::from)?;
        let shot = to_jpeg(data.as_str().unwrap_or_default())?;

        Ok((shot, (css_width, css_height)))
    }

    async fn run(self: Arc<Self>, mut child: tokio::process::Child) {
        let mut every = tokio::time::interval(FRAME_EVERY);
        loop {
            tokio::select! {
                _ = every.tick() => {
                    if self.common.watched() {
                        self.send_frame().await;
                    }
                }
                _ = child.wait() => break,
                () = self.common.stopping.cancelled() => break,
            }
        }

        self.common.mark_ended();
        self.session.end().await;
        self.tabs.lock().unwrap().clear();
        self.tree.kill();
        let _ = child.wait().await;
        self.tree.stop().await;

        if let (Some(engine), Some(driver)) = (self.common.engine.upgrade(), self.as_driver()) {
            engine.browser_ended(&driver).await;
        }
    }

    /// The active window's picture, sent only when it changed.
    async fn send_frame(&self) {
        let Some(active) = self.active() else {
            return;
        };
        {
            let _held = self.lock.lock().await;
            self.sync_windows().await;
        }
        let Ok((shot, (width, height))) = self.picture(&active).await else {
            return;
        };

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        shot.jpeg.hash(&mut hasher);
        let hash = hasher.finish();
        if std::mem::replace(&mut *self.last_frame.lock().unwrap(), hash) == hash {
            return;
        }

        self.common.send_frame(Frame {
            page: active,
            width,
            height,
            jpeg: shot.jpeg,
        });
    }

    async fn pane_key(&self, tab: &IeTab, key: &super::KeyInput) -> Result<(), String> {
        if key.event != "down" {
            return Ok(());
        }
        let control = key.modifiers & 6 != 0;
        if key.key.chars().count() == 1 && !control {
            return tab.type_text(&key.key).await;
        }

        let (parsed, _) = super::input::parse_chord(&key.key).unwrap_or((Key::from_input(key), 0));
        tab.press(&parsed, key.modifiers).await
    }

    async fn page_input(&self, input: ViewInput) -> Result<(), String> {
        let active = self.active().ok_or("the browser has no page open")?;
        let tab = self.tab(&active).ok_or("the active page has closed")?;
        match input {
            ViewInput::Mouse(mouse) if mouse.event == "up" => {
                let count = mouse.click_count.unwrap_or(1);
                let call = format!("clickAt({}, {}, {})", mouse.x, mouse.y, count > 1);
                self.helper(&active, &call, json!([])).await?;
            }
            ViewInput::Mouse(mouse) if mouse.event == "wheel" => {
                let dy = mouse.delta_y.unwrap_or_default();
                self.helper(&active, &format!("scroll(0, {dy})"), json!([])).await?;
            }
            ViewInput::Key(key) => self.pane_key(&tab, &key).await?,
            ViewInput::Text { text } => tab.type_text(&text).await?,
            ViewInput::Navigate { url } => tab.go(&url).await?,
            ViewInput::Dialog { accept, text } => tab.answer_dialog(accept, text.as_deref()).await?,
            _ => {}
        }

        Ok(())
    }
}

fn parse_size(text: &str) -> (f64, f64) {
    let mut parts = text.split('x').map(|part| part.trim().parse::<f64>().unwrap_or(0.0));

    (parts.next().unwrap_or(0.0), parts.next().unwrap_or(0.0))
}

/// IEDriverServer gives PNGs; the pane and the model take smaller JPEGs.
pub(crate) fn to_jpeg(base64_png: &str) -> Result<Shot, String> {
    use base64::Engine as _;

    let png = base64::engine::general_purpose::STANDARD
        .decode(base64_png)
        .map_err(|error| error.to_string())?;
    let picture = image::load_from_memory(&png).map_err(|error| error.to_string())?;
    let mut jpeg = Vec::new();
    picture
        .to_rgb8()
        .write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
        .map_err(|error| error.to_string())?;

    Ok(Shot {
        jpeg,
        width: picture.width(),
        height: picture.height(),
    })
}

impl IeTab {
    fn owner(&self) -> Result<Arc<Ie>, String> {
        self.owner.upgrade().ok_or_else(|| "the browser has closed".into())
    }

    async fn helper(&self, call: &str, args: Value) -> Result<Value, String> {
        self.owner()?.helper(&self.handle, call, args).await
    }

    /// Runs a session command on this window at its top document.
    async fn command(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value, String> {
        let owner = self.owner()?;
        let _held = owner.lock.lock().await;
        owner.focus(&self.handle).await?;

        owner
            .session
            .call(method, path, body)
            .await
            .map_err(|error| owner.blocked(&self.handle, error))
    }

    fn to_css(&self, x: f64, y: f64) -> (f64, f64) {
        let scale = self.state.lock().unwrap().scale.unwrap_or(1.0);

        (x / scale, y / scale)
    }

    /// Waits until the page has finished loading, at most 20 s.
    async fn wait_loaded(&self) {
        let deadline = tokio::time::Instant::now() + LOAD_WAIT;
        while tokio::time::Instant::now() < deadline {
            let ready = self
                .command(
                    Method::POST,
                    "/execute/sync",
                    Some(json!({
                        "script": "return document.readyState;",
                        "args": [],
                    })),
                )
                .await;
            match ready {
                Ok(state) if state == "complete" => return,
                Err(error) if error.contains("dialog") => return,
                _ => tokio::time::sleep(Duration::from_millis(250)).await,
            }
        }
    }
}

#[async_trait]
impl Tab for IeTab {
    fn id(&self) -> String {
        self.handle.clone()
    }

    fn record(&self, active: bool) -> PageRecord {
        let state = self.state.lock().unwrap();

        PageRecord {
            id: self.handle.clone(),
            url: state.url.clone(),
            title: state.title.clone(),
            active,
            popup: self.popup,
            dialog: state.dialog.clone(),
        }
    }

    async fn snapshot(&self, interactive: bool) -> Result<String, String> {
        let tree = self.helper(&format!("snapshot({interactive})"), json!([])).await?;

        Ok(tree.as_str().unwrap_or_default().to_string())
    }

    async fn text(&self) -> Result<String, String> {
        let text = self.helper("text()", json!([])).await?;

        Ok(text.as_str().unwrap_or_default().to_string())
    }

    async fn click(&self, target: Target, double: bool) -> Result<String, String> {
        let call = match target {
            Target::Ref(reference) => format!("click({reference}, {double})"),
            Target::Point(x, y) => {
                let (x, y) = self.to_css(x, y);
                format!("clickAt({x}, {y}, {double})")
            }
        };
        let done = self.helper(&call, json!([])).await?;

        Ok(done.as_str().unwrap_or("Clicked.").to_string())
    }

    async fn hover(&self, target: Target) -> Result<String, String> {
        let call = match target {
            Target::Ref(reference) => format!("hover({reference})"),
            Target::Point(x, y) => {
                let (x, y) = self.to_css(x, y);
                format!("hover(0, {x}, {y})")
            }
        };
        let done = self.helper(&call, json!([])).await?;

        Ok(done.as_str().unwrap_or("Hovered.").to_string())
    }

    async fn fill(&self, reference: u64, text: &str, checked: Option<bool>) -> Result<String, String> {
        let checked = checked.map_or("null".to_string(), |checked| checked.to_string());
        let call = format!("fill({reference}, arguments[0], {checked})");
        let done = self.helper(&call, json!([text])).await?;

        Ok(done.as_str().unwrap_or("Filled.").to_string())
    }

    async fn scroll(&self, reference: Option<u64>, dy: f64) -> Result<String, String> {
        let call = format!("scroll({}, {dy})", reference.unwrap_or(0));
        let done = self.helper(&call, json!([])).await?;

        Ok(done.as_str().unwrap_or("Scrolled.").to_string())
    }

    async fn press(&self, key: &Key, modifiers: u8) -> Result<(), String> {
        let (name, code) = key.name_and_code();
        let call = format!(
            "press(arguments[0], {code}, {}, {}, {})",
            modifiers & 2 != 0,
            modifiers & 8 != 0,
            modifiers & 1 != 0
        );
        self.helper(&call, json!([name])).await?;

        Ok(())
    }

    async fn type_text(&self, text: &str) -> Result<(), String> {
        self.helper("type(arguments[0])", json!([text])).await?;

        Ok(())
    }

    async fn go(&self, url: &str) -> Result<(), String> {
        match url {
            "back" => self.command(Method::POST, "/back", None).await?,
            "forward" => self.command(Method::POST, "/forward", None).await?,
            "reload" => self.command(Method::POST, "/refresh", None).await?,
            _ => {
                let body = json!({ "url": with_scheme(url) });
                self.command(Method::POST, "/url", Some(body)).await?
            }
        };
        self.wait_loaded().await;

        Ok(())
    }

    /// A click runs the page's own script, which may start loading another page a moment later.
    async fn settle(&self, url_before: &str) -> Settled {
        tokio::time::sleep(Duration::from_millis(300)).await;
        self.wait_loaded().await;
        if let Ok(owner) = self.owner() {
            let _held = owner.lock.lock().await;
            owner.sync_windows().await;
        }

        let state = self.state.lock().unwrap();
        Settled {
            url: state.url.clone(),
            title: state.title.clone(),
            navigated: state.url != url_before,
            dialog: state.dialog.clone(),
        }
    }

    async fn screenshot(&self) -> Result<Shot, String> {
        let (picture, (css_width, _)) = self.owner()?.picture(&self.handle).await?;
        let shot = Shot::fitted(picture.jpeg)?;
        self.state.lock().unwrap().scale = Some(f64::from(shot.width) / css_width.max(1.0));

        Ok(shot)
    }

    async fn zoom(&self, region: [f64; 4]) -> Result<Shot, String> {
        let (picture, (css_width, _)) = self.owner()?.picture(&self.handle).await?;
        let shot_width = f64::from(picture.width) * shot_factor(picture.width, picture.height);
        self.state.lock().unwrap().scale = Some(shot_width / css_width.max(1.0));

        Shot::zoomed(&picture.jpeg, region)
    }

    async fn evaluate(&self, script: &str, language: Language) -> Result<Value, String> {
        let body = match language {
            Language::JavaScript => "return eval(arguments[0]);",
            Language::VbScript => "window.execScript(arguments[0], 'VBScript'); return 'VBScript ran.';",
        };

        self.command(
            Method::POST,
            "/execute/sync",
            Some(json!({ "script": body, "args": [script] })),
        )
        .await
    }

    fn logs(&self, _network: bool, _matches: &dyn Fn(&str) -> bool, _limit: usize) -> Result<Vec<String>, String> {
        Err("IE mode cannot show console messages or network requests; use eval to read what the page holds".into())
    }

    async fn response_body(&self, _request: &str) -> Result<String, String> {
        Err("IE mode cannot show network requests".into())
    }

    async fn answer_dialog(&self, accept: bool, text: Option<&str>) -> Result<(), String> {
        if let Some(text) = text {
            self.command(Method::POST, "/alert/text", Some(json!({ "text": text })))
                .await?;
        }
        let path = if accept { "/alert/accept" } else { "/alert/dismiss" };
        self.command(Method::POST, path, None).await?;
        self.state.lock().unwrap().dialog = None;
        if let Ok(owner) = self.owner() {
            owner.changed();
        }

        Ok(())
    }

    async fn viewport(&self, size: Option<Size>) -> Result<(), String> {
        let (width, height) = match size {
            Some(size) if size.mobile => return Err("IE mode cannot pretend to be a phone".into()),
            Some(size) => (size.width, size.height),
            None => (1280, 900),
        };
        let rect = json!({ "x": -32000, "y": -32000, "width": width, "height": height });
        self.command(Method::POST, "/window/rect", Some(rect)).await?;

        Ok(())
    }
}

#[async_trait]
impl Driver for Ie {
    fn common(&self) -> &Common {
        &self.common
    }

    fn tabs(&self) -> Vec<Arc<dyn Tab>> {
        self.tabs
            .lock()
            .unwrap()
            .iter()
            .map(|tab| tab.clone() as Arc<dyn Tab>)
            .collect()
    }

    fn active(&self) -> Option<String> {
        self.active.lock().unwrap().clone()
    }

    /// IEDriverServer cannot open a window itself, so the page opens one.
    async fn open_tab(&self, url: &str) -> Result<Arc<dyn Tab>, String> {
        let active = self.active().ok_or("the browser has no page open")?;
        let tab = self.tab(&active).ok_or("the active page has closed")?;
        let before = self.tabs.lock().unwrap().len();
        tab.command(
            Method::POST,
            "/execute/sync",
            Some(json!({ "script": "window.open(arguments[0], '_blank');", "args": [with_scheme(url)] })),
        )
        .await?;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            {
                let _held = self.lock.lock().await;
                self.sync_windows().await;
            }
            if self.tabs.lock().unwrap().len() > before {
                let opened = self.active().and_then(|handle| self.tab(&handle));
                return opened
                    .map(|tab| tab as Arc<dyn Tab>)
                    .ok_or_else(|| "the new window closed at once".into());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        Err("the page could not open a window (a popup blocker may have stopped it)".into())
    }

    async fn select(&self, id: &str) -> Result<(), String> {
        if self.tab(id).is_none() {
            return Err(format!("no page `{id}`; `pages` lists them"));
        }
        *self.active.lock().unwrap() = Some(id.to_string());
        *self.last_frame.lock().unwrap() = 0;
        self.changed();

        Ok(())
    }

    async fn close_tab(&self, id: &str) -> Result<(), String> {
        let tab = self.tab(id).ok_or_else(|| format!("no page `{id}`"))?;
        tab.command(Method::DELETE, "/window", None).await?;
        *self.current.lock().unwrap() = None;

        let _held = self.lock.lock().await;
        self.sync_windows().await;

        Ok(())
    }

    async fn refresh(&self) {
        let Some(active) = self.active() else {
            return;
        };
        let _held = self.lock.lock().await;
        if self.focus(&active).await.is_ok() {
            self.sync_page(&active).await;
        }
    }

    fn viewers_changed(&self) {
        *self.last_frame.lock().unwrap() = 0;
    }

    fn stop(&self) {
        self.common.stopping.cancel();
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

    fn opening_note(&self) -> Option<String> {
        Some(
            "IE mode runs Edge in a window kept off-screen; it cannot show console or network logs, and `eval` \
             can run VBScript with `language: vbscript`."
                .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_is_asked_for_ie_mode_with_the_settings_it_needs() {
        let asked = capabilities(r"C:\Edge\msedge.exe", 5555);
        let options = &asked["alwaysMatch"]["se:ieOptions"];

        assert_eq!(options["ie.edgechromium"], true);
        assert_eq!(options["initialBrowserUrl"], "http://127.0.0.1:5555/");
        assert_eq!(asked["alwaysMatch"]["unhandledPromptBehavior"], "ignore");
        assert_eq!(parse_size("1264x649"), (1264.0, 649.0));
    }
}
