//! One tab: its address and title, what it logged, a dialog waiting on it, and the actions taken on it.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::watch;

use super::DialogRecord;
use super::PageRecord;
use super::cdp::{CdpError, Connection};
use super::input::Key;

/// The helpers `page.js` installs in the isolated world.
const PAGE_JS: &str = include_str!("page.js");
const WORLD: &str = "drift";
/// Console lines and requests kept per page; older ones are dropped.
const KEPT: usize = 500;
/// The longest an action waits for the page it started loading.
const LOAD_WAIT: Duration = Duration::from_secs(15);
/// How long after an action a navigation it caused is still looked for.
const SETTLE: Duration = Duration::from_millis(150);

struct Request {
    id: String,
    method: String,
    url: String,
    kind: String,
    status: Option<u64>,
    failed: Option<String>,
}

#[derive(Default)]
struct State {
    url: String,
    title: String,
    main_frame: Option<String>,
    loading: bool,
    world: Option<i64>,
    console: VecDeque<String>,
    requests: VecDeque<Request>,
    dialog: Option<DialogRecord>,
    /// Image pixels per CSS pixel in the last screenshot, so a click at its coordinates lands where it was seen.
    shot_scale: Option<f64>,
}

pub(crate) struct Page {
    pub(crate) target: String,
    pub(crate) session: String,
    pub(crate) popup: bool,
    cdp: Arc<Connection>,
    state: Mutex<State>,
    /// Counts the documents that finished loading.
    loads: watch::Sender<u64>,
}

/// What an action changed, for its result to say.
pub(crate) struct Settled {
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) navigated: bool,
    pub(crate) dialog: Option<DialogRecord>,
}

impl Page {
    pub(crate) fn new(target: String, session: String, popup: bool, cdp: Arc<Connection>, url: String) -> Arc<Self> {
        Arc::new(Self {
            target,
            session,
            popup,
            cdp,
            state: Mutex::new(State {
                url,
                ..State::default()
            }),
            loads: watch::channel(0).0,
        })
    }

    /// Turns on the events a test needs and lets the page run; `user_agent` replaces a headless one first.
    pub(crate) async fn setup(&self, user_agent: Option<&Value>) -> Result<(), CdpError> {
        for domain in ["Page.enable", "Runtime.enable", "Network.enable", "Log.enable"] {
            self.call(domain, json!({})).await?;
        }
        if let Some(user_agent) = user_agent {
            self.call("Emulation.setUserAgentOverride", user_agent.clone()).await?;
        }

        let tree = self.call("Page.getFrameTree", json!({})).await?;
        let frame = &tree["frameTree"]["frame"];
        {
            let mut state = self.state.lock().unwrap();
            state.main_frame = frame["id"].as_str().map(str::to_string);
            if let Some(url) = frame["url"].as_str() {
                state.url = url.into();
            }
        }

        // A page that was not paused answers this with an error, which means nothing.
        let _ = self.call("Runtime.runIfWaitingForDebugger", json!({})).await;

        Ok(())
    }

    pub(crate) async fn call(&self, method: &str, params: Value) -> Result<Value, CdpError> {
        self.cdp.call(method, params, Some(&self.session)).await
    }

    pub(crate) fn send(&self, method: &str, params: Value) {
        self.cdp.send(method, params, Some(&self.session));
    }

    pub(crate) fn record(&self, active: bool) -> PageRecord {
        let state = self.state.lock().unwrap();

        PageRecord {
            id: self.target.clone(),
            url: state.url.clone(),
            title: state.title.clone(),
            active,
            popup: self.popup,
            dialog: state.dialog.clone(),
        }
    }

    pub(crate) fn url(&self) -> String {
        self.state.lock().unwrap().url.clone()
    }

    pub(crate) fn dialog(&self) -> Option<DialogRecord> {
        self.state.lock().unwrap().dialog.clone()
    }

    /// The address and title the browser reports for the tab; true when either changed.
    pub(crate) fn set_info(&self, url: &str, title: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        let changed = state.url != url || state.title != title;
        url.clone_into(&mut state.url);
        title.clone_into(&mut state.title);

        changed
    }

    /// Takes in one of the page's events; true when what the pane shows of it changed.
    pub(crate) fn handle(&self, method: &str, params: &Value) -> bool {
        match method {
            "Page.frameNavigated" => self.navigated(&params["frame"]),
            "Page.navigatedWithinDocument" => self.moved_within(params),
            "Page.frameStartedLoading" | "Page.frameStoppedLoading" => {
                self.loading(params, method == "Page.frameStartedLoading");
                false
            }
            "Runtime.executionContextsCleared" => {
                self.state.lock().unwrap().world = None;
                false
            }
            "Page.javascriptDialogOpening" => {
                self.state.lock().unwrap().dialog = Some(DialogRecord {
                    kind: params["type"].as_str().unwrap_or("alert").into(),
                    message: params["message"].as_str().unwrap_or_default().into(),
                    default_prompt: params["defaultPrompt"].as_str().map(str::to_string),
                });
                true
            }
            "Page.javascriptDialogClosed" => self.state.lock().unwrap().dialog.take().is_some(),
            _ => {
                self.log(method, params);
                false
            }
        }
    }

    fn navigated(&self, frame: &Value) -> bool {
        if frame.get("parentId").is_some() {
            return false;
        }

        let url = format!(
            "{}{}",
            frame["url"].as_str().unwrap_or_default(),
            frame["urlFragment"].as_str().unwrap_or_default()
        );
        let mut state = self.state.lock().unwrap();
        state.main_frame = frame["id"].as_str().map(str::to_string);
        state.world = None;
        push(&mut state.console, format!("--- navigated to {url}"));
        state.url = url;

        true
    }

    fn moved_within(&self, params: &Value) -> bool {
        let mut state = self.state.lock().unwrap();
        if params["frameId"].as_str() != state.main_frame.as_deref() {
            return false;
        }
        params["url"].as_str().unwrap_or_default().clone_into(&mut state.url);

        true
    }

    fn loading(&self, params: &Value, started: bool) {
        let mut state = self.state.lock().unwrap();
        if params["frameId"].as_str() != state.main_frame.as_deref() {
            return;
        }
        state.loading = started;
        drop(state);

        if !started {
            self.loads.send_modify(|count| *count += 1);
        }
    }

    fn log(&self, method: &str, params: &Value) {
        let mut state = self.state.lock().unwrap();
        match method {
            "Runtime.consoleAPICalled" => push(&mut state.console, console_line(params)),
            "Runtime.exceptionThrown" => push(&mut state.console, exception_line(&params["exceptionDetails"])),
            "Log.entryAdded" => push(&mut state.console, log_line(&params["entry"])),
            "Network.requestWillBeSent" => {
                let request = Request {
                    id: params["requestId"].as_str().unwrap_or_default().into(),
                    method: params["request"]["method"].as_str().unwrap_or("GET").into(),
                    url: params["request"]["url"].as_str().unwrap_or_default().into(),
                    kind: params["type"].as_str().unwrap_or("Other").into(),
                    status: None,
                    failed: None,
                };
                push(&mut state.requests, request);
            }
            "Network.responseReceived" => {
                if let Some(request) = find(&mut state.requests, params) {
                    request.status = params["response"]["status"].as_u64();
                }
            }
            "Network.loadingFailed" => {
                if let Some(request) = find(&mut state.requests, params) {
                    request.failed = params["errorText"].as_str().map(str::to_string);
                }
            }
            _ => {}
        }
    }

    /// The isolated world of the current document, made and given the helpers on first use.
    async fn world(&self) -> Result<i64, String> {
        let (existing, frame) = {
            let state = self.state.lock().unwrap();
            (state.world, state.main_frame.clone())
        };
        if let Some(world) = existing {
            return Ok(world);
        }

        let frame = frame.ok_or("the page has not loaded a document yet")?;
        let created = self
            .call(
                "Page.createIsolatedWorld",
                json!({ "frameId": frame, "worldName": WORLD }),
            )
            .await
            .map_err(|error| error.to_string())?;
        let world = created["executionContextId"]
            .as_i64()
            .ok_or("the browser made no isolated world")?;
        self.evaluate_in(world, PAGE_JS).await?;
        self.state.lock().unwrap().world = Some(world);

        Ok(world)
    }

    /// Runs `expression` among the helpers, out of the page's sight; the value comes back as JSON.
    pub(crate) async fn helper(&self, expression: &str) -> Result<Value, String> {
        let world = self.world().await?;
        match self.evaluate_in(world, expression).await {
            Err(error) if error.contains("context") => {
                // The document changed under the world since it was made.
                self.state.lock().unwrap().world = None;
                let world = self.world().await?;
                self.evaluate_in(world, expression).await
            }
            result => result,
        }
    }

    async fn evaluate_in(&self, world: i64, expression: &str) -> Result<Value, String> {
        let params = json!({
            "expression": expression,
            "contextId": world,
            "returnByValue": true,
            "awaitPromise": true,
        });
        let result = self
            .call("Runtime.evaluate", params)
            .await
            .map_err(|error| error.to_string())?;

        evaluated(result)
    }

    /// Runs `expression` as the page's own script would, for debugging what it holds.
    pub(crate) async fn evaluate(&self, expression: &str) -> Result<Value, String> {
        let params = json!({
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": true,
            "replMode": true,
            "userGesture": true,
        });
        let result = self
            .call("Runtime.evaluate", params)
            .await
            .map_err(|error| error.to_string())?;

        evaluated(result)
    }

    pub(crate) async fn mouse(&self, kind: &str, x: f64, y: f64, count: u32) -> Result<(), CdpError> {
        let button = if kind == "mouseMoved" { "none" } else { "left" };
        let params = json!({ "type": kind, "x": x, "y": y, "button": button, "clickCount": count });
        self.call("Input.dispatchMouseEvent", params).await?;

        Ok(())
    }

    pub(crate) async fn click(&self, x: f64, y: f64, double: bool) -> Result<(), CdpError> {
        self.mouse("mouseMoved", x, y, 0).await?;
        let presses = if double { 2 } else { 1 };
        for count in 1..=presses {
            self.mouse("mousePressed", x, y, count).await?;
            self.mouse("mouseReleased", x, y, count).await?;
        }

        Ok(())
    }

    pub(crate) async fn wheel(&self, x: f64, y: f64, delta_y: f64) -> Result<(), CdpError> {
        let params = json!({ "type": "mouseWheel", "x": x, "y": y, "deltaX": 0, "deltaY": delta_y });
        self.call("Input.dispatchMouseEvent", params).await?;

        Ok(())
    }

    pub(crate) async fn press(&self, key: &Key, modifiers: u8) -> Result<(), CdpError> {
        for event in key.events(modifiers) {
            self.call("Input.dispatchKeyEvent", event).await?;
        }

        Ok(())
    }

    pub(crate) async fn insert_text(&self, text: &str) -> Result<(), CdpError> {
        self.call("Input.insertText", json!({ "text": text })).await?;

        Ok(())
    }

    /// Loads `url` and waits for it, at most 15 s; a page that cannot load is an error naming why.
    pub(crate) async fn navigate(&self, url: &str) -> Result<(), String> {
        let before = *self.loads.borrow();
        let result = self
            .call("Page.navigate", json!({ "url": url }))
            .await
            .map_err(|error| error.to_string())?;
        if let Some(error) = result["errorText"].as_str().filter(|error| !error.is_empty()) {
            return Err(format!("{url} did not load: {error}"));
        }

        // A link to an anchor on the same page loads nothing.
        if result.get("loaderId").is_some() {
            self.wait_loaded(before, LOAD_WAIT).await;
        }

        Ok(())
    }

    /// Goes `delta` entries back (negative) or forward in the tab's history.
    pub(crate) async fn history(&self, delta: i64) -> Result<(), String> {
        let history = self
            .call("Page.getNavigationHistory", json!({}))
            .await
            .map_err(|error| error.to_string())?;
        let index = history["currentIndex"].as_i64().unwrap_or(0) + delta;
        let entry = usize::try_from(index)
            .ok()
            .and_then(|index| history["entries"].get(index))
            .ok_or("there is no page that way in this tab's history")?;

        let before = *self.loads.borrow();
        self.call("Page.navigateToHistoryEntry", json!({ "entryId": entry["id"] }))
            .await
            .map_err(|error| error.to_string())?;
        self.wait_loaded(before, LOAD_WAIT).await;

        Ok(())
    }

    pub(crate) async fn reload(&self) -> Result<(), String> {
        let before = *self.loads.borrow();
        self.call("Page.reload", json!({}))
            .await
            .map_err(|error| error.to_string())?;
        self.wait_loaded(before, LOAD_WAIT).await;

        Ok(())
    }

    /// Waits until a load newer than `before` finishes, `limit` passes, or a dialog stops the page.
    async fn wait_loaded(&self, before: u64, limit: Duration) {
        let mut loads = self.loads.subscribe();
        let deadline = tokio::time::Instant::now() + limit;

        while *loads.borrow_and_update() <= before && self.dialog().is_none() {
            let waited = tokio::time::timeout_at(deadline, loads.changed()).await;
            if !matches!(waited, Ok(Ok(()))) {
                return;
            }
        }
    }

    /// Lets a navigation an action started finish, then says where the page is.
    pub(crate) async fn settle(&self, url_before: &str) -> Settled {
        let before = *self.loads.borrow();
        tokio::time::sleep(SETTLE).await;
        if self.state.lock().unwrap().loading {
            self.wait_loaded(before, LOAD_WAIT).await;
        }

        let state = self.state.lock().unwrap();
        Settled {
            url: state.url.clone(),
            title: state.title.clone(),
            navigated: state.url != url_before,
            dialog: state.dialog.clone(),
        }
    }

    /// A JPEG of what the page shows now, with its size.
    pub(crate) async fn screenshot(&self) -> Result<(Vec<u8>, u32, u32), String> {
        use base64::Engine as _;

        let shot = self
            .call("Page.captureScreenshot", json!({ "format": "jpeg", "quality": 80 }))
            .await
            .map_err(|error| error.to_string())?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(shot["data"].as_str().unwrap_or_default())
            .map_err(|error| error.to_string())?;
        let (width, height) = image::ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format()
            .map_err(|error| error.to_string())?
            .into_dimensions()
            .map_err(|error| error.to_string())?;

        let metrics = self
            .call("Page.getLayoutMetrics", json!({}))
            .await
            .map_err(|error| error.to_string())?;
        let css_width = metrics["cssVisualViewport"]["clientWidth"]
            .as_f64()
            .unwrap_or(f64::from(width));
        self.state.lock().unwrap().shot_scale = Some(f64::from(width) / css_width.max(1.0));

        Ok((bytes, width, height))
    }

    /// A picture for the pane, with the page's size in CSS pixels; unlike `screenshot` it leaves click scaling alone.
    pub(crate) async fn picture(&self) -> Result<(Vec<u8>, (f64, f64)), String> {
        use base64::Engine as _;

        let shot = self
            .call("Page.captureScreenshot", json!({ "format": "jpeg", "quality": 70 }))
            .await
            .map_err(|error| error.to_string())?;
        let jpeg = base64::engine::general_purpose::STANDARD
            .decode(shot["data"].as_str().unwrap_or_default())
            .map_err(|error| error.to_string())?;
        let metrics = self
            .call("Page.getLayoutMetrics", json!({}))
            .await
            .map_err(|error| error.to_string())?;
        let viewport = &metrics["cssVisualViewport"];
        let size = (
            viewport["clientWidth"].as_f64().unwrap_or_default(),
            viewport["clientHeight"].as_f64().unwrap_or_default(),
        );

        Ok((jpeg, size))
    }

    /// A point in the last screenshot's pixels, in the page's CSS pixels.
    pub(crate) fn to_css(&self, x: f64, y: f64) -> (f64, f64) {
        let scale = self.state.lock().unwrap().shot_scale.unwrap_or(1.0);

        (x / scale, y / scale)
    }

    /// Console lines or requests, newest last, those `matches` accepts; at most `limit`.
    pub(crate) fn logs(&self, network: bool, matches: &dyn Fn(&str) -> bool, limit: usize) -> Vec<String> {
        let state = self.state.lock().unwrap();
        let lines: Vec<String> = if network {
            state.requests.iter().map(request_line).collect()
        } else {
            state.console.iter().cloned().collect()
        };
        let kept: Vec<String> = lines.into_iter().filter(|line| matches(line)).collect();
        let skip = kept.len().saturating_sub(limit);

        kept.into_iter().skip(skip).collect()
    }

    pub(crate) async fn response_body(&self, request_id: &str) -> Result<String, String> {
        let body = self
            .call("Network.getResponseBody", json!({ "requestId": request_id }))
            .await
            .map_err(|error| error.to_string())?;
        if body["base64Encoded"].as_bool() == Some(true) {
            let size = body["body"].as_str().map_or(0, |data| data.len() * 3 / 4);
            return Ok(format!("(binary, about {size} bytes)"));
        }

        Ok(body["body"].as_str().unwrap_or_default().to_string())
    }

    pub(crate) async fn answer_dialog(&self, accept: bool, text: Option<&str>) -> Result<(), String> {
        let mut params = json!({ "accept": accept });
        if let Some(text) = text {
            params["promptText"] = json!(text);
        }
        self.call("Page.handleJavaScriptDialog", params)
            .await
            .map_err(|error| error.to_string())?;
        self.state.lock().unwrap().dialog = None;

        Ok(())
    }
}

fn push<T>(list: &mut VecDeque<T>, item: T) {
    if list.len() == KEPT {
        list.pop_front();
    }
    list.push_back(item);
}

fn find<'a>(requests: &'a mut VecDeque<Request>, params: &Value) -> Option<&'a mut Request> {
    let id = params["requestId"].as_str()?;
    requests.iter_mut().rev().find(|request| request.id == id)
}

/// The value of an evaluation, or its exception as the error.
fn evaluated(result: Value) -> Result<Value, String> {
    if let Some(details) = result.get("exceptionDetails") {
        let thrown = details["exception"]["description"]
            .as_str()
            .or_else(|| details["text"].as_str())
            .unwrap_or("the script threw");
        let first_line = thrown.lines().next().unwrap_or(thrown);
        return Err(first_line.trim_start_matches("Error: ").to_string());
    }

    Ok(result["result"]["value"].clone())
}

fn remote_text(value: &Value) -> String {
    match &value["value"] {
        Value::String(text) => text.clone(),
        Value::Null => value["description"]
            .as_str()
            .or_else(|| value["type"].as_str())
            .unwrap_or_default()
            .to_string(),
        other => other.to_string(),
    }
}

fn console_line(params: &Value) -> String {
    let level = params["type"].as_str().unwrap_or("log");
    let text: Vec<String> = params["args"]
        .as_array()
        .into_iter()
        .flatten()
        .map(remote_text)
        .collect();
    let frame = &params["stackTrace"]["callFrames"][0];
    let place = frame["url"]
        .as_str()
        .filter(|url| !url.is_empty())
        .map(|url| format!(" ({url}:{})", frame["lineNumber"].as_u64().unwrap_or(0) + 1))
        .unwrap_or_default();

    format!("[{level}] {}{place}", text.join(" "))
}

fn exception_line(details: &Value) -> String {
    let text = details["exception"]["description"]
        .as_str()
        .or_else(|| details["text"].as_str())
        .unwrap_or("Uncaught exception");
    let first = text.lines().next().unwrap_or(text);
    let place = details["url"]
        .as_str()
        .map(|url| format!(" ({url}:{})", details["lineNumber"].as_u64().unwrap_or(0) + 1))
        .unwrap_or_default();

    format!("[error] {first}{place}")
}

fn log_line(entry: &Value) -> String {
    let level = entry["level"].as_str().unwrap_or("info");
    let text = entry["text"].as_str().unwrap_or_default();
    let place = entry["url"].as_str().map(|url| format!(" ({url})")).unwrap_or_default();

    format!("[{level}] {text}{place}")
}

fn request_line(request: &Request) -> String {
    let outcome = match (&request.failed, request.status) {
        (Some(error), _) => format!("failed: {error}"),
        (None, Some(status)) => status.to_string(),
        (None, None) => "pending".into(),
    };

    format!(
        "{} {} {} {} (id {})",
        request.method, outcome, request.url, request.kind, request.id
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_and_requests_read_as_one_line_each() {
        let console = json!({
            "type": "error",
            "args": [{ "type": "string", "value": "boom" }, { "type": "number", "value": 3 }],
            "stackTrace": { "callFrames": [{ "url": "http://x/app.js", "lineNumber": 9 }] }
        });
        assert_eq!(console_line(&console), "[error] boom 3 (http://x/app.js:10)");

        let request = Request {
            id: "7.1".into(),
            method: "POST".into(),
            url: "http://x/api".into(),
            kind: "Fetch".into(),
            status: Some(500),
            failed: None,
        };
        assert_eq!(request_line(&request), "POST 500 http://x/api Fetch (id 7.1)");
    }

    #[test]
    fn an_exception_becomes_the_error_without_its_stack() {
        let thrown =
            json!({ "exceptionDetails": { "exception": { "description": "Error: ref 4 is gone\n    at x" } } });

        assert_eq!(evaluated(thrown), Err("ref 4 is gone".to_string()));
        assert_eq!(evaluated(json!({ "result": { "value": 5 } })), Ok(json!(5)));
    }

    #[test]
    fn logs_keep_the_newest_matching_lines() {
        let mut console = VecDeque::new();
        for number in 0..(KEPT + 5) {
            push(&mut console, number);
        }

        assert_eq!(console.len(), KEPT);
        assert_eq!(console.front(), Some(&5));
    }
}
