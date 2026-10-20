use serde_json::json;

use super::*;
use crate::tool::tests::Sandbox;

async fn call(sandbox: &Sandbox, input: Value) -> Result<Output, ToolError> {
    Browser.run(&sandbox.ctx, input).await
}

#[test]
fn looking_is_a_read_and_acting_is_not() {
    for action in ["snapshot", "screenshot", "logs", "pages", "wait"] {
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

    call(&sandbox, json!({ "action": "stop" })).await.unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    while sandbox.ctx.engine.browsers.record("ses_test").is_some() {
        assert!(tokio::time::Instant::now() < deadline, "the browser ended");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let profile = sandbox.ctx.engine.data_dir.join("browsers/sessions/ses_test");
    assert!(!profile.exists(), "an isolated profile goes with its browser");
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
<label for=guest>Name</label> <input id=guest name=guest>\
<select id=time><option>18:30</option><option>19:00</option></select>\
<button type=button id=reserve onclick=\"document.title='Hello '+document.getElementById('guest').value\">Reserve</button>\
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

    fills_the_form(&sandbox, engine).await;
    watches_and_stops(&sandbox).await;
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
