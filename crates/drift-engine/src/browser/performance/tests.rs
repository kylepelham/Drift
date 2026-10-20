use std::io::Write as _;

use serde_json::{Value, json};

use super::*;

const FRAME: &str = "MAIN";
const NAV: &str = "NAV1";
/// The load starts 10 s into the browser's clock.
const T0: f64 = 10_000_000.0;

fn at(ms: f64) -> f64 {
    T0 + ms * 1e3
}

fn instant(name: &str, ms: f64, args: Value) -> Value {
    json!({ "name": name, "ph": "I", "ts": at(ms), "pid": 2, "tid": 7, "args": args })
}

fn ran(name: &str, ms: f64, took: f64, data: Value) -> Value {
    json!({ "name": name, "ph": "X", "ts": at(ms), "dur": took * 1e3, "pid": 2, "tid": 7, "args": { "data": data } })
}

/// A request sent and finished at `(sent, done)` ms.
fn request(id: &str, url: &str, (sent, done): (f64, f64), bytes: u64, blocking: Option<&str>) -> Vec<Value> {
    let mut send = json!({ "requestId": id, "url": url, "frame": FRAME });
    if let Some(blocking) = blocking {
        send["renderBlocking"] = json!(blocking);
    }
    vec![
        instant("ResourceSendRequest", sent, json!({ "data": send })),
        instant(
            "ResourceReceiveResponse",
            done - 1.0,
            json!({ "data": { "requestId": id, "statusCode": 200, "timing": { "requestTime": at(sent) / 1e6, "receiveHeadersEnd": done - sent - 1.0 } } }),
        ),
        instant(
            "ResourceFinish",
            done,
            json!({ "data": { "requestId": id, "encodedDataLength": bytes, "finishTime": at(done) / 1e6 } }),
        ),
    ]
}

/// A shop's front page: a blocking stylesheet, a slow hero image as the largest paint, a long script, a banner that
/// shifts the layout, and a slow click.
fn shop() -> Vec<u8> {
    let mut events = milestones();
    events.extend(main_thread());
    events.extend(request(NAV, "http://shop.test/", (0.5, 3.0), 411, None));
    events.extend(request(
        "r2",
        "http://shop.test/style.css",
        (8.0, 262.0),
        189,
        Some("blocking"),
    ));
    events.extend(request(
        "r3",
        "http://shop.test/hero.jpg?w=900",
        (8.5, 615.0),
        84_000,
        None,
    ));
    events.extend(request(
        "r4",
        "https://fonts.example.com/serif.woff2",
        (9.0, 40.0),
        30_000,
        None,
    ));

    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gzip.write_all(json!({ "traceEvents": events }).to_string().as_bytes())
        .unwrap();
    gzip.finish().unwrap()
}

/// The navigation, its paints and its layout shifts.
fn milestones() -> Vec<Value> {
    vec![
        json!({ "name": "TracingStartedInBrowser", "ph": "I", "ts": at(-5.0), "pid": 1, "tid": 1, "args": { "data": { "frames": [{ "frame": FRAME, "isOutermostMainFrame": true, "processId": 2, "url": "http://shop.test/" }] } } }),
        json!({ "name": "thread_name", "ph": "M", "ts": 0, "pid": 2, "tid": 7, "args": { "name": "CrRendererMain" } }),
        json!({ "name": "navigationStart", "ph": "R", "ts": at(0.0), "pid": 2, "tid": 7, "args": { "frame": FRAME, "data": { "documentLoaderURL": "http://shop.test/", "navigationId": NAV } } }),
        json!({ "name": "navigationStart", "ph": "R", "ts": at(600.0), "pid": 2, "tid": 7, "args": { "frame": "SVG", "data": { "documentLoaderURL": "", "navigationId": "OTHER" } } }),
        instant(
            "firstContentfulPaint",
            270.0,
            json!({ "frame": FRAME, "data": { "navigationId": NAV } }),
        ),
        instant(
            "largestContentfulPaint::Candidate",
            270.0,
            json!({ "frame": FRAME, "data": { "navigationId": NAV, "candidateIndex": 1, "nodeId": 12, "nodeName": "H1", "type": "text" } }),
        ),
        instant(
            "LargestImagePaint::Candidate",
            640.0,
            json!({ "frame": FRAME, "data": { "DOMNodeId": 17, "imageUrl": "http://shop.test/hero.jpg" } }),
        ),
        instant(
            "largestContentfulPaint::Candidate",
            640.0,
            json!({ "frame": FRAME, "data": { "navigationId": NAV, "candidateIndex": 2, "nodeId": 17, "nodeName": "IMG class='hero'", "type": "image", "imageLoadStart": 11.0, "imageLoadEnd": 615.0 } }),
        ),
        instant("MarkDOMContent", 450.0, json!({ "data": { "frame": FRAME } })),
        instant("MarkLoad", 620.0, json!({ "data": { "frame": FRAME } })),
        instant(
            "LayoutShift",
            750.0,
            json!({ "frame": FRAME, "data": { "weighted_score_delta": 0.08, "had_recent_input": false } }),
        ),
        instant(
            "LayoutShift",
            900.0,
            json!({ "frame": FRAME, "data": { "weighted_score_delta": 0.04, "had_recent_input": false } }),
        ),
        instant(
            "LayoutShift",
            2700.0,
            json!({ "frame": FRAME, "data": { "weighted_score_delta": 0.5, "had_recent_input": true } }),
        ),
    ]
}

/// The long script, the slow click and its event timings.
fn main_thread() -> Vec<Value> {
    vec![
        ran("RunTask", 263.0, 183.0, json!({})),
        ran(
            "EvaluateScript",
            263.1,
            182.8,
            json!({ "url": "http://shop.test/app.js" }),
        ),
        ran("Layout", 450.0, 12.0, json!({})),
        ran("RunTask", 2637.0, 221.0, json!({})),
        ran("EventDispatch", 2637.05, 0.04, json!({ "type": "mouseup" })),
        ran("EventDispatch", 2637.1, 220.5, json!({ "type": "click" })),
        ran(
            "FunctionCall",
            2637.2,
            220.0,
            json!({ "url": "http://shop.test/app.js" }),
        ),
        json!({ "name": "EventTiming", "ph": "b", "ts": at(2636.0), "pid": 2, "tid": 7, "args": { "data": { "frame": FRAME, "interactionId": 9, "type": "pointerdown", "duration": 227.8, "timeStamp": 2635.4, "processingStart": 2636.4, "processingEnd": 2636.5 } } }),
        json!({ "name": "EventTiming", "ph": "b", "ts": at(2637.0), "pid": 2, "tid": 7, "args": { "data": { "frame": FRAME, "interactionId": 9, "type": "click", "duration": 225.9, "timeStamp": 2637.3, "processingStart": 2637.6, "processingEnd": 2857.8 } } }),
        json!({ "name": "EventTiming", "ph": "b", "ts": at(2637.0), "pid": 2, "tid": 7, "args": { "data": { "frame": FRAME, "interactionId": 0, "type": "pointerover", "duration": 6.0, "timeStamp": 2637.0, "processingStart": 2637.0, "processingEnd": 2637.1 } } }),
    ]
}

#[test]
fn a_load_reads_as_its_milestones_and_what_held_up_the_largest_paint() {
    let summary = summarize(&shop()).unwrap();

    assert!(summary.contains("First contentful paint: 270 ms (good)"), "{summary}");
    assert!(
        summary.contains("Largest contentful paint: 640 ms (good), IMG class='hero'"),
        "{summary}"
    );
    assert!(summary.contains("image http://shop.test/hero.jpg"), "{summary}");
    assert!(
        summary.contains("9 ms before the image was requested, 604 ms loading it, 25 ms until it was painted"),
        "{summary}"
    );
    assert!(
        summary.contains("DOMContentLoaded at 450 ms, load at 620 ms"),
        "{summary}"
    );
}

#[test]
fn shifts_after_input_do_not_count_and_close_ones_add_up() {
    let summary = summarize(&shop()).unwrap();

    assert!(
        summary.contains("Cumulative layout shift: 0.120 (needs improvement), 2 shifts, the largest 0.080 at 750 ms"),
        "{summary}"
    );
}

#[test]
fn the_main_thread_names_its_long_tasks_and_the_scripts_that_ran() {
    let summary = summarize(&shop()).unwrap();

    assert!(
        summary.contains("2 long tasks; total blocking time after the first paint: 171 ms (good)"),
        "{summary}"
    );
    assert!(
        summary.contains("221 ms at 2.6 s, a click event, mostly in app.js"),
        "{summary}"
    );
    assert!(
        summary.contains(
            "183 ms at 263 ms, running app.js
"
        ),
        "{summary}"
    );
    assert!(summary.contains("Script time by file: app.js 403 ms"), "{summary}");
    assert!(summary.contains("style and layout 12 ms"), "{summary}");
}

#[test]
fn the_network_lists_blocking_large_and_other_origin_requests() {
    let summary = summarize(&shop()).unwrap();

    assert!(summary.contains("4 requests, 115 KB transferred"), "{summary}");
    assert!(summary.contains("Render-blocking: /style.css 254 ms"), "{summary}");
    assert!(summary.contains("Slowest: /hero.jpg 606 ms"), "{summary}");
    assert!(
        summary.contains("Largest: /hero.jpg 84 KB, https://fonts.example.com/serif.woff2 30 KB"),
        "{summary}"
    );
    assert!(
        summary.contains("Other origins: https://fonts.example.com 1 request, 30 KB"),
        "{summary}"
    );
}

#[test]
fn an_interaction_is_one_line_split_where_its_time_went() {
    let summary = summarize(&shop()).unwrap();

    assert!(summary.contains("click at 2.6 s: 228 ms until the next paint (needs improvement); 0 ms waiting for the main thread, 220 ms in handlers, 7 ms to paint"), "{summary}");
    assert!(!summary.contains("pointerover"), "{summary}");
}

#[test]
fn a_trace_that_is_not_gzipped_json_is_an_error() {
    assert!(summarize(b"not a trace").is_err());
}
