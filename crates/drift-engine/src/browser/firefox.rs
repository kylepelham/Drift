//! Firefox and Camoufox over WebDriver BiDi, always headless. Pages are read through the same helpers as Chrome,
//! in a sandbox the page cannot see; input is real WebDriver input; the pane gets screenshots while it watches.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, watch};

use super::bidi::{Bidi, Event, plain};
use super::driver::{Common, Driver, Language, Pointer, Settled, Shot, Size, Spec, Tab, Target, shot_factor};
use super::input::{Key, parse_chord};
use super::view::{Frame, PaneFit, ViewInput, pane_viewport};
use super::{DialogRecord, Kind, PageRecord, with_scheme};
use crate::Engine;
use crate::platform::process::{Tree, spawn_owned};

const PAGE_JS: &str = include_str!("chromium/page.js");
const SANDBOX: &str = "drift";
const START_WAIT: Duration = Duration::from_secs(30);
const LOAD_WAIT: Duration = Duration::from_secs(15);
/// About 30 pictures a second while a pane watches; Firefox takes one in about 10 ms.
const FRAME_EVERY: Duration = Duration::from_millis(33);
/// After this many unchanged pictures in a row the page is still, and is pictured only every `STILL_EVERY` ticks
/// until it changes or the pane sends input.
const STILL_AFTER: u32 = 15;
const STILL_EVERY: u32 = 8;
/// Console lines and requests kept per page.
const KEPT: usize = 500;

const EVENTS: [&str; 12] = [
    "browsingContext.contextCreated",
    "browsingContext.contextDestroyed",
    "browsingContext.navigationStarted",
    "browsingContext.load",
    "browsingContext.fragmentNavigated",
    "browsingContext.userPromptOpened",
    "browsingContext.userPromptClosed",
    "log.entryAdded",
    "network.beforeRequestSent",
    "network.responseCompleted",
    "network.fetchError",
    "browsingContext.historyUpdated",
];

/// Preferences that turn on BiDi (Camoufox ships with it off) and keep a profile quiet and small.
fn preferences(integrated_auth: &[String]) -> String {
    let mut prefs = vec![
        ("remote.active-protocols", "1".to_string()),
        ("remote.enabled", "true".into()),
        ("browser.shell.checkDefaultBrowser", "false".into()),
        ("browser.startup.homepage_override.mstone", "\"ignore\"".into()),
        ("browser.aboutwelcome.enabled", "false".into()),
        ("datareporting.policy.dataSubmissionEnabled", "false".into()),
        ("toolkit.telemetry.reportingpolicy.firstRun", "false".into()),
        ("app.update.disabledForTesting", "true".into()),
        ("browser.cache.disk.capacity", "51200".into()),
        ("dom.disable_open_during_load", "false".into()),
    ];
    if !integrated_auth.is_empty() {
        let hosts = format!("\"{}\"", integrated_auth.join(",").replace('*', ""));
        prefs.push(("network.negotiate-auth.trusted-uris", hosts.clone()));
        prefs.push(("network.automatic-ntlm-auth.trusted-uris", hosts));
    }

    prefs.into_iter().fold(String::new(), |mut text, (name, value)| {
        let _ = writeln!(text, "user_pref(\"{name}\", {value});");
        text
    })
}

struct Request {
    id: String,
    method: String,
    url: String,
    status: Option<u64>,
    failed: Option<String>,
}

#[derive(Default)]
struct TabState {
    url: String,
    title: String,
    dialog: Option<DialogRecord>,
    console: VecDeque<String>,
    requests: VecDeque<Request>,
    loading: bool,
    scale: Option<f64>,
}

pub(crate) struct FoxTab {
    context: String,
    popup: bool,
    bidi: Arc<Bidi>,
    state: Mutex<TabState>,
    /// Counts the documents that finished loading.
    loads: watch::Sender<u64>,
    pane_fit: PaneFit,
    /// The agent chose a viewport, which the pane's size does not override.
    agent_sized: AtomicBool,
}

pub(crate) struct Firefox {
    common: Common,
    me: Weak<Firefox>,
    bidi: Arc<Bidi>,
    tree: Tree,
    tabs: Mutex<Vec<Arc<FoxTab>>>,
    active: Mutex<Option<String>>,
    last_frame: Mutex<u64>,
    /// Unchanged pictures in a row.
    unchanged: AtomicU32,
    pane_fit: PaneFit,
    pointer: Pointer,
}

/// Launches the browser and reads the BiDi port it prints.
async fn launch(spec: &Spec) -> Result<(tokio::process::Child, Tree, u16), String> {
    let profile = spec.profile.clone().ok_or("Firefox needs a profile folder")?;
    std::fs::create_dir_all(&profile).map_err(|error| error.to_string())?;
    std::fs::write(profile.join("user.js"), preferences(&spec.integrated_auth)).map_err(|error| error.to_string())?;

    let mut command = tokio::process::Command::new(&spec.program);
    command
        .args(["--headless", "--remote-debugging-port", "0", "--no-remote", "--profile"])
        .arg(&profile)
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let (mut child, tree) = spawn_owned(&mut command)
        .await
        .map_err(|error| format!("{} could not start: {error}", spec.kind.label()))?;

    let stderr = child.stderr.take().ok_or("no error stream")?;
    let mut lines = BufReader::new(stderr).lines();
    let port = tokio::time::timeout(START_WAIT, async {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(port) = line
                .split("ws://127.0.0.1:")
                .nth(1)
                .and_then(|rest| rest.trim().parse::<u16>().ok())
            {
                return Some(port);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
    .ok_or_else(|| format!("{} did not open its BiDi port", spec.kind.label()))?;

    // What Firefox prints from now on is not read, but the pipe must keep draining.
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });

    Ok((child, tree, port))
}

impl Firefox {
    pub(crate) async fn start(engine: Weak<Engine>, spec: Spec) -> Result<Arc<Self>, String> {
        let label = spec.kind.label();
        let (child, tree, port) = launch(&spec).await?;
        let (bidi, events) = Bidi::connect(port)
            .await
            .map_err(|error| format!("could not connect to {label}: {error}"))?;
        let capabilities = json!({ "alwaysMatch": {
            "acceptInsecureCerts": true,
            "unhandledPromptBehavior": { "default": "ignore" },
        }});
        bidi.call("session.new", json!({ "capabilities": capabilities }))
            .await
            .map_err(|error| format!("{label} refused a session: {error}"))?;
        bidi.call("session.subscribe", json!({ "events": EVENTS }))
            .await
            .map_err(|error| format!("{label} refused its events: {error}"))?;

        let firefox = Arc::new_cyclic(|me| Self {
            common: Common::new(engine, spec),
            me: me.clone(),
            bidi: bidi.clone(),
            tree,
            tabs: Mutex::default(),
            active: Mutex::default(),
            last_frame: Mutex::new(0),
            unchanged: AtomicU32::new(0),
            pane_fit: PaneFit::default(),
            pointer: Pointer::default(),
        });
        let tree = bidi
            .call("browsingContext.getTree", json!({ "maxDepth": 0 }))
            .await
            .unwrap_or_default();
        for context in tree["contexts"].as_array().into_iter().flatten() {
            firefox.add(context, false);
        }
        if firefox.common.kind == Kind::Camoufox {
            firefox.pass_as_firefox().await;
        }
        tokio::spawn(firefox.clone().run(events, child));

        Ok(firefox)
    }

    /// Camoufox names itself in its user agent, and sites that do not know the name serve their oldest pages, so
    /// pages are told it is the Firefox it is built on.
    async fn pass_as_firefox(&self) {
        let Some(tab) = self.tabs.lock().unwrap().first().cloned() else {
            return;
        };
        let Ok(agent) = tab.sandboxed("navigator.userAgent").await else {
            return;
        };
        let Some(agent) = agent.as_str().and_then(firefox_agent) else {
            return;
        };

        let _ = self
            .bidi
            .call("emulation.setUserAgentOverride", json!({ "userAgent": agent }))
            .await;
    }

    fn as_driver(&self) -> Option<Arc<dyn Driver>> {
        self.me.upgrade().map(|me| me as Arc<dyn Driver>)
    }

    fn changed(&self) {
        if let Some(driver) = self.as_driver() {
            driver.changed();
        }
    }

    fn tab(&self, context: &str) -> Option<Arc<FoxTab>> {
        self.tabs
            .lock()
            .unwrap()
            .iter()
            .find(|tab| tab.context == context)
            .cloned()
    }

    fn add(&self, context: &Value, popup: bool) -> Option<Arc<FoxTab>> {
        let id = context["context"].as_str()?.to_string();
        if self.tab(&id).is_some() {
            return None;
        }

        let tab = Arc::new(FoxTab {
            context: id.clone(),
            popup,
            bidi: self.bidi.clone(),
            state: Mutex::new(TabState {
                url: context["url"].as_str().unwrap_or_default().to_string(),
                ..TabState::default()
            }),
            loads: watch::channel(0).0,
            pane_fit: self.pane_fit.clone(),
            agent_sized: AtomicBool::new(false),
        });
        self.tabs.lock().unwrap().push(tab.clone());
        if self.pane_fit.lock().unwrap().is_some() {
            let fitting = tab.clone();
            tokio::spawn(async move { fitting.fit_pane().await });
        }
        let mut active = self.active.lock().unwrap();
        if active.is_none() || popup {
            *active = Some(id);
        }
        drop(active);
        self.changed();

        Some(tab)
    }

    async fn run(self: Arc<Self>, mut events: mpsc::UnboundedReceiver<Event>, mut child: tokio::process::Child) {
        let mut every = tokio::time::interval(FRAME_EVERY);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut ticks: u32 = 0;
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    Some(event) => self.dispatch(&event),
                    None => break,
                },
                _ = every.tick() => {
                    ticks = ticks.wrapping_add(1);
                    let still = self.unchanged.load(Ordering::SeqCst) >= STILL_AFTER;
                    if self.common.watched() && (!still || ticks.is_multiple_of(STILL_EVERY)) {
                        self.send_frame().await;
                    }
                }
                _ = child.wait() => break,
                () = self.common.stopping.cancelled() => break,
            }
        }

        self.common.mark_ended();
        self.tabs.lock().unwrap().clear();
        self.tree.kill();
        let _ = child.wait().await;
        self.tree.stop().await;

        if let (Some(engine), Some(driver)) = (self.common.engine.upgrade(), self.as_driver()) {
            engine.browser_ended(&driver).await;
        }
    }

    fn dispatch(&self, event: &Event) {
        let params = &event.params;
        match event.method.as_str() {
            "browsingContext.contextCreated" if params["parent"].is_null() => {
                let opened_by_page = !params["originalOpener"].is_null();
                self.add(params, opened_by_page);
            }
            "browsingContext.contextDestroyed" => self.forget(params["context"].as_str().unwrap_or_default()),
            _ => {
                let context = params["context"]
                    .as_str()
                    .or_else(|| params["source"]["context"].as_str())
                    .unwrap_or_default();
                let Some(tab) = self.tab(context) else {
                    return;
                };
                if tab.handle(&event.method, params) {
                    self.changed();
                }
                if event.method == "browsingContext.load" {
                    self.refresh_soon(tab);
                }
            }
        }
    }

    fn refresh_soon(&self, tab: Arc<FoxTab>) {
        let Some(firefox) = self.me.upgrade() else {
            return;
        };

        tokio::spawn(async move {
            if tab.read_title().await {
                firefox.changed();
            }
        });
    }

    fn forget(&self, context: &str) {
        let mut tabs = self.tabs.lock().unwrap();
        tabs.retain(|tab| tab.context != context);
        let fallback = tabs.last().map(|tab| tab.context.clone());
        drop(tabs);

        let mut active = self.active.lock().unwrap();
        if active.as_deref() == Some(context) {
            *active = fallback;
        }
        drop(active);
        self.changed();
    }

    async fn send_frame(&self) {
        let Some(tab) = self.active().and_then(|id| self.tab(&id)) else {
            return;
        };
        let Ok((jpeg, (width, height))) = tab.picture().await else {
            return;
        };

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        jpeg.hash(&mut hasher);
        let hash = hasher.finish();
        if std::mem::replace(&mut *self.last_frame.lock().unwrap(), hash) == hash {
            self.unchanged.fetch_add(1, Ordering::SeqCst);
            return;
        }
        self.unchanged.store(0, Ordering::SeqCst);

        self.common.send_frame(Frame {
            page: tab.context.clone(),
            width,
            height,
            jpeg,
        });
    }

    async fn page_input(&self, input: ViewInput) -> Result<(), String> {
        self.unchanged.store(0, Ordering::SeqCst);
        if let ViewInput::Fit { width, height } = input {
            self.fit(width, height);
            return Ok(());
        }
        let tab = self
            .active()
            .and_then(|id| self.tab(&id))
            .ok_or("the browser has no page open")?;
        match input {
            ViewInput::Mouse(mouse) => {
                let button = match mouse.button.as_deref() {
                    Some("middle") => 1,
                    Some("right") => 2,
                    _ => 0,
                };
                let at = json!({ "type": "pointerMove", "x": mouse.x.round(), "y": mouse.y.round() });
                let actions = match mouse.event.as_str() {
                    "down" => vec![at, json!({ "type": "pointerDown", "button": button })],
                    "up" => vec![at, json!({ "type": "pointerUp", "button": button })],
                    "wheel" => return tab.wheel(mouse.x, mouse.y, mouse.delta_y.unwrap_or_default()).await,
                    _ => vec![at],
                };
                tab.pointer(actions).await?;
                self.follow_pointer(mouse.x, mouse.y);
            }
            ViewInput::Key(key) => {
                let kind = if key.event == "up" { "keyUp" } else { "keyDown" };
                let value = Key::from_input(&key).webdriver_value();
                tab.keys(vec![json!({ "type": kind, "value": value })]).await?;
            }
            ViewInput::Text { text } => tab.type_text(&text).await?,
            ViewInput::Navigate { url } => tab.go(&url).await?,
            ViewInput::Dialog { accept, text } => tab.answer_dialog(accept, text.as_deref()).await?,
            _ => {}
        }

        Ok(())
    }

    /// Lays every page out to fill the pane, as Chrome's are; pages opened later are laid out the same.
    fn fit(&self, width: f64, height: f64) {
        let Some(viewport) = pane_viewport(width, height) else {
            return;
        };
        if self.pane_fit.lock().unwrap().replace(viewport) == Some(viewport) {
            return;
        }

        for tab in self.tabs.lock().unwrap().iter() {
            let tab = tab.clone();
            tokio::spawn(async move { tab.fit_pane().await });
        }
    }

    /// Looks up the cursor under the pane's pointer for the pane to show.
    fn follow_pointer(&self, x: f64, y: f64) {
        if !self.pointer.moved(x, y) {
            return;
        }
        let Some(firefox) = self.me.upgrade() else {
            return;
        };

        tokio::spawn(async move {
            while let Some((x, y)) = firefox.pointer.next() {
                let tab = firefox.active().and_then(|id| firefox.tab(&id));
                let tab = tab.filter(|tab| tab.state.lock().unwrap().dialog.is_none());
                if let Some(tab) = tab
                    && let Ok(cursor) = tab.helper(&format!("cursor({x}, {y})")).await
                {
                    firefox.common.set_cursor(cursor.as_str().unwrap_or("default"));
                }
            }
        });
    }
}

fn push<T>(list: &mut VecDeque<T>, item: T) {
    if list.len() == KEPT {
        list.pop_front();
    }
    list.push_back(item);
}

impl FoxTab {
    async fn full_picture(&self) -> Result<Vec<u8>, String> {
        use base64::Engine as _;

        let params = json!({ "context": self.context, "format": { "type": "image/jpeg", "quality": 0.9 } });
        let shot = self.bidi.call("browsingContext.captureScreenshot", params).await?;

        base64::engine::general_purpose::STANDARD
            .decode(shot["data"].as_str().unwrap_or_default())
            .map_err(|error| error.to_string())
    }

    /// Clicks at screenshot points land where they were seen: screenshot pixels per CSS pixel.
    async fn note_shot_width(&self, shot_width: f64) {
        let inner = self.sandboxed("innerWidth").await.ok().and_then(|width| width.as_f64());
        if let Some(inner) = inner {
            self.state.lock().unwrap().scale = Some(shot_width / inner.max(1.0));
        }
    }

    /// Takes in one of the page's events; true when what the pane shows of it changed.
    fn handle(&self, method: &str, params: &Value) -> bool {
        let mut state = self.state.lock().unwrap();
        match method {
            "browsingContext.navigationStarted" => {
                state.loading = true;
                false
            }
            "browsingContext.load" | "browsingContext.fragmentNavigated" | "browsingContext.historyUpdated" => {
                state.loading = false;
                params["url"].as_str().unwrap_or_default().clone_into(&mut state.url);
                drop(state);
                self.loads.send_modify(|count| *count += 1);
                true
            }
            "browsingContext.userPromptOpened" => {
                state.dialog = Some(DialogRecord {
                    kind: params["type"].as_str().unwrap_or("alert").into(),
                    message: params["message"].as_str().unwrap_or_default().into(),
                    default_prompt: params["defaultValue"].as_str().map(str::to_string),
                });
                true
            }
            "browsingContext.userPromptClosed" => state.dialog.take().is_some(),
            "log.entryAdded" => {
                let level = params["level"].as_str().unwrap_or("info");
                let text = params["text"].as_str().unwrap_or_default();
                push(&mut state.console, format!("[{level}] {text}"));
                false
            }
            _ => {
                note_request(&mut state, method, params);
                false
            }
        }
    }

    async fn sandboxed(&self, expression: &str) -> Result<Value, String> {
        let params = json!({
            "expression": expression,
            "target": { "context": self.context, "sandbox": SANDBOX },
            "awaitPromise": true,
            "resultOwnership": "none",
        });
        let result = self.bidi.call("script.evaluate", params).await?;

        evaluated(&result)
    }

    /// Runs one of the page helpers out of the page's sight, installing them on a document that lacks them.
    async fn helper(&self, call: &str) -> Result<Value, String> {
        match self.sandboxed(&format!("__drift.{call}")).await {
            Err(error) if error.contains("__drift") => {
                self.sandboxed(PAGE_JS).await?;
                self.sandboxed(&format!("__drift.{call}")).await
            }
            result => result,
        }
    }

    async fn read_title(&self) -> bool {
        let Ok(title) = self.sandboxed("document.title").await else {
            return false;
        };
        let title = title.as_str().unwrap_or_default().to_string();
        let mut state = self.state.lock().unwrap();

        std::mem::replace(&mut state.title, title) != state.title
    }

    async fn perform(&self, source: Value) -> Result<(), String> {
        let params = json!({ "context": self.context, "actions": [source] });
        self.bidi.call("input.performActions", params).await?;

        Ok(())
    }

    async fn pointer(&self, actions: Vec<Value>) -> Result<(), String> {
        let source = json!({
            "type": "pointer",
            "id": "mouse",
            "parameters": { "pointerType": "mouse" },
            "actions": actions,
        });

        self.perform(source).await
    }

    async fn keys(&self, actions: Vec<Value>) -> Result<(), String> {
        self.perform(json!({ "type": "key", "id": "keyboard", "actions": actions }))
            .await
    }

    async fn wheel(&self, x: f64, y: f64, dy: f64) -> Result<(), String> {
        let scroll = json!({ "type": "scroll", "x": x.round(), "y": y.round(), "deltaX": 0, "deltaY": dy.round() });

        self.perform(json!({ "type": "wheel", "id": "wheel", "actions": [scroll] }))
            .await
    }

    async fn click_at(&self, x: f64, y: f64, double: bool) -> Result<(), String> {
        let mut actions = vec![json!({ "type": "pointerMove", "x": x.round(), "y": y.round() })];
        let presses = if double { 2 } else { 1 };
        for _ in 0..presses {
            actions.push(json!({ "type": "pointerDown", "button": 0 }));
            actions.push(json!({ "type": "pointerUp", "button": 0 }));
        }

        self.pointer(actions).await
    }

    async fn point(&self, target: Target) -> Result<(f64, f64, String), String> {
        match target {
            Target::Ref(reference) => {
                let point = self.helper(&format!("point({reference})")).await?;
                let x = point["x"].as_f64().ok_or("the element has no position")?;
                let y = point["y"].as_f64().ok_or("the element has no position")?;
                Ok((x, y, format!("ref {reference}")))
            }
            Target::Point(x, y) => {
                let scale = self.state.lock().unwrap().scale.unwrap_or(1.0);
                Ok((x / scale, y / scale, format!("{x},{y}")))
            }
        }
    }

    async fn wait_loaded(&self, before: u64) {
        let mut loads = self.loads.subscribe();
        let deadline = tokio::time::Instant::now() + LOAD_WAIT;

        while *loads.borrow_and_update() <= before && self.state.lock().unwrap().dialog.is_none() {
            let waited = tokio::time::timeout_at(deadline, loads.changed()).await;
            if !matches!(waited, Ok(Ok(()))) {
                return;
            }
        }
    }

    /// A picture for the pane at the page's own resolution, with the page's size in CSS pixels.
    async fn picture(&self) -> Result<(Vec<u8>, (f64, f64)), String> {
        let jpeg = self.full_picture().await?;
        let size = self.sandboxed("[innerWidth, innerHeight]").await?;
        let width = size[0].as_f64().unwrap_or_default();
        let height = size[1].as_f64().unwrap_or_default();

        Ok((jpeg, (width, height)))
    }

    /// Lays the page out at the size its pane asks for, unless the agent chose one.
    async fn fit_pane(&self) {
        let Some((width, height)) = *self.pane_fit.lock().unwrap() else {
            return;
        };
        if self.agent_sized.load(Ordering::SeqCst) {
            return;
        }

        let viewport = json!({ "width": width, "height": height });
        let params = json!({ "context": self.context, "viewport": viewport });
        let _ = self.bidi.call("browsingContext.setViewport", params).await;
    }
}

fn note_request(state: &mut TabState, method: &str, params: &Value) {
    let id = params["request"]["request"].as_str().unwrap_or_default();
    match method {
        "network.beforeRequestSent" => {
            let request = Request {
                id: id.to_string(),
                method: params["request"]["method"].as_str().unwrap_or("GET").into(),
                url: params["request"]["url"].as_str().unwrap_or_default().into(),
                status: None,
                failed: None,
            };
            push(&mut state.requests, request);
        }
        "network.responseCompleted" => {
            if let Some(request) = state.requests.iter_mut().rev().find(|request| request.id == id) {
                request.status = params["response"]["status"].as_u64();
            }
        }
        "network.fetchError" => {
            if let Some(request) = state.requests.iter_mut().rev().find(|request| request.id == id) {
                request.failed = params["errorText"].as_str().map(str::to_string);
            }
        }
        _ => {}
    }
}

/// `… Camoufox/156.0.1-beta.36` as the `… Firefox/156.0` it is built on; none when it already reads as Firefox.
fn firefox_agent(agent: &str) -> Option<String> {
    let (before, version) = agent.split_once("Camoufox/")?;
    let major: String = version.chars().take_while(char::is_ascii_digit).collect();
    if major.is_empty() {
        return None;
    }

    Some(format!("{before}Firefox/{major}.0"))
}

/// The value of an evaluation, or its exception as the error.
fn evaluated(result: &Value) -> Result<Value, String> {
    if result["type"] == "exception" {
        let details = &result["exceptionDetails"];
        let text = details["text"].as_str().unwrap_or("the script threw");
        return Err(text.trim_start_matches("Error: ").to_string());
    }

    Ok(plain(&result["result"]))
}

fn request_line(request: &Request) -> String {
    let outcome = match (&request.failed, request.status) {
        (Some(error), _) => format!("failed: {error}"),
        (None, Some(status)) => status.to_string(),
        (None, None) => "pending".into(),
    };

    format!("{} {} {} (id {})", request.method, outcome, request.url, request.id)
}

#[async_trait]
impl Tab for FoxTab {
    fn id(&self) -> String {
        self.context.clone()
    }

    fn record(&self, active: bool) -> PageRecord {
        let state = self.state.lock().unwrap();

        PageRecord {
            id: self.context.clone(),
            url: state.url.clone(),
            title: state.title.clone(),
            active,
            popup: self.popup,
            dialog: state.dialog.clone(),
        }
    }

    async fn snapshot(&self, interactive: bool) -> Result<String, String> {
        let options = json!({ "interactive": interactive });
        let tree = self.helper(&format!("snapshot({options})")).await?;

        Ok(tree.as_str().unwrap_or_default().to_string())
    }

    async fn text(&self) -> Result<String, String> {
        let text = self.helper("text()").await?;

        Ok(text.as_str().unwrap_or_default().to_string())
    }

    async fn click(&self, target: Target, double: bool) -> Result<String, String> {
        let (x, y, what) = self.point(target).await?;
        self.click_at(x, y, double).await?;

        Ok(format!("Clicked {what}."))
    }

    async fn hover(&self, target: Target) -> Result<String, String> {
        let (x, y, what) = self.point(target).await?;
        self.pointer(vec![json!({ "type": "pointerMove", "x": x.round(), "y": y.round() })])
            .await?;

        Ok(format!("Hovered {what}."))
    }

    async fn fill(&self, reference: u64, text: &str, checked: Option<bool>) -> Result<String, String> {
        let checked = checked.map_or("undefined".to_string(), |checked| checked.to_string());
        let prepared = self
            .helper(&format!("prepare({reference}, {}, {checked})", json!(text)))
            .await?;

        if let Some(done) = prepared["done"].as_str() {
            return Ok(format!("Ref {reference}: {done}."));
        }
        if let Some(point) = prepared.get("click") {
            let x = point["x"].as_f64().unwrap_or_default();
            let y = point["y"].as_f64().unwrap_or_default();
            self.click_at(x, y, false).await?;
            return Ok(format!("Clicked ref {reference} to change it."));
        }

        // The field's text is selected, so typing replaces it; an empty value deletes it.
        if text.is_empty() {
            let (delete, _) = parse_chord("Delete")?;
            self.press(&delete, 0).await?;
            return Ok(format!("Cleared ref {reference}."));
        }
        self.type_text(text).await?;

        Ok(format!("Filled ref {reference}."))
    }

    async fn scroll(&self, reference: Option<u64>, dy: f64) -> Result<String, String> {
        if let Some(reference) = reference {
            self.helper(&format!("scroll({reference})")).await?;
            return Ok(format!("Scrolled ref {reference} into view."));
        }
        self.wheel(400.0, 300.0, dy).await?;

        Ok(format!("Scrolled {dy} pixels."))
    }

    async fn press(&self, key: &Key, modifiers: u8) -> Result<(), String> {
        let held = super::input::webdriver_modifiers(modifiers);
        let mut actions: Vec<Value> = held
            .iter()
            .map(|value| json!({ "type": "keyDown", "value": value }))
            .collect();
        let value = key.webdriver_value();
        actions.push(json!({ "type": "keyDown", "value": value }));
        actions.push(json!({ "type": "keyUp", "value": value }));
        actions.extend(
            held.iter()
                .rev()
                .map(|value| json!({ "type": "keyUp", "value": value })),
        );

        self.keys(actions).await
    }

    async fn type_text(&self, text: &str) -> Result<(), String> {
        let actions: Vec<Value> = text
            .chars()
            .flat_map(|character| {
                let value = character.to_string();
                [
                    json!({ "type": "keyDown", "value": value }),
                    json!({ "type": "keyUp", "value": value }),
                ]
            })
            .collect();

        self.keys(actions).await
    }

    async fn go(&self, url: &str) -> Result<(), String> {
        let (method, params) = match url {
            "back" => (
                "browsingContext.traverseHistory",
                json!({ "context": self.context, "delta": -1 }),
            ),
            "forward" => (
                "browsingContext.traverseHistory",
                json!({ "context": self.context, "delta": 1 }),
            ),
            "reload" => (
                "browsingContext.reload",
                json!({ "context": self.context, "wait": "complete" }),
            ),
            _ => (
                "browsingContext.navigate",
                json!({ "context": self.context, "url": with_scheme(url), "wait": "complete" }),
            ),
        };
        let before = *self.loads.borrow();
        self.bidi.call(method, params).await?;
        if method == "browsingContext.traverseHistory" {
            self.wait_loaded(before).await;
        }
        self.read_title().await;

        Ok(())
    }

    async fn settle(&self, url_before: &str) -> Settled {
        let before = *self.loads.borrow();
        tokio::time::sleep(Duration::from_millis(150)).await;
        if self.state.lock().unwrap().loading {
            self.wait_loaded(before).await;
        }
        self.read_title().await;

        let state = self.state.lock().unwrap();
        Settled {
            url: state.url.clone(),
            title: state.title.clone(),
            navigated: state.url != url_before,
            dialog: state.dialog.clone(),
        }
    }

    async fn screenshot(&self) -> Result<Shot, String> {
        let shot = Shot::fit(self.full_picture().await?).await?;
        self.note_shot_width(f64::from(shot.width)).await;

        Ok(shot)
    }

    async fn zoom(&self, region: [f64; 4]) -> Result<Shot, String> {
        let picture = self.full_picture().await?;
        let (width, height) = image::ImageReader::new(std::io::Cursor::new(&picture))
            .with_guessed_format()
            .map_err(|error| error.to_string())?
            .into_dimensions()
            .map_err(|error| error.to_string())?;
        self.note_shot_width(f64::from(width) * shot_factor(width, height))
            .await;

        Shot::zoom(picture, region).await
    }

    async fn evaluate(&self, script: &str, language: Language) -> Result<Value, String> {
        if language == Language::VbScript {
            return Err("VBScript runs only in IE mode".into());
        }
        let params = json!({
            "expression": script,
            "target": { "context": self.context },
            "awaitPromise": true,
            "resultOwnership": "none",
            "userActivation": true,
        });
        let result = self.bidi.call("script.evaluate", params).await?;

        evaluated(&result)
    }

    fn logs(&self, network: bool, matches: &dyn Fn(&str) -> bool, limit: usize) -> Result<Vec<String>, String> {
        let state = self.state.lock().unwrap();
        let lines: Vec<String> = if network {
            state.requests.iter().map(request_line).collect()
        } else {
            state.console.iter().cloned().collect()
        };
        let kept: Vec<String> = lines.into_iter().filter(|line| matches(line)).collect();
        let skip = kept.len().saturating_sub(limit);

        Ok(kept.into_iter().skip(skip).collect())
    }

    async fn response_body(&self, _request: &str) -> Result<String, String> {
        Err("Firefox does not hand response bodies to Drift; use eval with fetch to read one again".into())
    }

    async fn answer_dialog(&self, accept: bool, text: Option<&str>) -> Result<(), String> {
        let mut params = json!({ "context": self.context, "accept": accept });
        if let Some(text) = text {
            params["userText"] = json!(text);
        }
        self.bidi.call("browsingContext.handleUserPrompt", params).await?;
        self.state.lock().unwrap().dialog = None;

        Ok(())
    }

    async fn viewport(&self, size: Option<Size>) -> Result<(), String> {
        self.agent_sized.store(size.is_some(), Ordering::SeqCst);
        let (viewport, ratio) = match size {
            Some(size) => (
                json!({ "width": size.width, "height": size.height }),
                if size.mobile { json!(2) } else { Value::Null },
            ),
            // The pane's size takes over again from the one the agent chose.
            None => match *self.pane_fit.lock().unwrap() {
                Some((width, height)) => (json!({ "width": width, "height": height }), Value::Null),
                None => (Value::Null, Value::Null),
            },
        };
        let params = json!({ "context": self.context, "viewport": viewport, "devicePixelRatio": ratio });
        self.bidi.call("browsingContext.setViewport", params).await?;

        self.go("reload").await
    }
}

#[async_trait]
impl Driver for Firefox {
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

    async fn open_tab(&self, url: &str) -> Result<Arc<dyn Tab>, String> {
        let created = self
            .bidi
            .call("browsingContext.create", json!({ "type": "tab" }))
            .await?;
        let context = created["context"].as_str().unwrap_or_default().to_string();
        let tab = match self.tab(&context) {
            Some(tab) => tab,
            None => self
                .add(&json!({ "context": context, "url": "about:blank" }), false)
                .ok_or("the new tab closed at once")?,
        };
        self.select(&context).await?;
        if url != "about:blank" {
            tab.go(url).await?;
        }

        Ok(tab)
    }

    async fn select(&self, id: &str) -> Result<(), String> {
        if self.tab(id).is_none() {
            return Err(format!("no page `{id}`; `pages` lists them"));
        }
        let _ = self
            .bidi
            .call("browsingContext.activate", json!({ "context": id }))
            .await;
        *self.active.lock().unwrap() = Some(id.to_string());
        *self.last_frame.lock().unwrap() = 0;
        self.changed();

        Ok(())
    }

    async fn close_tab(&self, id: &str) -> Result<(), String> {
        self.bidi
            .call("browsingContext.close", json!({ "context": id }))
            .await?;

        Ok(())
    }

    async fn refresh(&self) {
        let tabs = self.tabs.lock().unwrap().clone();
        let mut changed = false;
        for tab in tabs {
            changed |= tab.read_title().await;
        }

        if changed {
            self.changed();
        }
    }

    fn viewers_changed(&self) {
        *self.last_frame.lock().unwrap() = 0;
    }

    fn stop(&self) {
        let Some(firefox) = self.me.upgrade() else {
            return;
        };

        tokio::spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(5), firefox.bidi.call("browser.close", json!({}))).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
            firefox.common.stopping.cancel();
        });
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
        Some(format!(
            "{} runs headless; it cannot hand over response bodies, and a phone-sized viewport keeps the desktop user agent.",
            self.common.kind.label()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_turns_on_bidi_and_trusts_intranet_sign_in() {
        let prefs = preferences(&["*.corp.local".into()]);

        assert!(prefs.contains("user_pref(\"remote.active-protocols\", 1);"));
        assert!(prefs.contains("user_pref(\"network.negotiate-auth.trusted-uris\", \".corp.local\");"));
    }

    #[test]
    fn camoufox_reads_as_the_firefox_it_is_built_on() {
        let camoufox = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:156.0) Gecko/20100101 Camoufox/156.0.1-beta.36";

        assert_eq!(
            firefox_agent(camoufox).as_deref(),
            Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:156.0) Gecko/20100101 Firefox/156.0")
        );
        assert_eq!(firefox_agent("Mozilla/5.0 (X11) Gecko/20100101 Firefox/156.0"), None);
    }

    #[test]
    fn an_exception_is_the_error() {
        let thrown = json!({ "type": "exception", "exceptionDetails": { "text": "Error: ref 3 is gone" } });
        let fine = json!({ "type": "success", "result": { "type": "number", "value": 4 } });

        assert_eq!(evaluated(&thrown), Err("ref 3 is gone".to_string()));
        assert_eq!(evaluated(&fine), Ok(json!(4)));
    }
}
