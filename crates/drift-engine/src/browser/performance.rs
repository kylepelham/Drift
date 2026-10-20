//! What a Chrome trace says about a page's speed: the load's milestones and largest paint, layout shifts, where the
//! main thread's time went and its long tasks, the network, and the slowest interactions.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Read as _;

use serde::Deserialize;
use serde_json::Value;

/// A task this long keeps the page from answering input; the time past it counts as blocking.
const LONG_TASK: f64 = 50_000.0;
const LISTED: usize = 3;

#[derive(Deserialize)]
struct Trace {
    #[serde(rename = "traceEvents")]
    events: Vec<Event>,
}

#[derive(Deserialize)]
struct Event {
    #[serde(default)]
    name: String,
    #[serde(default)]
    ph: String,
    /// Microseconds, on the same clock as network timings' `requestTime` seconds.
    #[serde(default)]
    ts: f64,
    dur: Option<f64>,
    #[serde(default)]
    pid: i64,
    #[serde(default)]
    tid: i64,
    #[serde(default)]
    args: Value,
}

impl Event {
    fn data(&self) -> &Value {
        &self.args["data"]
    }

    fn frame(&self) -> &str {
        self.args["frame"]
            .as_str()
            .or_else(|| self.data()["frame"].as_str())
            .unwrap_or_default()
    }

    fn text(&self, field: &str) -> &str {
        self.data()[field].as_str().unwrap_or_default()
    }

    fn number(&self, field: &str) -> f64 {
        self.data()[field].as_f64().unwrap_or_default()
    }
}

/// The page the trace is about: its main frame, the renderer thread that ran it, and the load it recorded, if any.
struct Scope {
    frame: String,
    pid: i64,
    tid: Option<i64>,
    start: f64,
    navigation: Option<Navigation>,
}

struct Navigation {
    id: String,
    url: String,
    ts: f64,
}

/// A summary of a gzipped trace for the model, in plain lines.
pub(crate) fn summarize(gzipped: &[u8]) -> Result<String, String> {
    let mut json = Vec::new();
    flate2::read::GzDecoder::new(gzipped)
        .read_to_end(&mut json)
        .map_err(|error| format!("the trace could not be unpacked: {error}"))?;
    let trace: Trace =
        serde_json::from_slice(&json).map_err(|error| format!("the trace could not be read: {error}"))?;
    drop(json);

    let scope = scope(&trace.events).ok_or("the trace holds no page")?;
    let mut text = String::new();
    let load = scope
        .navigation
        .as_ref()
        .map(|navigation| load(&trace.events, &scope, navigation));
    match &load {
        Some(load) => write_load(&mut text, load),
        None => text.push_str(
            "Recording (no page load in it; times are from its start)
",
        ),
    }
    write_shifts(&mut text, &shifts(&trace.events, &scope));
    let blocking_from = load.as_ref().and_then(|load| load.fcp).unwrap_or(0.0);
    if let Some(thread) = main_thread(&trace.events, &scope, blocking_from) {
        write_thread(&mut text, &thread, load.is_some());
    }
    write_network(&mut text, &network(&trace.events, &scope), &scope);
    write_interactions(&mut text, &interactions(&trace.events, &scope));

    Ok(text.trim_end().to_string())
}

fn scope(events: &[Event]) -> Option<Scope> {
    let started = events.iter().find(|event| event.name == "TracingStartedInBrowser")?;
    let main = started.data()["frames"]
        .as_array()?
        .iter()
        .find(|frame| frame["isOutermostMainFrame"].as_bool() == Some(true) || frame.get("parent").is_none())?;
    let frame = main["frame"].as_str()?.to_string();

    let navigation = events
        .iter()
        .filter(|event| event.name == "navigationStart" && event.frame() == frame)
        .filter(|event| is_document(event.text("documentLoaderURL")))
        .max_by(|a, b| a.ts.total_cmp(&b.ts));
    let committed = events
        .iter()
        .filter(|event| event.name == "FrameCommittedInBrowser" && event.text("frame") == frame)
        .filter_map(|event| event.data()["processId"].as_i64())
        .next_back();
    let pid = navigation
        .map(|event| event.pid)
        .or(committed)
        .or_else(|| main["processId"].as_i64())?;
    let tid = events
        .iter()
        .find(|event| event.ph == "M" && event.pid == pid && event.args["name"] == "CrRendererMain")
        .map(|event| event.tid);

    Some(Scope {
        frame,
        pid,
        tid,
        start: navigation.map_or(started.ts, |event| event.ts),
        navigation: navigation.map(|event| Navigation {
            id: event.text("navigationId").to_string(),
            url: event.text("documentLoaderURL").to_string(),
            ts: event.ts,
        }),
    })
}

fn is_document(url: &str) -> bool {
    url.starts_with("http:") || url.starts_with("https:") || url.starts_with("file:")
}

/// The load's milestones, each in microseconds from its start.
struct Load {
    ttfb: Option<f64>,
    fcp: Option<f64>,
    lcp: Option<Lcp>,
    dom_ready: Option<f64>,
    loaded: Option<f64>,
}

struct Lcp {
    at: f64,
    element: String,
    image: Option<Image>,
}

/// When the largest paint's image was found and loaded, in milliseconds from the load's start, as Chrome gives them.
struct Image {
    url: String,
    load_start: f64,
    load_end: f64,
}

fn load(events: &[Event], scope: &Scope, navigation: &Navigation) -> Load {
    let since = |event: &Event| event.ts - navigation.ts;
    let ttfb = events
        .iter()
        .find(|event| event.name == "ResourceReceiveResponse" && event.text("requestId") == navigation.id)
        .map(|event| {
            let timing = &event.data()["timing"];
            match (timing["requestTime"].as_f64(), timing["receiveHeadersEnd"].as_f64()) {
                (Some(sent), Some(headers)) => sent * 1e6 + headers * 1e3 - navigation.ts,
                _ => since(event),
            }
        });
    let fcp = events
        .iter()
        .find(|event| event.name == "firstContentfulPaint" && event.text("navigationId") == navigation.id)
        .map(since);
    let marked = |name: &str| {
        events
            .iter()
            .filter(|event| event.name == name && event.text("frame") == scope.frame && event.ts >= navigation.ts)
            .map(since)
            .min_by(f64::total_cmp)
    };

    Load {
        ttfb,
        fcp,
        lcp: lcp(events, navigation),
        dom_ready: marked("MarkDOMContent"),
        loaded: marked("MarkLoad"),
    }
}

/// The last largest-paint candidate of the load, unless the page invalidated it after.
fn lcp(events: &[Event], navigation: &Navigation) -> Option<Lcp> {
    let last = events
        .iter()
        .filter(|event| {
            event.name.starts_with("largestContentfulPaint::") && event.text("navigationId") == navigation.id
        })
        .filter(|event| event.name.ends_with("::Candidate") || event.name.ends_with("::Invalidate"))
        .max_by(|a, b| {
            a.ts.total_cmp(&b.ts)
                .then(a.number("candidateIndex").total_cmp(&b.number("candidateIndex")))
        })?;
    if last.name.ends_with("::Invalidate") {
        return None;
    }

    let node = last.data()["nodeId"].as_i64();
    let url = events
        .iter()
        .filter(|event| event.name == "LargestImagePaint::Candidate" && event.data()["DOMNodeId"].as_i64() == node)
        .map(|event| event.text("imageUrl").to_string())
        .next_back();
    let image = url.filter(|_| last.text("type") == "image").map(|url| Image {
        url,
        load_start: last.number("imageLoadStart"),
        load_end: last.number("imageLoadEnd"),
    });

    Some(Lcp {
        at: last.ts - navigation.ts,
        element: last.text("nodeName").to_string(),
        image,
    })
}

/// Cumulative layout shift as Chrome scores it: the worst window of shifts less than 1 s apart, at most 5 s long.
struct Shifts {
    score: f64,
    count: usize,
    worst: Option<(f64, f64)>,
}

fn shifts(events: &[Event], scope: &Scope) -> Shifts {
    let mut found: Vec<(f64, f64)> = events
        .iter()
        .filter(|event| event.name == "LayoutShift" && event.frame() == scope.frame && event.ts >= scope.start)
        .filter(|event| event.data()["had_recent_input"].as_bool() != Some(true))
        .map(|event| (event.ts, event.number("weighted_score_delta")))
        .collect();
    found.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut score: f64 = 0.0;
    let mut window = 0.0;
    let mut window_start = f64::MIN;
    let mut previous = f64::MIN;
    for &(ts, delta) in &found {
        if ts - previous > 1e6 || ts - window_start > 5e6 {
            window = 0.0;
            window_start = ts;
        }
        window += delta;
        previous = ts;
        score = score.max(window);
    }
    let worst = found
        .iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|&(ts, delta)| (ts - scope.start, delta));

    Shifts {
        score,
        count: found.len(),
        worst,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Work {
    Scripting,
    StyleAndLayout,
    Painting,
    Parsing,
    Other,
}

impl Work {
    fn of(name: &str) -> Option<Self> {
        match name {
            "EvaluateScript"
            | "FunctionCall"
            | "TimerFire"
            | "EventDispatch"
            | "FireAnimationFrame"
            | "FireIdleCallback"
            | "RunMicrotasks"
            | "v8.compile"
            | "v8.compileModule"
            | "v8.evaluateModule"
            | "v8.run"
            | "MajorGC"
            | "MinorGC"
            | "XHRReadyStateChange"
            | "XHRLoad" => Some(Self::Scripting),
            "UpdateLayoutTree" | "Layout" | "RecalculateStyles" | "HitTest" => Some(Self::StyleAndLayout),
            "Paint" | "PrePaint" | "Layerize" | "UpdateLayer" | "Commit" | "PaintImage" | "Decode Image" => {
                Some(Self::Painting)
            }
            "ParseHTML" | "ParseAuthorStyleSheet" => Some(Self::Parsing),
            other if other.starts_with("V8.GC") => Some(Self::Scripting),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Scripting => "scripting",
            Self::StyleAndLayout => "style and layout",
            Self::Painting => "painting",
            Self::Parsing => "parsing HTML and CSS",
            Self::Other => "other",
        }
    }
}

/// The script an event runs, when it says.
fn source(event: &Event) -> Option<&str> {
    let url = match event.name.as_str() {
        "EvaluateScript" | "FunctionCall" | "v8.compile" | "v8.compileModule" | "v8.evaluateModule" => {
            event.text("url")
        }
        "v8.run" => event.args["fileName"].as_str().unwrap_or_default(),
        _ => "",
    };

    (!url.is_empty()).then_some(url)
}

/// What started a long task, in words.
fn trigger(event: &Event) -> Option<String> {
    match event.name.as_str() {
        "EventDispatch" => Some(format!("a {} event", event.text("type"))),
        "TimerFire" => Some("a timer".into()),
        "FireAnimationFrame" => Some("an animation frame".into()),
        "ParseHTML" => Some("parsing HTML".into()),
        "EvaluateScript" => source(event).map(|url| format!("running {}", file_name(url))),
        "Layout" => Some("layout".into()),
        "UpdateLayoutTree" => Some("style recalculation".into()),
        _ => None,
    }
}

struct MainThread {
    busy: f64,
    work: Vec<(Work, f64)>,
    scripts: Vec<(String, f64)>,
    long_tasks: Vec<LongTask>,
    long_count: usize,
    blocking: f64,
}

struct LongTask {
    at: f64,
    duration: f64,
    trigger: Option<String>,
    script: Option<String>,
}

/// An event open on the main thread while later ones nest inside it.
struct Open {
    end: f64,
    left: f64,
    work: Option<Work>,
    url: Option<String>,
}

/// The top-level task being walked: when it began, what started it, and its script time by source.
struct Task {
    at: f64,
    duration: f64,
    /// The longest event inside it that says what it was, with that event's duration.
    trigger: Option<(f64, String)>,
    scripts: HashMap<String, f64>,
}

#[derive(Default)]
struct Totals {
    work: HashMap<Work, f64>,
    scripts: HashMap<String, f64>,
    tasks: Vec<LongTask>,
    busy: f64,
    blocking: f64,
}

fn main_thread(events: &[Event], scope: &Scope, blocking_from: f64) -> Option<MainThread> {
    let tid = scope.tid?;
    let mut ran: Vec<&Event> = events
        .iter()
        .filter(|event| event.ph == "X" && event.pid == scope.pid && event.tid == tid && event.ts >= scope.start)
        .collect();
    ran.sort_by(|a, b| {
        a.ts.total_cmp(&b.ts)
            .then(b.dur.unwrap_or(0.0).total_cmp(&a.dur.unwrap_or(0.0)))
    });

    let mut totals = Totals::default();
    let mut open: Vec<Open> = Vec::new();
    let mut task: Option<Task> = None;
    for event in ran {
        let duration = event.dur.unwrap_or(0.0);
        while open.last().is_some_and(|top| top.end <= event.ts) {
            close(&mut open, &mut task, &mut totals, blocking_from);
        }
        if open.is_empty() {
            task = Some(Task {
                at: event.ts - scope.start,
                duration,
                trigger: None,
                scripts: HashMap::new(),
            });
        }
        let parent = open.last_mut();
        let (work, url) = match parent {
            Some(parent) => {
                parent.left -= duration;
                (
                    Work::of(&event.name).or(parent.work),
                    source(event).map(str::to_string).or(parent.url.clone()),
                )
            }
            None => (Work::of(&event.name), source(event).map(str::to_string)),
        };
        if let Some(task) = task
            .as_mut()
            .filter(|task| task.trigger.as_ref().is_none_or(|(longest, _)| duration > *longest))
        {
            task.trigger = trigger(event).map(|label| (duration, label)).or(task.trigger.take());
        }
        open.push(Open {
            end: event.ts + duration,
            left: duration,
            work,
            url,
        });
    }
    while !open.is_empty() {
        close(&mut open, &mut task, &mut totals, blocking_from);
    }

    Some(thread_summary(totals))
}

/// Ends the innermost open event: its own time goes to its kind of work, and a top-level task is judged as a whole;
/// blocking time counts from `blocking_from`, microseconds into the recording.
fn close(open: &mut Vec<Open>, task: &mut Option<Task>, totals: &mut Totals, blocking_from: f64) {
    let Some(ended) = open.pop() else { return };
    let own = ended.left.max(0.0);
    let work = ended.work.unwrap_or(Work::Other);
    *totals.work.entry(work).or_default() += own;
    if work == Work::Scripting
        && let Some(url) = ended.url
    {
        *totals.scripts.entry(url.clone()).or_default() += own;
        if let Some(task) = task.as_mut() {
            *task.scripts.entry(url).or_default() += own;
        }
    }
    if !open.is_empty() {
        return;
    }

    let Some(done) = task.take() else { return };
    totals.busy += done.duration;
    if done.duration < LONG_TASK {
        return;
    }
    if done.at >= blocking_from {
        totals.blocking += done.duration - LONG_TASK;
    }
    let script = done
        .scripts
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(url, _)| url);
    totals.tasks.push(LongTask {
        at: done.at,
        duration: done.duration,
        trigger: done.trigger.map(|(_, label)| label),
        script,
    });
}

fn thread_summary(totals: Totals) -> MainThread {
    let mut work: Vec<(Work, f64)> = totals.work.into_iter().filter(|(_, time)| *time >= 1_000.0).collect();
    work.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut scripts: Vec<(String, f64)> = totals
        .scripts
        .into_iter()
        .filter(|(_, time)| *time >= 1_000.0)
        .collect();
    scripts.sort_by(|a, b| b.1.total_cmp(&a.1));
    scripts.truncate(LISTED + 2);
    let long_count = totals.tasks.len();
    let mut long_tasks = totals.tasks;
    long_tasks.sort_by(|a, b| b.duration.total_cmp(&a.duration));
    long_tasks.truncate(LISTED);

    MainThread {
        busy: totals.busy,
        work,
        scripts,
        long_tasks,
        long_count,
        blocking: totals.blocking,
    }
}

struct Request {
    url: String,
    sent: f64,
    finished: Option<f64>,
    status: Option<u64>,
    failed: bool,
    bytes: f64,
    blocking: bool,
}

fn network(events: &[Event], scope: &Scope) -> Vec<Request> {
    let mut requests: Vec<Request> = Vec::new();
    let mut index: HashMap<&str, usize> = HashMap::new();
    for event in events.iter().filter(|event| event.ts >= scope.start) {
        let id = event.text("requestId");
        match event.name.as_str() {
            "ResourceSendRequest" if event.text("frame") == scope.frame && !event.text("url").starts_with("data:") => {
                index.insert(id, requests.len());
                requests.push(Request {
                    url: event.text("url").to_string(),
                    sent: event.ts,
                    finished: None,
                    status: None,
                    failed: false,
                    bytes: 0.0,
                    blocking: matches!(event.text("renderBlocking"), "blocking" | "in_body_parser_blocking"),
                });
            }
            "ResourceReceiveResponse" => {
                if let Some(request) = index.get(id).map(|&at| &mut requests[at]) {
                    request.status = event.data()["statusCode"].as_u64();
                }
            }
            "ResourceFinish" => {
                if let Some(request) = index.get(id).map(|&at| &mut requests[at]) {
                    let finish = event.number("finishTime");
                    request.finished = Some(if finish > 0.0 { finish * 1e6 } else { event.ts });
                    request.failed = event.data()["didFail"].as_bool() == Some(true);
                    request.bytes = event.number("encodedDataLength");
                }
            }
            _ => {}
        }
    }

    requests
}

struct Interaction {
    kind: String,
    at: f64,
    duration: f64,
    delay: f64,
    processing: f64,
}

/// The slowest interactions, one per user action: Chrome reports each of its events (pointerdown, click...).
fn interactions(events: &[Event], scope: &Scope) -> Vec<Interaction> {
    let mut by_id: HashMap<u64, Vec<&Event>> = HashMap::new();
    for event in events
        .iter()
        .filter(|event| event.name == "EventTiming" && event.ph == "b" && event.text("frame") == scope.frame)
        .filter(|event| event.ts >= scope.start)
    {
        if let Some(id) = event.data()["interactionId"].as_u64().filter(|id| *id > 0) {
            by_id.entry(id).or_default().push(event);
        }
    }

    let mut found: Vec<Interaction> = by_id
        .into_values()
        .filter_map(|events| interaction(&events, scope))
        .collect();
    found.sort_by(|a, b| b.duration.total_cmp(&a.duration));
    found.truncate(LISTED);

    found
}

/// One action's events read as one: its longest duration, broken down by the event whose handlers ran longest.
fn interaction(events: &[&Event], scope: &Scope) -> Option<Interaction> {
    let processing = |event: &&Event| event.number("processingEnd") - event.number("processingStart");
    let duration = events.iter().map(|event| event.number("duration")).fold(0.0, f64::max);
    let main = events.iter().max_by(|a, b| processing(a).total_cmp(&processing(b)))?;

    Some(Interaction {
        kind: main.text("type").to_string(),
        at: main.ts - scope.start,
        duration,
        delay: (main.number("processingStart") - main.number("timeStamp")).max(0.0),
        processing: processing(main).max(0.0),
    })
}

fn write_load(text: &mut String, load: &Load) {
    text.push_str("Load\n");
    if let Some(ttfb) = load.ttfb {
        let _ = writeln!(
            text,
            "- Time to first byte: {} ({})",
            ms(ttfb),
            rate(ttfb, 800e3, 1800e3)
        );
    }
    if let Some(fcp) = load.fcp {
        let _ = writeln!(
            text,
            "- First contentful paint: {} ({})",
            ms(fcp),
            rate(fcp, 1800e3, 3000e3)
        );
    }
    match &load.lcp {
        Some(lcp) => write_lcp(text, lcp, load.ttfb),
        None => text.push_str("- Largest contentful paint: none recorded\n"),
    }
    if let (Some(ready), Some(loaded)) = (load.dom_ready, load.loaded) {
        let _ = writeln!(text, "- DOMContentLoaded at {}, load at {}", ms(ready), ms(loaded));
    }
}

fn write_lcp(text: &mut String, lcp: &Lcp, ttfb: Option<f64>) {
    let _ = writeln!(
        text,
        "- Largest contentful paint: {} ({}), {}",
        ms(lcp.at),
        rate(lcp.at, 2500e3, 4000e3),
        lcp.element
    );
    let ttfb = ttfb.unwrap_or(0.0);
    let Some(image) = &lcp.image else {
        let _ = writeln!(
            text,
            "  - after the first byte, {} until it was painted",
            ms(lcp.at - ttfb)
        );
        return;
    };

    let start = image.load_start * 1e3;
    let end = image.load_end * 1e3;
    let _ = writeln!(text, "  - image {}", image.url);
    let _ = writeln!(
        text,
        "  - first byte {}, then {} before the image was requested, {} loading it, {} until it was painted",
        ms(ttfb),
        ms((start - ttfb).max(0.0)),
        ms((end - start).max(0.0)),
        ms((lcp.at - end).max(0.0))
    );
}

fn write_shifts(text: &mut String, shifts: &Shifts) {
    let Some((at, worst)) = shifts.worst else {
        text.push_str("- Layout shifts: none\n");
        return;
    };

    let _ = writeln!(
        text,
        "- Cumulative layout shift: {:.3} ({}), {}, the largest {worst:.3} at {}",
        shifts.score,
        rate(shifts.score, 0.1, 0.25),
        plural(shifts.count, "shift"),
        ms(at)
    );
}

fn write_thread(text: &mut String, thread: &MainThread, loaded: bool) {
    text.push_str("\nMain thread\n");
    let work: Vec<String> = thread
        .work
        .iter()
        .map(|(work, time)| format!("{} {}", work.label(), ms(*time)))
        .collect();
    let _ = writeln!(text, "- Busy {}: {}", ms(thread.busy), work.join(", "));
    let since = if loaded {
        "after the first paint"
    } else {
        "in the recording"
    };
    let _ = writeln!(
        text,
        "- {}; total blocking time {since}: {} ({})",
        plural(thread.long_count, "long task"),
        ms(thread.blocking),
        rate(thread.blocking, 200e3, 600e3)
    );
    for task in &thread.long_tasks {
        let mut line = format!("  - {} at {}", ms(task.duration), ms(task.at));
        if let Some(trigger) = &task.trigger {
            let _ = write!(line, ", {trigger}");
        }
        if let Some(script) = task.script.as_deref().filter(|script| {
            !task
                .trigger
                .as_deref()
                .is_some_and(|trigger| trigger.contains(file_name(script)))
        }) {
            let _ = write!(line, ", mostly in {}", file_name(script));
        }
        let _ = writeln!(text, "{line}");
    }
    if !thread.scripts.is_empty() {
        let scripts: Vec<String> = thread
            .scripts
            .iter()
            .map(|(url, time)| format!("{} {}", file_name(url), ms(*time)))
            .collect();
        let _ = writeln!(text, "- Script time by file: {}", scripts.join(", "));
    }
}

fn write_network(text: &mut String, requests: &[Request], scope: &Scope) {
    if requests.is_empty() {
        return;
    }
    let origin = scope
        .navigation
        .as_ref()
        .map(|navigation| origin(&navigation.url))
        .unwrap_or_default();
    let bytes: f64 = requests.iter().map(|request| request.bytes).sum();
    let failed: Vec<String> = requests
        .iter()
        .filter(|request| request.failed || request.status.is_some_and(|status| status >= 400))
        .map(|request| match request.status.filter(|status| *status >= 400) {
            Some(status) => format!("{} ({status})", short(&request.url, origin)),
            None => short(&request.url, origin),
        })
        .collect();

    text.push_str("\nNetwork\n");
    let _ = writeln!(
        text,
        "- {}, {} transferred",
        plural(requests.len(), "request"),
        size(bytes)
    );
    if !failed.is_empty() {
        let _ = writeln!(text, "- Failed: {}", failed.join(", "));
    }
    let timed = |request: &Request| request.finished.map(|end| end - request.sent);
    if scope.navigation.is_some() {
        let blocking: Vec<String> = requests
            .iter()
            .filter(|request| request.blocking)
            .map(|request| format!("{} {}", short(&request.url, origin), ms(timed(request).unwrap_or(0.0))))
            .collect();
        if !blocking.is_empty() {
            let _ = writeln!(text, "- Render-blocking: {}", blocking.join(", "));
        }
    }
    let mut slow: Vec<(&Request, f64)> = requests
        .iter()
        .filter_map(|request| timed(request).map(|time| (request, time)))
        .filter(|(_, time)| *time >= 500e3)
        .collect();
    slow.sort_by(|a, b| b.1.total_cmp(&a.1));
    write_list(
        text,
        "Slowest",
        slow.iter()
            .take(LISTED)
            .map(|(request, time)| format!("{} {}", short(&request.url, origin), ms(*time))),
    );
    let mut large: Vec<&Request> = requests.iter().filter(|request| request.bytes >= 20_000.0).collect();
    large.sort_by(|a, b| b.bytes.total_cmp(&a.bytes));
    write_list(
        text,
        "Largest",
        large
            .iter()
            .take(LISTED)
            .map(|request| format!("{} {}", short(&request.url, origin), size(request.bytes))),
    );
    write_hosts(text, requests, origin);
}

fn write_hosts(text: &mut String, requests: &[Request], origin: &str) {
    let mut hosts: HashMap<&str, (usize, f64)> = HashMap::new();
    for request in requests
        .iter()
        .filter(|request| !origin.is_empty() && !request.url.starts_with(origin))
    {
        let entry = hosts.entry(self::origin(&request.url)).or_default();
        entry.0 += 1;
        entry.1 += request.bytes;
    }
    let mut hosts: Vec<(&str, (usize, f64))> = hosts.into_iter().collect();
    hosts.sort_by(|a, b| b.1.1.total_cmp(&a.1.1));

    write_list(
        text,
        "Other origins",
        hosts
            .iter()
            .take(LISTED + 2)
            .map(|(host, (count, bytes))| format!("{host} {}, {}", plural(*count, "request"), size(*bytes))),
    );
}

fn write_list(text: &mut String, title: &str, items: impl Iterator<Item = String>) {
    let items: Vec<String> = items.collect();
    if !items.is_empty() {
        let _ = writeln!(text, "- {title}: {}", items.join(", "));
    }
}

fn write_interactions(text: &mut String, found: &[Interaction]) {
    if found.is_empty() {
        return;
    }

    text.push_str("\nInteractions\n");
    for interaction in found {
        let duration = interaction.duration * 1e3;
        let after = (interaction.duration - interaction.delay - interaction.processing).max(0.0) * 1e3;
        let _ = writeln!(
            text,
            "- {} at {}: {} until the next paint ({}); {} waiting for the main thread, {} in handlers, {} to paint",
            interaction.kind,
            ms(interaction.at),
            ms(duration),
            rate(duration, 200e3, 500e3),
            ms(interaction.delay * 1e3),
            ms(interaction.processing * 1e3),
            ms(after)
        );
    }
}

/// Google's thresholds for a Web Vital: good up to `good`, poor past `poor`.
fn rate(value: f64, good: f64, poor: f64) -> &'static str {
    if value <= good {
        "good"
    } else if value <= poor {
        "needs improvement"
    } else {
        "poor"
    }
}

/// Microseconds as people read them.
fn ms(us: f64) -> String {
    if us >= 1e6 {
        return format!("{:.1} s", us / 1e6);
    }

    format!("{:.0} ms", us / 1e3)
}

fn size(bytes: f64) -> String {
    if bytes >= 1e6 {
        return format!("{:.1} MB", bytes / 1e6);
    }

    format!("{:.0} KB", bytes / 1e3)
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        return format!("1 {noun}");
    }

    format!("{count} {noun}s")
}

/// `scheme://host:port` of an address.
fn origin(url: &str) -> &str {
    let after_scheme = url.find("://").map_or(0, |at| at + 3);
    let end = url[after_scheme..].find('/').map_or(url.len(), |at| at + after_scheme);

    &url[..end]
}

/// An address without the page's own origin or its query, kept short.
fn short(url: &str, origin: &str) -> String {
    let path = url.strip_prefix(origin).filter(|_| !origin.is_empty()).unwrap_or(url);
    let path = path.split('?').next().unwrap_or(path);
    if path.chars().count() <= 80 {
        return path.to_string();
    }

    let tail: String = path
        .chars()
        .rev()
        .take(77)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("...{tail}")
}

fn file_name(url: &str) -> &str {
    let path = url.split('?').next().unwrap_or(url);

    path.rsplit('/').find(|part| !part.is_empty()).unwrap_or(path)
}

#[cfg(test)]
mod tests;
