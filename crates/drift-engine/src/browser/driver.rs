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

/// What traces and audits answer on engines that are not Chrome or Edge.
pub(crate) const ONLY_CHROMIUM: &str =
    "performance traces and Lighthouse audits run only in Chrome and Edge; `open` with `engine` chrome or edge";

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

/// The most pixels an agent's screenshot has: a 1280x800 viewport's worth, Anthropic's suggested size for web
/// apps, so a page laid out to fill a big pane costs no more tokens to look at.
const SHOT_PIXELS: f64 = 1280.0 * 800.0;
/// The most a zoom enlarges the region it shows.
const ZOOM_MAX: f64 = 4.0;
const SHOT_QUALITY: u8 = 80;

/// How much a page picture this size shrinks to become a screenshot; 1 when it fits `SHOT_PIXELS`.
pub(crate) fn shot_factor(width: u32, height: u32) -> f64 {
    let pixels = f64::from(width) * f64::from(height);

    (SHOT_PIXELS / pixels.max(1.0)).sqrt().min(1.0)
}

impl Shot {
    /// The agent's screenshot of a page picture: the picture scaled down in its own shape to fit `SHOT_PIXELS`.
    pub(crate) fn fitted(jpeg: Vec<u8>) -> Result<Self, String> {
        let picture = decode(&jpeg)?;
        let (width, height) = (picture.width(), picture.height());
        let factor = shot_factor(width, height);
        if factor >= 1.0 {
            return Ok(Self { jpeg, width, height });
        }

        resized(&picture, scaled(width, factor), scaled(height, factor))
    }

    /// A region of a page picture, given as `[x0, y0, x1, y1]` in the pixels of the screenshot `fitted` makes of
    /// it, cut from the full picture and enlarged up to `ZOOM_MAX` times within `SHOT_PIXELS`.
    pub(crate) fn zoomed(jpeg: &[u8], region: [f64; 4]) -> Result<Self, String> {
        let picture = decode(jpeg)?;
        let (width, height) = (picture.width(), picture.height());
        let factor = shot_factor(width, height);
        let screenshot = (f64::from(width) * factor, f64::from(height) * factor);

        let [x0, y0, x1, y1] = region;
        if !region.iter().all(|value| value.is_finite()) || x1 <= x0 || y1 <= y0 {
            return Err("`region` is [x0, y0, x1, y1] with x1 above x0 and y1 above y0".into());
        }
        let left = x0.clamp(0.0, screenshot.0) / factor;
        let top = y0.clamp(0.0, screenshot.1) / factor;
        let right = x1.clamp(0.0, screenshot.0) / factor;
        let bottom = y1.clamp(0.0, screenshot.1) / factor;
        let (cut_width, cut_height) = ((right - left).round(), (bottom - top).round());
        if cut_width < 1.0 || cut_height < 1.0 {
            let (shot_width, shot_height) = (screenshot.0.round(), screenshot.1.round());
            return Err(format!("`region` is outside the {shot_width}x{shot_height} screenshot"));
        }

        let cut = picture.crop_imm(left as u32, top as u32, cut_width as u32, cut_height as u32);
        let enlarge = (SHOT_PIXELS / (cut_width * cut_height)).sqrt().min(ZOOM_MAX);
        resized(&cut, scaled(cut.width(), enlarge), scaled(cut.height(), enlarge))
    }
}

fn decode(jpeg: &[u8]) -> Result<image::DynamicImage, String> {
    image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg).map_err(|error| error.to_string())
}

fn scaled(side: u32, factor: f64) -> u32 {
    ((f64::from(side) * factor).round() as u32).max(1)
}

fn resized(picture: &image::DynamicImage, width: u32, height: u32) -> Result<Shot, String> {
    let resized = picture.resize_exact(width, height, image::imageops::FilterType::Triangle);
    let mut jpeg = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, SHOT_QUALITY);
    resized
        .to_rgb8()
        .write_with_encoder(encoder)
        .map_err(|error| error.to_string())?;

    Ok(Shot { jpeg, width, height })
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

    /// A region of what `screenshot` shows now, `[x0, y0, x1, y1]` in its pixels, enlarged for a closer look.
    async fn zoom(&self, region: [f64; 4]) -> Result<Shot, String>;

    async fn evaluate(&self, script: &str, language: Language) -> Result<Value, String>;

    /// Console lines or requests, newest last; an error when the engine cannot see them.
    fn logs(&self, network: bool, matches: &dyn Fn(&str) -> bool, limit: usize) -> Result<Vec<String>, String>;

    async fn response_body(&self, request: &str) -> Result<String, String>;

    async fn answer_dialog(&self, accept: bool, text: Option<&str>) -> Result<(), String>;

    async fn viewport(&self, size: Option<Size>) -> Result<(), String>;

    /// Starts recording a performance trace of the page; `throttle` slows its CPU and network as a mid-range phone's.
    async fn start_trace(&self, _throttle: bool) -> Result<(), String> {
        Err(ONLY_CHROMIUM.into())
    }

    /// Ends the recording: the trace as gzipped JSON, as DevTools' Performance panel opens it.
    async fn stop_trace(&self) -> Result<Vec<u8>, String> {
        Err(ONLY_CHROMIUM.into())
    }
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

    /// The DevTools Protocol port, for tools such as Lighthouse that connect to the browser themselves.
    fn devtools_port(&self) -> Option<u16> {
        None
    }

    /// The user agent a hidden Chrome or Edge gives its pages in place of its headless one.
    fn desktop_agent(&self) -> Option<String> {
        None
    }

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
    /// The CSS cursor the page shows under the pane's pointer.
    pub(crate) cursor: tokio::sync::watch::Sender<String>,
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
            cursor: tokio::sync::watch::channel("default".into()).0,
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

    /// Tells watching panes the cursor changed; the same cursor again says nothing.
    pub(crate) fn set_cursor(&self, cursor: &str) {
        self.cursor.send_if_modified(|current| {
            if current == cursor {
                return false;
            }
            cursor.clone_into(current);
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A white 1280x2048 page picture with a red box at 600,1500 sized 200x100.
    fn tall_page() -> Vec<u8> {
        let mut page = image::RgbImage::from_pixel(1280, 2048, image::Rgb([255, 255, 255]));
        for x in 600..800 {
            for y in 1500..1600 {
                page.put_pixel(x, y, image::Rgb([220, 0, 0]));
            }
        }
        let mut jpeg = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95);
        page.write_with_encoder(encoder).unwrap();
        jpeg
    }

    #[test]
    fn a_tall_page_is_seen_at_a_desktop_screenshots_cost_in_its_own_shape() {
        let shot = Shot::fitted(tall_page()).unwrap();
        assert_eq!((shot.width, shot.height), (800, 1280));

        let mut desktop = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut desktop, 80);
        image::RgbImage::new(1280, 800).write_with_encoder(encoder).unwrap();
        let kept = Shot::fitted(desktop.clone()).unwrap();
        assert_eq!(
            (kept.width, kept.height, kept.jpeg),
            (1280, 800, desktop),
            "a desktop page is sent as taken"
        );
    }

    #[test]
    fn a_zoom_cuts_the_full_picture_where_the_screenshot_shows_it_and_enlarges_it() {
        // The box is at 375,937.5 to 500,1000 in the 800x1280 screenshot.
        let zoomed = Shot::zoomed(&tall_page(), [375.0, 937.5, 500.0, 1000.0]).unwrap();
        assert_eq!(
            (zoomed.width, zoomed.height),
            (800, 400),
            "200x100 of the page, 4x at most"
        );

        let picture = decode(&zoomed.jpeg).unwrap().to_rgb8();
        let middle = picture.get_pixel(400, 200);
        assert!(middle[0] > 180 && middle[1] < 60, "the box fills it: {middle:?}");
    }

    #[test]
    fn a_zoom_outside_the_screenshot_or_backwards_is_refused() {
        let outside = Shot::zoomed(&tall_page(), [900.0, 0.0, 1000.0, 10.0]).err().unwrap();
        assert!(outside.contains("800x1280"), "{outside}");
        assert!(Shot::zoomed(&tall_page(), [10.0, 10.0, 5.0, 20.0]).is_err());
        assert!(Shot::zoomed(&tall_page(), [0.0, 0.0, f64::NAN, 20.0]).is_err());
    }
}
