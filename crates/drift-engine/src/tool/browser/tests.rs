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
    let mut viewer = instance.watch();
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
