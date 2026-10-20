use std::fmt::Write as _;

use serde_json::json;

use super::*;
use crate::tool::tests::Sandbox;

async fn call(sandbox: &Sandbox, input: Value) -> Result<Output, ToolError> {
    Browser.run(&sandbox.ctx, input).await
}

#[test]
fn looking_is_a_read_and_acting_is_not() {
    for action in ["snapshot", "screenshot", "zoom", "logs", "pages", "wait"] {
        assert!(!Browser.call_mutates(&json!({ "action": action })), "{action}");
    }
    for action in ["open", "click", "fill", "eval", "stop"] {
        assert!(Browser.call_mutates(&json!({ "action": action })), "{action}");
    }

    let looking = json!({ "action": "batch", "actions": [{ "action": "snapshot" }, { "action": "screenshot" }] });
    let acting = json!({ "action": "batch", "actions": [{ "action": "snapshot" }, { "action": "click", "ref": 1 }] });
    assert!(!Browser.call_mutates(&looking));
    assert!(Browser.call_mutates(&acting));
}

#[test]
fn a_public_site_asks_and_a_dev_server_does_not() {
    let sandbox = Sandbox::new("browser-asks");

    let public = Browser.asks(&sandbox.ctx, &json!({ "action": "open", "url": "example.com" }));
    assert_eq!(public.len(), 1);
    assert_eq!(
        (public[0].kind.as_str(), public[0].pattern.as_str()),
        ("browser", "example.com")
    );
    assert!(!public[0].default_allow);

    let local = Browser.asks(&sandbox.ctx, &json!({ "action": "open", "url": "localhost:5173" }));
    assert!(local[0].default_allow);
    assert!(Browser.asks(&sandbox.ctx, &json!({ "action": "pages" })).is_empty());
}

#[tokio::test]
async fn acting_without_a_browser_says_to_open_one() {
    let sandbox = Sandbox::new("browser-none");

    let refused = call(&sandbox, json!({ "action": "click", "ref": 1 }))
        .await
        .unwrap_err();
    assert!(refused.0.contains("`open` a page first"), "{}", refused.0);

    let pages = call(&sandbox, json!({ "action": "pages" })).await.unwrap();
    assert!(pages.output.contains("no browser open"));
}

/// Drives the real installed browser through a form; skipped where neither Chrome nor Edge is installed.
#[tokio::test(flavor = "multi_thread")]
async fn a_real_browser_fills_a_form_and_reports_what_changed() {
    if crate::browser::installed().is_none() {
        return;
    }
    let sandbox = Sandbox::new("browser-live");
    let page = "data:text/html,<title>Form</title><label>Name <input id=name></label>\
        <button onclick=\"document.title='Hello '+document.getElementById('name').value;console.error('sent')\">Send</button>";

    let opened = call(&sandbox, json!({ "action": "open", "url": page })).await.unwrap();
    assert!(opened.output.contains("Started"), "{}", opened.output);

    let snapshot = call(&sandbox, json!({ "action": "snapshot", "interactive": true }))
        .await
        .unwrap();
    let textbox = reference(&snapshot.output, "textbox");
    let button = reference(&snapshot.output, "button");

    let filled = call(
        &sandbox,
        json!({ "action": "batch", "actions": [
            { "action": "fill", "ref": textbox, "text": "Ada" },
            { "action": "click", "ref": button }
        ]}),
    )
    .await
    .unwrap();
    assert!(filled.output.contains("2. click"), "{}", filled.output);
    let tabs = sandbox.ctx.engine.browsers.record("ses_test").unwrap().pages;
    assert_eq!(
        tabs[0].title, "Hello Ada",
        "a title the page's script set reaches the tab"
    );

    let title = call(&sandbox, json!({ "action": "eval", "expression": "document.title" }))
        .await
        .unwrap();
    assert_eq!(title.output, "Hello Ada");
    let logs = call(&sandbox, json!({ "action": "logs", "search": "sent" }))
        .await
        .unwrap();
    assert!(logs.output.contains("[error] sent"), "{}", logs.output);

    let agent = call(
        &sandbox,
        json!({ "action": "eval", "expression": "navigator.userAgent + ' ' + navigator.webdriver" }),
    )
    .await
    .unwrap();
    assert!(!agent.output.contains("Headless"), "{}", agent.output);
    assert!(agent.output.ends_with("false"), "{}", agent.output);

    let shot = call(&sandbox, json!({ "action": "screenshot" })).await.unwrap();
    assert_eq!(shot.metadata.images.map(|images| images.len()), Some(1));

    // The pane: a watcher gets frames of the page, and what the user types reaches it.
    let instance = sandbox.ctx.engine.browsers.get("ses_test").unwrap();
    let mut viewer = crate::browser::driver::watch(instance.clone());
    let frame = tokio::time::timeout(std::time::Duration::from_secs(10), viewer.frames.recv())
        .await
        .expect("a frame within 10 s")
        .unwrap();
    assert!(frame.jpeg.starts_with(&[0xFF, 0xD8]));
    assert!(frame.width > 0.0 && frame.height > 0.0);
    call(&sandbox, json!({ "action": "fill", "ref": textbox, "text": "" }))
        .await
        .unwrap();
    let typed = crate::browser::ViewInput::Text { text: "Grace".into() };
    instance.input(typed).await.unwrap();
    let value = call(
        &sandbox,
        json!({ "action": "eval", "expression": "document.getElementById('name').value" }),
    )
    .await
    .unwrap();
    assert_eq!(value.output, "Grace");
    drop(viewer);

    sees_a_tall_page_small_clicks_where_seen_and_zooms(&sandbox).await;

    call(&sandbox, json!({ "action": "stop" })).await.unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    while sandbox.ctx.engine.browsers.record("ses_test").is_some() {
        assert!(tokio::time::Instant::now() < deadline, "the browser ended");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let profile = sandbox.ctx.engine.data_dir.join("browsers/sessions/ses_test");
    assert!(!profile.exists(), "an isolated profile goes with its browser");
}

/// A tall page's screenshot costs a desktop one's, a click at its points lands, and zoom enlarges a region of it.
async fn sees_a_tall_page_small_clicks_where_seen_and_zooms(sandbox: &Sandbox) {
    let page = "data:text/html,<body style='margin:0'><button style='position:absolute;left:600px;top:1500px;\
        width:200px;height:100px' onclick=\"document.title='Booked'\">Book</button>";
    call(sandbox, json!({ "action": "navigate", "url": page }))
        .await
        .unwrap();
    call(sandbox, json!({ "action": "viewport", "width": 1280, "height": 2048 }))
        .await
        .unwrap();

    let shot = call(sandbox, json!({ "action": "screenshot" })).await.unwrap();
    assert_eq!(size_in(&shot.output), (800, 1280), "{}", shot.output);

    // The button's middle, 700,1550 on the page, is 437.5,968.75 in the screenshot.
    call(sandbox, json!({ "action": "click", "x": 437.5, "y": 968.75 }))
        .await
        .unwrap();
    let title = call(sandbox, json!({ "action": "eval", "expression": "document.title" }))
        .await
        .unwrap();
    assert_eq!(title.output, "Booked");

    let zoomed = call(sandbox, json!({ "action": "zoom", "region": [375, 937.5, 500, 1000] }))
        .await
        .unwrap();
    assert_eq!(size_in(&zoomed.output), (800, 400), "{}", zoomed.output);
    assert_eq!(zoomed.metadata.images.map(|images| images.len()), Some(1));
}

/// The last `WxH` size a result names.
fn size_in(text: &str) -> (u32, u32) {
    let size = text
        .split([' ', ','])
        .map(|word| word.trim_end_matches('.'))
        .filter_map(|word| word.split_once('x'))
        .filter_map(|(width, height)| Some((width.parse().ok()?, height.parse().ok()?)))
        .next_back();

    size.unwrap_or_else(|| panic!("no size in: {text}"))
}

/// The ref of the first line in a snapshot with this role.
fn reference(snapshot: &str, role: &str) -> u64 {
    let line = snapshot
        .lines()
        .find(|line| line.trim_start().starts_with(&format!("- {role}")))
        .unwrap_or_else(|| panic!("no {role} in:\n{snapshot}"));
    let start = line.find("ref=").unwrap() + 4;
    let digits: String = line[start..].chars().take_while(char::is_ascii_digit).collect();

    digits.parse().unwrap()
}

/// A small booking form, written so IE's oldest modes run it too.
const FORM: &str = "<!doctype html><html><head><title>Book a table</title></head><body>\
<p id=note>Tables for two to six.</p><label for=guest>Name</label> <input id=guest name=guest>\
<select id=time><option>18:30</option><option>19:00</option></select>\
<button type=button id=reserve style=\"cursor:pointer\" onclick=\"document.title='Hello '+document.getElementById('guest').value\">Reserve</button>\
</body></html>";

/// Serves `html` on loopback for as long as the test runs; IE mode cannot open a `data:` address.
async fn serve(html: &'static str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut request = [0u8; 4096];
                let _ = stream.read(&mut request).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{html}",
                    html.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });

    format!("http://127.0.0.1:{port}/")
}

/// Opens the form in `engine`, fills and sends it, takes a picture, watches it, and stops.
async fn drives(engine: &str) {
    let sandbox = Sandbox::new(&format!("browser-{engine}"));
    let named = std::env::var_os(format!("DRIFT_TEST_{}", engine.to_uppercase()));
    if let Some(path) = named.filter(|path| std::path::Path::new(path).is_file()) {
        let kind = crate::browser::Kind::parse(engine).unwrap();
        sandbox.ctx.engine.set_browser_path(kind, path.to_str()).unwrap();
    }
    let url = serve(FORM).await;

    let opened = call(&sandbox, json!({ "action": "open", "url": url, "engine": engine }))
        .await
        .unwrap();
    assert!(opened.output.contains("Started"), "{}", opened.output);
    if engine == "camoufox" {
        let agent = call(
            &sandbox,
            json!({ "action": "eval", "expression": "navigator.userAgent" }),
        );
        let agent = agent.await.unwrap().output;
        assert!(agent.contains("Firefox/") && !agent.contains("Camoufox"), "{agent}");
    }

    fills_the_form(&sandbox, engine).await;
    fits_the_pane_and_follows_the_cursor(&sandbox, engine).await;
    watches_and_stops(&sandbox).await;
}

/// A pane of 1400x900 lays the page out at that size, and the pointer over the button shows the page's cursor.
async fn fits_the_pane_and_follows_the_cursor(sandbox: &Sandbox, engine: &str) {
    use crate::browser::ViewInput;

    if engine == "ie" {
        return;
    }
    let driver = sandbox.ctx.engine.browsers.get("ses_test").unwrap();
    driver
        .input(ViewInput::Fit {
            width: 1400.0,
            height: 900.0,
        })
        .await
        .unwrap();
    let mut size = String::new();
    for _ in 0..50 {
        let measured = call(
            sandbox,
            json!({ "action": "eval", "expression": "innerWidth + 'x' + innerHeight" }),
        );
        size = measured.await.unwrap().output;
        if size == "1400x900" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(size, "1400x900", "{engine}");

    for (element, shown) in [("reserve", "pointer"), ("note", "text"), ("guest", "text")] {
        let seen = cursor_over(sandbox, &driver, element).await;
        assert_eq!(seen, shown, "{engine} over #{element}");
    }
}

/// Moves the pane's pointer just inside an element and waits for the cursor the page shows there.
async fn cursor_over(
    sandbox: &Sandbox,
    driver: &std::sync::Arc<dyn crate::browser::driver::Driver>,
    id: &str,
) -> String {
    let rect = format!("JSON.stringify(document.getElementById('{id}').getBoundingClientRect())");
    let rect = call(sandbox, json!({ "action": "eval", "expression": rect }))
        .await
        .unwrap();
    let rect: Value = serde_json::from_str(&rect.output).unwrap();
    let mut cursor = driver.common().cursor.subscribe();
    cursor.borrow_and_update();
    let mouse = crate::browser::input::MouseInput {
        event: "move".into(),
        x: rect["x"].as_f64().unwrap() + 4.0,
        y: rect["y"].as_f64().unwrap() + rect["height"].as_f64().unwrap() / 2.0,
        button: None,
        click_count: None,
        delta_x: None,
        delta_y: None,
        modifiers: 0,
    };
    driver.input(crate::browser::ViewInput::Mouse(mouse)).await.unwrap();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), cursor.changed()).await;

    cursor.borrow().clone()
}

async fn fills_the_form(sandbox: &Sandbox, engine: &str) {
    let snapshot = call(sandbox, json!({ "action": "snapshot", "interactive": true }))
        .await
        .unwrap();
    let textbox = reference(&snapshot.output, "textbox");
    let button = reference(&snapshot.output, "button");
    let combobox = reference(&snapshot.output, "combobox");
    call(
        sandbox,
        json!({ "action": "batch", "actions": [
            { "action": "fill", "ref": textbox, "text": "Ada" },
            { "action": "fill", "ref": combobox, "text": "19:00" },
            { "action": "click", "ref": button }
        ]}),
    )
    .await
    .unwrap();

    let title = call(sandbox, json!({ "action": "eval", "expression": "document.title" }))
        .await
        .unwrap();
    assert_eq!(title.output, "Hello Ada", "{engine}");
    let chosen = call(
        sandbox,
        json!({ "action": "eval", "expression": "document.getElementById('time').value" }),
    )
    .await
    .unwrap();
    assert_eq!(chosen.output, "19:00", "{engine}");
}

async fn watches_and_stops(sandbox: &Sandbox) {
    let shot = call(sandbox, json!({ "action": "screenshot" })).await.unwrap();
    assert_eq!(shot.metadata.images.map(|images| images.len()), Some(1));

    let driver = sandbox.ctx.engine.browsers.get("ses_test").unwrap();
    let mut viewer = crate::browser::driver::watch(driver.clone());
    let frame = tokio::time::timeout(std::time::Duration::from_secs(20), viewer.frames.recv())
        .await
        .expect("a frame within 20 s")
        .unwrap();
    assert!(frame.jpeg.starts_with(&[0xFF, 0xD8]));
    drop(viewer);

    call(sandbox, json!({ "action": "stop" })).await.unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    while sandbox.ctx.engine.browsers.record("ses_test").is_some() {
        assert!(tokio::time::Instant::now() < deadline, "the browser ended");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// Needs Firefox installed, or `DRIFT_TEST_FIREFOX` naming its program.
#[tokio::test(flavor = "multi_thread")]
async fn firefox_fills_a_form_over_bidi() {
    let installed = crate::browser::engines::executable(crate::browser::Kind::Firefox).is_some();
    if installed || std::env::var_os("DRIFT_TEST_FIREFOX").is_some() {
        drives("firefox").await;
    }
}

/// Needs `DRIFT_TEST_CAMOUFOX` naming Camoufox's program.
#[tokio::test(flavor = "multi_thread")]
async fn camoufox_fills_a_form_over_bidi() {
    if std::env::var_os("DRIFT_TEST_CAMOUFOX").is_some() {
        drives("camoufox").await;
    }
}

/// Opens a real Edge window in IE mode, so it runs only when `DRIFT_TEST_IE` is set.
#[tokio::test(flavor = "multi_thread")]
async fn ie_mode_fills_a_form_through_iedriver() {
    if std::env::var_os("DRIFT_TEST_IE").is_some() {
        drives("ie").await;
    }
}

const SIGN_IN: &str = "<!doctype html><html><head><title>Sign in</title></head><body>\
<form onsubmit=\"document.title='Welcome '+document.getElementById('user').value+' '+document.getElementById('pass').value.length;return false\">\
<input id=user><input id=pass type=password><button id=go>Sign in</button></form></body></html>";

/// The project's sign-in fills the form from a stored credential, and the password never reaches the model.
#[tokio::test(flavor = "multi_thread")]
async fn login_signs_in_with_a_stored_credential_the_model_never_sees() {
    if crate::browser::installed().is_none() {
        return;
    }
    let sandbox = Sandbox::new("browser-login");
    let url = serve(SIGN_IN).await;
    let workspace = sandbox
        .ctx
        .engine
        .store
        .add_workspace(&sandbox.ctx.workspace.to_string_lossy(), "Harbour Kitchen", "")
        .unwrap();
    let session = sandbox
        .ctx
        .engine
        .store
        .create_session(crate::store::NewSession {
            workspace_id: &workspace.id,
            parent_id: None,
            visibility: crate::session::types::Visibility::Sibling,
            title: "Sign-in check",
            agent: "build",
            model: None,
        })
        .unwrap();
    let mut ctx = sandbox.ctx_clone();
    ctx.session_id = session.id.clone();
    let config = format!(
        r##"{{ "browser": {{ "baseUrl": "{url}", "login": {{ "credential": "front", "fields": {{ "#user": "{{user}}", "#pass": "{{password}}" }}, "submit": "#go" }} }} }}"##
    );
    std::fs::write(sandbox.ctx.workspace.join("drift.json"), config).unwrap();
    let credential = crate::browser::project::Credential {
        name: "front".into(),
        user: "front.desk".into(),
        domain: String::new(),
        has_password: false,
    };
    sandbox
        .ctx
        .engine
        .save_browser_credential(&workspace.id, &credential, Some("Quay-Side-7"))
        .unwrap();

    let opened = Browser.run(&ctx, json!({ "action": "open" })).await.unwrap();
    assert!(opened.output.contains("`login`"), "{}", opened.output);
    let signed = Browser.run(&ctx, json!({ "action": "login" })).await.unwrap();
    let echoed = Browser
        .run(
            &ctx,
            json!({ "action": "eval", "expression": "document.title + ' ' + document.getElementById('pass').value" }),
        )
        .await
        .unwrap();

    assert!(signed.output.contains("front.desk"), "{}", signed.output);
    assert!(echoed.output.starts_with("Welcome front.desk 11"), "{}", echoed.output);
    assert!(
        !echoed.output.contains("Quay-Side-7"),
        "a password the page shows is hidden: {}",
        echoed.output
    );
    assert!(echoed.output.contains("[password]"), "{}", echoed.output);
    sandbox.ctx.engine.browsers.stop_all();
}

/// Fetches IEDriverServer from Selenium's releases into Drift's engines folder; runs when `DRIFT_TEST_DOWNLOADS` is set.
#[tokio::test(flavor = "multi_thread")]
async fn iedriver_downloads_into_the_engines_folder() {
    if std::env::var_os("DRIFT_TEST_DOWNLOADS").is_none() {
        return;
    }
    let sandbox = Sandbox::new("browser-download");
    let engine = sandbox.ctx.engine.clone();

    engine.download_browser_engine(crate::browser::Kind::Ie).unwrap();
    let target = engine.data_dir.join("browsers/engines/IEDriverServer.exe");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    while !target.is_file() || engine.browser_engines().iter().any(|status| status.download.is_some()) {
        let failed = engine
            .browser_engines()
            .iter()
            .find_map(|status| status.download.clone()?.error);
        assert!(failed.is_none(), "{failed:?}");
        assert!(tokio::time::Instant::now() < deadline, "downloaded within 60 s");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    assert!(std::fs::metadata(&target).unwrap().len() > 100_000);
}

/// A shop's front page with what a trace should find: a stylesheet that blocks rendering, a slow hero image that
/// is the largest paint, a script with a long task, a banner that shifts the layout, and a slow click handler.
fn shop_page(path: &str) -> (&'static str, Vec<u8>, u64) {
    match path {
        "/style.css" => (
            "text/css",
            b"body{font-family:sans-serif;margin:0}.hero{width:900px;height:500px;display:block}".to_vec(),
            250,
        ),
        "/app.js" => (
            "text/javascript",
            br"
            const start = performance.now();
            while (performance.now() - start < 180) {}
            setTimeout(() => {
                const banner = document.createElement('p');
                banner.textContent = 'Free delivery on orders over 40 pounds this week';
                banner.style.height = '120px';
                document.body.prepend(banner);
            }, 300);
            document.getElementById('basket').addEventListener('click', () => {
                const until = performance.now() + 220;
                while (performance.now() < until) {}
                document.getElementById('count').textContent = '1';
            });"
            .to_vec(),
            0,
        ),
        "/hero.svg" => ("image/svg+xml", hero(), 600),
        _ => (
            "text/html",
            br#"<!doctype html><html><head><title>Harbour Kitchen</title><link rel=stylesheet href=/style.css></head>
<body><h1>Harbour Kitchen</h1><img class=hero src=/hero.svg alt="Dining room"><p>Basket: <span id=count>0</span></p>
<button id=basket>Add to basket</button><script src=/app.js></script></body></html>"#
                .to_vec(),
            0,
        ),
    }
}

/// Chrome passes over images too plain to be the largest paint, so the hero has enough detail to count.
fn hero() -> Vec<u8> {
    let mut svg = String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" width="900" height="500">"#);
    for index in 0..300u32 {
        let x = index * 37 % 900;
        let y = index * 53 % 500;
        let shade = index * 7 % 255;
        let _ = write!(
            svg,
            r#"<circle cx="{x}" cy="{y}" r="{}" fill="rgb({shade},120,{})"/>"#,
            10 + index % 40,
            255 - shade
        );
    }
    svg.push_str("</svg>");

    svg.into_bytes()
}

async fn serve_shop() -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut request = [0u8; 4096];
                let read = stream.read(&mut request).await.unwrap_or(0);
                let line = String::from_utf8_lossy(&request[..read]);
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                let (kind, body, delay) = shop_page(&path);
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: {kind}\r\ncontent-length: {}\r\ncache-control: no-store\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(&body).await;
            });
        }
    });

    format!("http://127.0.0.1:{port}/")
}

/// Traces the shop loading and a slow click in the real browser, and reads back what the summary should name.
#[tokio::test(flavor = "multi_thread")]
async fn a_trace_names_the_largest_paint_the_long_script_and_the_slow_click() {
    if crate::browser::installed().is_none() {
        return;
    }
    let sandbox = Sandbox::new("browser-trace");
    let url = serve_shop().await;
    call(&sandbox, json!({ "action": "open", "url": url })).await.unwrap();

    let loaded = call(&sandbox, json!({ "action": "trace", "throttle": true }))
        .await
        .unwrap()
        .output;
    let snapshot = call(&sandbox, json!({ "action": "snapshot", "interactive": true }))
        .await
        .unwrap();
    let button = reference(&snapshot.output, "button");
    call(&sandbox, json!({ "action": "trace", "record": "start" }))
        .await
        .unwrap();
    call(&sandbox, json!({ "action": "click", "ref": button }))
        .await
        .unwrap();
    let clicked = call(&sandbox, json!({ "action": "trace", "record": "stop" }))
        .await
        .unwrap()
        .output;
    sandbox.ctx.engine.browsers.stop_all();

    assert!(
        loaded.contains("Largest contentful paint:") && loaded.contains("IMG class='hero'"),
        "{loaded}"
    );
    assert!(loaded.contains("hero.svg"), "{loaded}");
    assert!(loaded.contains("running app.js"), "{loaded}");
    assert!(loaded.contains("Render-blocking: /style.css"), "{loaded}");
    assert!(loaded.contains("Cumulative layout shift: 0.1"), "{loaded}");
    let path = loaded
        .split("The trace is ")
        .nth(1)
        .and_then(|rest| rest.split(" (open").next())
        .unwrap();
    assert!(std::path::Path::new(path).is_file(), "{path}");
    assert!(clicked.starts_with("Recording of"), "{clicked}");
    assert!(clicked.contains("a click event"), "{clicked}");
    assert!(
        clicked.contains("click at") && clicked.contains("in handlers"),
        "{clicked}"
    );
}

/// Installs Lighthouse with npm and audits the shop; runs when `DRIFT_TEST_LIGHTHOUSE` is set, as it downloads.
#[tokio::test(flavor = "multi_thread")]
async fn lighthouse_audits_a_page_in_the_conversations_browser() {
    if std::env::var_os("DRIFT_TEST_LIGHTHOUSE").is_none() || crate::browser::installed().is_none() {
        return;
    }
    let sandbox = Sandbox::new("browser-audit");
    let url = serve_shop().await;
    call(&sandbox, json!({ "action": "open", "url": url })).await.unwrap();

    let audited = call(&sandbox, json!({ "action": "audit" })).await;
    let snapshot = call(
        &sandbox,
        json!({ "action": "audit", "mode": "snapshot", "device": "mobile" }),
    )
    .await;
    let still = call(&sandbox, json!({ "action": "eval", "expression": "document.title" })).await;
    sandbox.ctx.engine.browsers.stop_all();

    let audited = audited.unwrap().output;
    assert!(audited.contains("Scores: Accessibility"), "{audited}");
    assert!(audited.contains("lang"), "the page's html has no lang: {audited}");
    assert!(audited.contains("lighthouse.html"), "{audited}");
    let snapshot = snapshot.unwrap().output;
    assert!(snapshot.contains("(snapshot, a phone)"), "{snapshot}");
    assert_eq!(
        still.unwrap().output,
        "Harbour Kitchen",
        "the page is still usable after the audits"
    );
}
