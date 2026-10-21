//! IE mode as Edge runs it where a group policy gives it a site list: one Edge, started with no debugging port, so
//! the list sends each address to IE mode or to Chromium and shares the session cookies it names. Chromium tabs
//! are driven through Drift's extension, IE mode tabs through IEDriverServer attached to the same Edge, and the
//! pane shows the active tab whichever engine has it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};

use super::chromium::Chromium;
use super::chromium::cdp::{Connection, Event};
use super::chromium::relay::{self, Relayed};
use super::driver::{Common, Driver, Language, Settled, Shot, Size, Spec, Tab, Target, watch};
use super::ie::Ie;
use super::input::Key;
use super::site_list::{Route, SiteList};
use super::view::ViewInput;
use super::{Kind, PageRecord, with_scheme};
use crate::Engine;
use crate::platform::process::Tree;
use crate::platform::window;

/// How long Edge may take to download the site list a new profile lacks.
const SITE_LIST_WAIT: Duration = Duration::from_secs(15);
/// How often Edge's windows are checked for one that came onto a screen.
const SWEEP_EVERY: Duration = Duration::from_millis(250);
/// How long a page may take to open in the other engine.
const HANDOFF_WAIT: Duration = Duration::from_secs(30);
/// A tab gone from one engine and a tab new in the other within this long are the same tab, moved.
const MOVE_WINDOW: Duration = Duration::from_secs(10);
/// How long an action's page is looked for in the other engine once it left its own.
const MOVE_WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Chromium,
    Ie,
}

const SIDES: [Side; 2] = [Side::Chromium, Side::Ie];

impl Side {
    fn index(self) -> usize {
        match self {
            Self::Chromium => 0,
            Self::Ie => 1,
        }
    }
}

pub(crate) struct Mixed {
    common: Common,
    me: Weak<Mixed>,
    /// What each engine's own driver is started from.
    spec: Spec,
    pid: u32,
    tree: Tree,
    profile: PathBuf,
    /// The extension, for what the Chromium driver never asks: loading into IE mode, fronting an IE mode tab.
    relay: Arc<Connection>,
    chromium: Arc<Chromium>,
    ie: Mutex<Option<Arc<Ie>>>,
    attaching: tokio::sync::Mutex<()>,
    /// Why IE mode could not attach, for the action waiting on it.
    attach_error: Mutex<Option<String>>,
    tracked: Mutex<Tracked>,
    view: Mutex<Option<View>>,
}

#[derive(Default)]
struct Tracked {
    active: Option<String>,
    /// Each engine's active tab when last looked at, so a change in either is followed.
    seen: [Option<String>; 2],
    /// Each engine's tabs when last looked at.
    ids: [Vec<String>; 2],
    /// Tabs that left an engine lately, for a tab new in the other to take the place of.
    left: Vec<(String, Side, Instant)>,
    /// The tab each moved tab became.
    moved: HashMap<String, String>,
}

/// The engine whose frames the pane gets, while it watches.
struct View {
    side: Side,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for View {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// What an engine's own driver starts from: the browser's spec, as that engine, with no profile of its own to tidy.
fn part_spec(spec: &Spec, kind: Kind) -> Spec {
    Spec {
        kind,
        profile: None,
        ..spec.clone()
    }
}

/// The extension's own notices go to this driver; everything else is the Chromium driver's.
async fn split(
    mut events: mpsc::UnboundedReceiver<Event>,
    chromium: mpsc::UnboundedSender<Event>,
    notices: mpsc::UnboundedSender<Event>,
) {
    while let Some(event) = events.recv().await {
        let to = if event.method.starts_with("Drift.") {
            &notices
        } else {
            &chromium
        };
        let _ = to.send(event);
    }
}

/// A new profile has no site list until Edge downloads it, and an address loaded before then opens in Chromium.
async fn wait_for_site_list(profile: &std::path::Path) {
    let file = SiteList::file(profile);
    let deadline = tokio::time::Instant::now() + SITE_LIST_WAIT;
    while !file.is_file() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

impl Mixed {
    pub(crate) async fn start(engine: Weak<Engine>, spec: Spec) -> Result<Arc<Self>, String> {
        let profile = spec
            .profile
            .clone()
            .ok_or("IE mode with a site list needs a profile folder")?;
        let Relayed {
            child,
            tree,
            pid,
            cdp,
            events,
        } = relay::launch(&spec.program, &profile, &spec.integrated_auth).await?;

        let (to_chromium, chromium_events) = mpsc::unbounded_channel();
        let (to_mixed, notices) = mpsc::unbounded_channel();
        tokio::spawn(split(events, to_chromium, to_mixed));
        let part = part_spec(&spec, Kind::Edge);
        let chromium = match Chromium::drive(Weak::new(), part, cdp.clone(), chromium_events, None).await {
            Ok(chromium) => chromium,
            Err(error) => {
                tree.kill();
                return Err(error);
            }
        };
        wait_for_site_list(&profile).await;

        let mixed = Arc::new_cyclic(|me| Self {
            common: Common::new(engine, spec.clone()),
            me: me.clone(),
            spec,
            pid,
            tree,
            profile,
            relay: cdp,
            chromium: chromium.clone(),
            ie: Mutex::default(),
            attaching: tokio::sync::Mutex::new(()),
            attach_error: Mutex::default(),
            tracked: Mutex::default(),
            view: Mutex::default(),
        });
        mixed.join(chromium.as_ref());
        tokio::spawn(mixed.clone().run(child, notices));

        Ok(mixed)
    }

    fn as_driver(&self) -> Option<Arc<dyn Driver>> {
        self.me.upgrade().map(|me| me as Arc<dyn Driver>)
    }

    /// Makes an engine's driver report its changes here.
    fn join(&self, part: &dyn Driver) {
        let me = self.me.clone();
        let _ = part.common().on_change.set(Box::new(move || {
            if let Some(mixed) = me.upgrade() {
                mixed.part_changed();
            }
        }));
    }

    fn part_changed(&self) {
        self.follow();
        self.restream();
        if let Some(driver) = self.as_driver() {
            driver.changed();
        }
    }

    fn ie(&self) -> Option<Arc<Ie>> {
        self.ie.lock().unwrap().clone()
    }

    fn part(&self, side: Side) -> Option<Arc<dyn Driver>> {
        match side {
            Side::Chromium => Some(self.chromium.clone() as Arc<dyn Driver>),
            Side::Ie => self.ie().map(|ie| ie as Arc<dyn Driver>),
        }
    }

    fn ids(&self, side: Side) -> Vec<String> {
        self.part(side)
            .map(|part| part.tabs().iter().map(|tab| tab.id()).collect())
            .unwrap_or_default()
    }

    fn side_of(&self, id: &str) -> Option<Side> {
        SIDES
            .into_iter()
            .find(|side| self.ids(*side).iter().any(|known| known == id))
    }

    fn find(&self, id: &str) -> Option<Arc<dyn Tab>> {
        SIDES
            .into_iter()
            .filter_map(|side| self.part(side))
            .flat_map(|part| part.tabs())
            .find(|tab| tab.id() == id)
    }

    /// The tab a tab became, after any moves between the engines.
    fn resolve(&self, id: &str) -> String {
        let tracked = self.tracked.lock().unwrap();
        let mut id = id.to_string();
        for _ in 0..8 {
            match tracked.moved.get(&id) {
                Some(next) => id = next.clone(),
                None => break,
            }
        }

        id
    }

    fn wrap(&self, id: String) -> Arc<dyn Tab> {
        Arc::new(MixedTab {
            mixed: self.me.clone(),
            id,
        })
    }

    /// The engine the site list sends an address to; anything it does not name opens in Chromium.
    fn route(&self, url: &str) -> Side {
        let routed = SiteList::read(&SiteList::file(&self.profile)).and_then(|list| list.route(url));

        if routed == Some(Route::Ie) {
            Side::Ie
        } else {
            Side::Chromium
        }
    }

    /// Notes the engines' tabs: a new active tab in either becomes the active one, and a tab that left one engine
    /// as a tab appeared in the other moved there.
    fn follow(&self) {
        let ids = SIDES.map(|side| self.ids(side));
        let actives = [self.chromium.active(), self.ie().and_then(|ie| ie.active())];
        let now = Instant::now();
        let mut tracked = self.tracked.lock().unwrap();

        for side in SIDES {
            note_moves(&mut tracked, side, &ids[side.index()], now);
        }
        tracked.left.retain(|(_, _, at)| now.duration_since(*at) < MOVE_WINDOW);
        for side in SIDES {
            let active = &actives[side.index()];
            if *active != tracked.seen[side.index()] {
                if active.is_some() {
                    tracked.active.clone_from(active);
                }
                tracked.seen[side.index()].clone_from(active);
            }
        }

        let open = |id: &String| ids.iter().flatten().any(|known| known == id);
        if !tracked.active.as_ref().is_some_and(open) {
            tracked.active = actives
                .iter()
                .rev()
                .flatten()
                .find(|id| open(id))
                .or_else(|| ids.iter().flatten().last())
                .cloned();
        }
        tracked.ids = ids;
    }

    fn choose(&self, id: &str) {
        self.tracked.lock().unwrap().active = Some(id.to_string());
        self.part_changed();
    }

    /// Points the pane's frames at the engine of the active tab while it watches, and stops them when no one does.
    fn restream(&self) {
        let wanted = if self.common.watched() {
            self.active().and_then(|id| self.side_of(&id))
        } else {
            None
        };
        let mut view = self.view.lock().unwrap();
        if view.as_ref().map(|view| view.side) == wanted {
            return;
        }

        *view = wanted.and_then(|side| self.show(side));
    }

    /// Watches an engine's driver and passes its frames and cursor on as this browser's.
    fn show(&self, side: Side) -> Option<View> {
        let part = self.part(side)?;
        let mixed = self.me.clone();

        let task = tokio::spawn(async move {
            let mut viewer = watch(part.clone());
            let mut cursor = part.common().cursor.subscribe();
            loop {
                tokio::select! {
                    frame = viewer.frames.recv() => match frame {
                        Ok(frame) => {
                            let Some(mixed) = mixed.upgrade() else { break };
                            let _ = mixed.common.frames.send(frame);
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => break,
                    },
                    changed = cursor.changed() => {
                        let Some(mixed) = mixed.upgrade().filter(|_| changed.is_ok()) else { break };
                        let shown = cursor.borrow_and_update().clone();
                        mixed.common.set_cursor(&shown);
                    }
                }
            }
        });

        Some(View { side, task })
    }

    async fn run(self: Arc<Self>, mut child: tokio::process::Child, mut notices: mpsc::UnboundedReceiver<Event>) {
        let mut sweep = tokio::time::interval(SWEEP_EVERY);
        loop {
            tokio::select! {
                _ = child.wait() => break,
                () = self.common.stopping.cancelled() => break,
                notice = notices.recv() => match notice {
                    Some(notice) => self.notice(&notice),
                    None => break,
                },
                _ = sweep.tick() => self.sweep(),
            }
        }

        self.common.mark_ended();
        *self.view.lock().unwrap() = None;
        if let Some(ie) = self.ie.lock().unwrap().take() {
            ie.kill();
        }
        self.chromium.kill();
        self.tree.kill();
        let _ = child.wait().await;
        self.tree.stop().await;

        if let (Some(engine), Some(driver)) = (self.common.engine.upgrade(), self.as_driver()) {
            engine.browser_ended(&driver).await;
        }
    }

    /// Keeps Edge's windows out of sight, and lets IE mode go once its last tab closed, to attach again later.
    fn sweep(&self) {
        window::hide_windows(self.pid);

        let ended = self
            .ie()
            .is_some_and(|ie| ie.common().is_ended() || ie.tabs().is_empty());
        if !ended {
            return;
        }
        if let Some(ie) = self.ie.lock().unwrap().take() {
            ie.kill();
        }
        self.part_changed();
    }

    fn notice(&self, notice: &Event) {
        if notice.method == "Drift.ieTab" && self.ie().is_none() {
            self.attach_soon();
        }
    }

    /// A tab is in IE mode and nothing drives IE mode yet: IEDriverServer attaches to it.
    fn attach_soon(&self) {
        let Some(mixed) = self.me.upgrade() else {
            return;
        };

        tokio::spawn(async move {
            let Ok(_attaching) = mixed.attaching.try_lock() else {
                return;
            };
            if mixed.ie().is_some() {
                return;
            }

            let part = part_spec(&mixed.spec, Kind::Ie);
            match Ie::start(Weak::new(), part, Some(mixed.pid)).await {
                Ok(ie) => {
                    mixed.join(ie.as_ref());
                    *mixed.ie.lock().unwrap() = Some(ie);
                    mixed.part_changed();
                }
                Err(error) => *mixed.attach_error.lock().unwrap() = Some(error),
            }
        });
    }

    /// Loads an address the site list sends to IE mode in a Chromium tab while nothing drives IE mode, and waits
    /// for IEDriverServer to attach to the tab it became.
    async fn load_in_ie_mode(&self, target: &str, url: &str) -> Result<String, String> {
        self.attach_error.lock().unwrap().take();
        self.relay
            .call("Drift.load", json!({ "targetId": target, "url": url }), None)
            .await
            .map_err(|error| error.to_string())?;

        let deadline = tokio::time::Instant::now() + HANDOFF_WAIT;
        loop {
            if let Some(id) = self.ie().and_then(|ie| ie.active()) {
                return Ok(id);
            }
            if let Some(error) = self.attach_error.lock().unwrap().take() {
                return Err(format!("IE mode could not attach to Edge: {error}"));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!("{url} did not open in IE mode within 30 s"));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Loads an address of the other engine: Edge would move the tab there, so its page opens there and the old
    /// tab closes.
    async fn hand_off(&self, old: &str, from: Side, url: &str) -> Result<(), String> {
        let ie = self.ie();
        let new = match (from, &ie) {
            (Side::Chromium, None) => self.load_in_ie_mode(old, url).await?,
            (Side::Chromium, Some(ie)) => ie.open_tab(url).await?.id(),
            (Side::Ie, _) => self.chromium.open_tab(url).await?.id(),
        };
        self.tracked.lock().unwrap().moved.insert(old.to_string(), new.clone());
        if new != old
            && let Some(part) = self
                .part(from)
                .filter(|part| part.tabs().iter().any(|tab| tab.id() == old))
        {
            let _ = part.close_tab(old).await;
        }
        self.choose(&new);

        Ok(())
    }

    /// The tab an action's tab became in the other engine, when a click there loaded one of its addresses.
    async fn moved_from(&self, old: &str) -> Option<Arc<dyn Tab>> {
        let deadline = tokio::time::Instant::now() + MOVE_WAIT;
        while tokio::time::Instant::now() < deadline {
            self.follow();
            let now = self.resolve(old);
            if now != old
                && let Some(tab) = self.find(&now)
            {
                return Some(tab);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        None
    }

    /// Brings an IE mode tab to the front of its Edge window, where it is painted and so can be pictured.
    async fn front_ie_tab(&self, id: &str) {
        let Some(tab) = self.find(id) else {
            return;
        };
        let _ = self
            .relay
            .call("Drift.showIeTab", json!({ "url": tab.url() }), None)
            .await;
    }

    async fn navigate(&self, url: &str) -> Result<(), String> {
        let active = self.active().ok_or("the browser has no page open")?;

        self.wrap(active).go(url).await
    }
}

/// Tabs that left this engine are noted, and a tab new to it takes the place of one that just left the other.
fn note_moves(tracked: &mut Tracked, side: Side, ids: &[String], now: Instant) {
    let before = std::mem::take(&mut tracked.ids[side.index()]);
    for id in before.iter().filter(|id| !ids.contains(id)) {
        tracked.left.push((id.clone(), side, now));
    }

    for id in ids.iter().filter(|id| !before.contains(id)) {
        let moved = tracked
            .left
            .iter()
            .position(|(_, from, at)| *from != side && now.duration_since(*at) < MOVE_WINDOW);
        let Some(index) = moved else { continue };
        let (old, _, _) = tracked.left.remove(index);
        if tracked.active.as_deref() == Some(old.as_str()) {
            tracked.active = Some(id.clone());
        }
        tracked.moved.insert(old, id.clone());
    }
    tracked.ids[side.index()] = before;
}

#[async_trait]
impl Driver for Mixed {
    fn common(&self) -> &Common {
        &self.common
    }

    fn tabs(&self) -> Vec<Arc<dyn Tab>> {
        SIDES
            .into_iter()
            .flat_map(|side| self.ids(side))
            .map(|id| self.wrap(id))
            .collect()
    }

    fn active(&self) -> Option<String> {
        self.follow();

        self.tracked.lock().unwrap().active.clone()
    }

    async fn open_tab(&self, url: &str) -> Result<Arc<dyn Tab>, String> {
        let url = with_scheme(url);
        let opened = match (self.route(&url), self.ie()) {
            (Side::Ie, Some(ie)) => ie.open_tab(&url).await?,
            (Side::Ie, None) => {
                let blank = self.chromium.open_tab("about:blank").await?;
                let tab = self.wrap(blank.id());
                tab.go(&url).await?;
                return Ok(tab);
            }
            (Side::Chromium, _) => self.chromium.open_tab(&url).await?,
        };
        self.choose(&opened.id());

        Ok(self.wrap(opened.id()))
    }

    async fn select(&self, id: &str) -> Result<(), String> {
        let id = self.resolve(id);
        let side = self
            .side_of(&id)
            .ok_or_else(|| format!("no page `{id}`; `pages` lists them"))?;
        let part = self.part(side).ok_or("the browser has closed")?;

        self.tracked.lock().unwrap().active = Some(id.clone());
        part.select(&id).await?;
        if side == Side::Ie {
            self.front_ie_tab(&id).await;
        }
        self.part_changed();

        Ok(())
    }

    async fn close_tab(&self, id: &str) -> Result<(), String> {
        let id = self.resolve(id);
        let side = self.side_of(&id).ok_or_else(|| format!("no page `{id}`"))?;
        let part = self.part(side).ok_or("the browser has closed")?;

        part.close_tab(&id).await
    }

    async fn refresh(&self) {
        for part in SIDES.into_iter().filter_map(|side| self.part(side)) {
            part.refresh().await;
        }
    }

    fn viewers_changed(&self) {
        let before = self.view.lock().unwrap().as_ref().map(|view| view.side);
        self.restream();
        let after = self.view.lock().unwrap().as_ref().map(|view| view.side);

        // A pane joining one already watching gets its own first picture.
        if before.is_some()
            && before == after
            && let Some(part) = after.and_then(|side| self.part(side))
        {
            part.viewers_changed();
        }
    }

    /// Closes Edge's windows, which ends it, and ends it anyway if it does not close.
    fn stop(&self) {
        if let Some(ie) = self.ie() {
            ie.stop();
        }
        self.chromium.stop();
        let Some(mixed) = self.me.upgrade() else {
            return;
        };

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            mixed.common.stopping.cancel();
        });
    }

    fn kill(&self) {
        if let Some(ie) = self.ie() {
            ie.kill();
        }
        self.chromium.kill();
        self.tree.kill();
        self.common.stopping.cancel();
    }

    async fn input(&self, input: ViewInput) -> Result<(), String> {
        self.common.touch();
        match input {
            ViewInput::Select { page } => self.select(&page).await,
            ViewInput::Close { page } => self.close_tab(&page).await,
            ViewInput::NewPage {} => self.open_tab("about:blank").await.map(|_| ()),
            ViewInput::Navigate { url } => self.navigate(&url).await,
            ViewInput::Fit { width, height } => self.chromium.input(ViewInput::Fit { width, height }).await,
            other => {
                let active = self.active().ok_or("the browser has no page open")?;
                let side = self.side_of(&active).ok_or("the active page has closed")?;
                let part = self.part(side).ok_or("the browser has closed")?;
                part.input(other).await
            }
        }
    }

    fn opening_note(&self) -> Option<String> {
        Some(
            "IE mode runs Edge off-screen as this machine's IE mode site list sets it up: listed pages open in IE \
             mode, pages the list sends to Edge open as Chromium tabs beside them, and the session cookies it names \
             are shared. IE mode tabs cannot show console or network logs, and `eval` there can run VBScript with \
             `language: vbscript`."
                .into(),
        )
    }

    fn mixes_engines(&self) -> bool {
        true
    }
}

/// A tab of either engine, followed to the other when it moves there.
struct MixedTab {
    mixed: Weak<Mixed>,
    id: String,
}

impl MixedTab {
    fn mixed(&self) -> Result<Arc<Mixed>, String> {
        self.mixed.upgrade().ok_or_else(|| "the browser has closed".into())
    }

    fn tab(&self) -> Result<Arc<dyn Tab>, String> {
        let mixed = self.mixed()?;
        let id = mixed.resolve(&self.id);

        mixed.find(&id).ok_or_else(|| format!("page `{id}` has closed"))
    }
}

fn gone(url_before: &str) -> Settled {
    Settled {
        url: String::new(),
        title: String::new(),
        navigated: !url_before.is_empty(),
        dialog: None,
    }
}

#[async_trait]
impl Tab for MixedTab {
    fn id(&self) -> String {
        self.mixed
            .upgrade()
            .map_or_else(|| self.id.clone(), |mixed| mixed.resolve(&self.id))
    }

    fn record(&self, active: bool) -> PageRecord {
        match self.tab() {
            Ok(tab) => tab.record(active),
            Err(_) => PageRecord {
                id: self.id(),
                url: String::new(),
                title: String::new(),
                active,
                popup: false,
                dialog: None,
            },
        }
    }

    async fn snapshot(&self, interactive: bool) -> Result<String, String> {
        self.tab()?.snapshot(interactive).await
    }

    async fn text(&self) -> Result<String, String> {
        self.tab()?.text().await
    }

    async fn click(&self, target: Target, double: bool) -> Result<String, String> {
        self.tab()?.click(target, double).await
    }

    async fn hover(&self, target: Target) -> Result<String, String> {
        self.tab()?.hover(target).await
    }

    async fn fill(&self, reference: u64, text: &str, checked: Option<bool>) -> Result<String, String> {
        self.tab()?.fill(reference, text, checked).await
    }

    async fn scroll(&self, reference: Option<u64>, dy: f64) -> Result<String, String> {
        self.tab()?.scroll(reference, dy).await
    }

    async fn press(&self, key: &Key, modifiers: u8) -> Result<(), String> {
        self.tab()?.press(key, modifiers).await
    }

    async fn type_text(&self, text: &str) -> Result<(), String> {
        self.tab()?.type_text(text).await
    }

    /// An address the site list sends to the other engine moves the tab there, as Edge does.
    async fn go(&self, url: &str) -> Result<(), String> {
        let mixed = self.mixed()?;
        let tab = self.tab()?;
        if matches!(url, "back" | "forward" | "reload") {
            return tab.go(url).await;
        }

        let url = with_scheme(url);
        let from = mixed.side_of(&tab.id()).ok_or("the page has closed")?;
        if mixed.route(&url) == from {
            return tab.go(&url).await;
        }

        mixed.hand_off(&tab.id(), from, &url).await
    }

    /// A click that loaded an address of the other engine moved the tab there, so that is where it settles.
    async fn settle(&self, url_before: &str) -> Settled {
        let (Ok(mixed), Ok(tab)) = (self.mixed(), self.tab()) else {
            return gone(url_before);
        };
        let settled = tab.settle(url_before).await;
        if mixed.find(&tab.id()).is_some() {
            return settled;
        }

        match mixed.moved_from(&tab.id()).await {
            Some(moved) => moved.settle(url_before).await,
            None => settled,
        }
    }

    async fn screenshot(&self) -> Result<Shot, String> {
        self.tab()?.screenshot().await
    }

    async fn zoom(&self, region: [f64; 4]) -> Result<Shot, String> {
        self.tab()?.zoom(region).await
    }

    async fn evaluate(&self, script: &str, language: Language) -> Result<Value, String> {
        self.tab()?.evaluate(script, language).await
    }

    fn logs(&self, network: bool, matches: &dyn Fn(&str) -> bool, limit: usize) -> Result<Vec<String>, String> {
        self.tab()?.logs(network, matches, limit)
    }

    async fn response_body(&self, request: &str) -> Result<String, String> {
        self.tab()?.response_body(request).await
    }

    async fn answer_dialog(&self, accept: bool, text: Option<&str>) -> Result<(), String> {
        self.tab()?.answer_dialog(accept, text).await
    }

    async fn viewport(&self, size: Option<Size>) -> Result<(), String> {
        self.tab()?.viewport(size).await
    }
}
