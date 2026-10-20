//! Each browser action: find the conversation's browser and page, act, and say what changed.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use grep::matcher::Matcher;
use grep::regex::RegexMatcher;
use serde_json::Value;

use super::super::Context;
use super::super::image::Image;
use super::Step;
use crate::browser::driver::{Driver, Language, Settled, Size, Tab, Target};
use crate::browser::input::parse_chord;
use crate::browser::{Kind, Storage, Wanted};

/// The most of a snapshot or a script's value one result carries.
const MAX_TEXT: usize = 50_000;
const MAX_WAIT: Duration = Duration::from_secs(30);
const DEFAULT_WAIT: Duration = Duration::from_secs(10);
const LOG_LINES: usize = 200;

/// Actions that work while a dialog blocks the page.
const PAST_DIALOG: &[&str] = &["dialog", "pages", "select", "close", "logs", "stop", "open"];

pub(super) async fn run(context: &Context, action: &str, input: &Value) -> Result<Step, String> {
    match action {
        "open" => open(context, input).await,
        "stop" => Ok(stop(context)),
        "pages" => Ok(pages(context)),
        _ => {
            let driver = browser(context, action).await?;
            driver.common().touch();
            let tab = driver.tab(input["page"].as_str())?;
            if let Some(dialog) = tab.dialog().filter(|_| !PAST_DIALOG.contains(&action)) {
                return Err(format!(
                    "the page is showing a {} dialog (\"{}\"); answer it with the dialog action first",
                    dialog.kind, dialog.message
                ));
            }

            on_page(context, &driver, &tab, action, input).await
        }
    }
}

/// The conversation's browser; navigating starts one, other actions need one already open.
async fn browser(context: &Context, action: &str) -> Result<Arc<dyn Driver>, String> {
    if let Some(driver) = context.engine.browsers.get(&context.session_id) {
        return Ok(driver);
    }
    if action != "navigate" {
        return Err("this conversation has no browser open; `open` a page first".into());
    }

    let (driver, _) = context
        .engine
        .open_browser(&context.session_id, Wanted::default())
        .await?;

    Ok(driver)
}

async fn on_page(
    context: &Context,
    driver: &Arc<dyn Driver>,
    tab: &Arc<dyn Tab>,
    action: &str,
    input: &Value,
) -> Result<Step, String> {
    match action {
        "snapshot" => snapshot(tab, input).await,
        "screenshot" => screenshot(tab).await,
        "logs" => logs(tab, input).await,
        "eval" => eval(tab, input).await,
        "wait" => wait(context, tab, input).await,
        "dialog" => dialog(tab, input).await,
        "viewport" => viewport(tab, input).await,
        "trace" => super::performance::trace(context, tab, input).await,
        "audit" => super::performance::audit(context, driver, tab, input).await,
        "select" => {
            driver.select(&tab.id()).await?;
            Ok(Step::text(format!(
                "Tab {} is active: {}",
                tab.id(),
                describe(tab.as_ref())
            )))
        }
        "close" => {
            driver.close_tab(&tab.id()).await?;
            Ok(Step::text(format!("Closed tab {}.", tab.id())))
        }
        _ => act(context, driver, tab, action, input).await,
    }
}

/// Actions that can change the page: they report the address, title, dialog or tab they led to.
async fn act(
    context: &Context,
    driver: &Arc<dyn Driver>,
    tab: &Arc<dyn Tab>,
    action: &str,
    input: &Value,
) -> Result<Step, String> {
    let url_before = tab.url();
    let tabs_before = driver.tabs().len();

    let done = match action {
        "navigate" => {
            let url = input["url"].as_str().ok_or("`url` is required")?;
            let (_, project) = context.engine.browser_project(&context.session_id);
            let url = if matches!(url, "back" | "forward" | "reload") {
                url.to_string()
            } else {
                project.resolve(url)
            };
            tab.go(&url).await?;
            "Loaded.".to_string()
        }
        "click" => {
            tab.click(target(input)?, input["double"].as_bool().unwrap_or(false))
                .await?
        }
        "hover" => tab.hover(target(input)?).await?,
        "fill" => fill(context, tab, input).await?,
        "login" => super::login::run(context, tab.as_ref(), input).await?,
        "type" => {
            let text = input["text"].as_str().ok_or("`text` is required")?;
            tab.type_text(text).await?;
            format!("Typed {} characters.", text.chars().count())
        }
        "press" => {
            let chord = input["key"].as_str().ok_or("`key` is required")?;
            let (key, modifiers) = parse_chord(chord)?;
            tab.press(&key, modifiers).await?;
            format!("Pressed {chord}.")
        }
        "scroll" => {
            let reference = input["ref"].as_u64();
            let dy = input["dy"].as_f64();
            if reference.is_none() && dy.is_none() {
                return Err("`ref` or `dy` is required".into());
            }
            tab.scroll(reference, dy.unwrap_or_default()).await?
        }
        other => return Err(format!("unknown action `{other}`")),
    };

    let mut settled = tab.settle(&url_before).await;
    driver.refresh().await;
    settled.title = tab.record(false).title;
    let mut text = done;
    report(&mut text, &settled, matches!(action, "navigate" | "login"));
    report_tabs(&mut text, driver.as_ref(), tabs_before);

    Ok(Step::text(text))
}

fn target(input: &Value) -> Result<Target, String> {
    if let Some(reference) = input["ref"].as_u64() {
        return Ok(Target::Ref(reference));
    }

    let x = input["x"].as_f64();
    let y = input["y"].as_f64();
    match (x, y) {
        (Some(x), Some(y)) => Ok(Target::Point(x, y)),
        _ => Err("`ref`, or `x` and `y`, is required".into()),
    }
}

fn report(text: &mut String, settled: &Settled, always: bool) {
    if settled.navigated || always {
        let _ = write!(text, " The page is now \"{}\" at {}.", settled.title, settled.url);
    }
    if let Some(dialog) = &settled.dialog {
        let _ = write!(
            text,
            " A {} dialog opened: \"{}\"; answer it with the dialog action.",
            dialog.kind, dialog.message
        );
    }
}

fn report_tabs(text: &mut String, driver: &dyn Driver, before: usize) {
    let tabs = driver.tabs();
    if tabs.len() <= before {
        return;
    }
    if let Some(tab) = tabs.last() {
        let _ = write!(
            text,
            " A new tab opened ({}): {}; it is now the active tab.",
            tab.id(),
            tab.url()
        );
    }
}

async fn open(context: &Context, input: &Value) -> Result<Step, String> {
    let engine = &context.engine;
    let (_, project) = engine.browser_project(&context.session_id);
    let url = match (input["url"].as_str(), &project.base_url) {
        (Some(url), _) => project.resolve(url),
        (None, Some(base)) => project.resolve(base),
        (None, None) => "about:blank".to_string(),
    };
    let kind = match input["engine"].as_str() {
        Some(name) => Some(Kind::parse(name).ok_or_else(|| format!("`{name}` is not an engine"))?),
        None => None,
    };

    let before = engine
        .browsers
        .get(&context.session_id)
        .map(|driver| driver.common().started_at);
    let wanted = Wanted { kind, url: Some(&url) };
    let (driver, notes) = engine.open_browser(&context.session_id, wanted).await?;
    driver.common().touch();
    let started = before != Some(driver.common().started_at);

    // A browser just started has one blank tab; the first address goes there rather than beside it.
    let blank = driver
        .tabs()
        .into_iter()
        .find(|tab| tab.url() == "about:blank" && driver.is_active(&tab.id()));
    let tab = match blank.filter(|_| started) {
        Some(tab) => {
            tab.go(&url).await?;
            tab
        }
        None => driver.open_tab(&url).await?,
    };
    driver.refresh().await;

    let mut text = String::new();
    if started {
        let profile = match driver.common().storage {
            Storage::Isolated => "its own (signed out)",
            Storage::Shared => "the workspace's shared",
        };
        let _ = write!(
            text,
            "Started {} with {profile} profile. ",
            driver.common().kind.label()
        );
    }
    let _ = write!(text, "Tab {} shows {}.", tab.id(), describe(tab.as_ref()));
    for note in notes.iter().chain(driver.opening_note().iter()) {
        let _ = write!(text, " {note}");
    }
    let settled = tab.settle(&tab.url()).await;
    report(&mut text, &settled, false);
    if started {
        project_notes(&mut text, &project);
    }

    Ok(Step::text(text))
}

/// What the project's drift.json says about its app, told when its browser starts.
fn project_notes(text: &mut String, project: &crate::browser::BrowserProject) {
    if project.login.is_some() {
        text.push_str(" This project has a sign-in set up: `login` signs in without you seeing the password.");
    }
    if let Some(notes) = project.notes.as_deref().filter(|notes| !notes.trim().is_empty()) {
        let _ = write!(text, "\n\nNotes for this app (from drift.json):\n{}", notes.trim());
    }
}

fn stop(context: &Context) -> Step {
    let Some(driver) = context.engine.browsers.get(&context.session_id) else {
        return Step::text("This conversation has no browser open.");
    };
    driver.stop();

    Step::text("The browser is closing.")
}

fn pages(context: &Context) -> Step {
    let Some(driver) = context.engine.browsers.get(&context.session_id) else {
        return Step::text("This conversation has no browser open.");
    };

    let lines: Vec<String> = driver
        .tabs()
        .iter()
        .map(|tab| {
            let id = tab.id();
            let marker = if driver.is_active(&id) { "*" } else { " " };
            let popup = if tab.record(false).popup {
                " (opened by the page)"
            } else {
                ""
            };
            format!("{marker} {id}  {}{popup}", describe(tab.as_ref()))
        })
        .collect();
    if lines.is_empty() {
        return Step::text("The browser has no tabs open.");
    }

    Step::text(format!(
        "{} tabs (* is active):\n{}",
        driver.common().kind.label(),
        lines.join("\n")
    ))
}

fn describe(tab: &dyn Tab) -> String {
    let record = tab.record(false);
    if record.title.is_empty() {
        return record.url;
    }

    format!("\"{}\" at {}", record.title, record.url)
}

async fn snapshot(tab: &Arc<dyn Tab>, input: &Value) -> Result<Step, String> {
    let tree = tab.snapshot(input["interactive"].as_bool().unwrap_or(false)).await?;
    let body = if tree.trim().is_empty() {
        "(the page shows nothing to read)"
    } else {
        &tree
    };

    Ok(Step::text(format!("{}\n\n{}", describe(tab.as_ref()), cut(body))))
}

async fn screenshot(tab: &Arc<dyn Tab>) -> Result<Step, String> {
    let shot = tab.screenshot().await?;
    let text = format!(
        "Screenshot of {}, {}x{}. Click or hover at `x`/`y` in this image's pixels.",
        describe(tab.as_ref()),
        shot.width,
        shot.height
    );

    Ok(Step {
        text,
        images: vec![Image::from_bytes("image/jpeg", &shot.jpeg)],
    })
}

/// A field filled from the agent's text, or from the project's stored credential without the agent seeing it.
async fn fill(context: &Context, tab: &Arc<dyn Tab>, input: &Value) -> Result<String, String> {
    let reference = input["ref"].as_u64().ok_or("`ref` is required")?;
    let checked = input["checked"].as_bool();
    let Some(part) = input["secret"].as_str() else {
        let text = input["text"].as_str().unwrap_or_default();
        return tab.fill(reference, text, checked).await;
    };

    let secret = super::login::secret(context, input["credential"].as_str())?;
    let value = match part {
        "user" => &secret.user,
        "password" => &secret.password,
        "domain" => &secret.domain,
        other => {
            return Err(format!(
                "`{other}` is not a credential part; use user, password or domain"
            ));
        }
    };
    tab.fill(reference, value, None).await?;

    Ok(format!("Filled ref {reference} with the stored {part}."))
}

async fn wait(context: &Context, tab: &Arc<dyn Tab>, input: &Value) -> Result<Step, String> {
    let limit = input["ms"]
        .as_u64()
        .map_or(DEFAULT_WAIT, Duration::from_millis)
        .min(MAX_WAIT);
    let Some(wanted) = input["text"].as_str() else {
        tokio::select! {
            () = tokio::time::sleep(limit) => {}
            () = context.abort.cancelled() => return Err("the wait was stopped".into()),
        }
        return Ok(Step::text(format!("Waited {} ms.", limit.as_millis())));
    };

    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if tab.text().await?.contains(wanted) {
            return Ok(Step::text(format!("\"{wanted}\" is on the page.")));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("\"{wanted}\" did not appear within {} ms", limit.as_millis()));
        }
        tokio::select! {
            () = tokio::time::sleep(Duration::from_millis(250)) => {}
            () = context.abort.cancelled() => return Err("the wait was stopped".into()),
        }
    }
}

async fn eval(tab: &Arc<dyn Tab>, input: &Value) -> Result<Step, String> {
    let expression = input["expression"].as_str().ok_or("`expression` is required")?;
    let language = match input["language"].as_str() {
        Some("vbscript") => Language::VbScript,
        _ => Language::JavaScript,
    };
    let value = tab.evaluate(expression, language).await?;
    let text = match &value {
        Value::Null => "undefined or null".to_string(),
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    };

    Ok(Step::text(cut(&text)))
}

async fn logs(tab: &Arc<dyn Tab>, input: &Value) -> Result<Step, String> {
    if let Some(request) = input["request"].as_str() {
        let body = tab.response_body(request).await?;
        return Ok(Step::text(cut(&body)));
    }

    let network = input["kind"].as_str() == Some("network");
    let matcher = input["search"]
        .as_str()
        .map(RegexMatcher::new)
        .transpose()
        .map_err(|error| format!("invalid regex: {error}"))?;
    let matches = |line: &str| {
        matcher
            .as_ref()
            .is_none_or(|matcher| matcher.is_match(line.as_bytes()).unwrap_or(false))
    };
    let lines = tab.logs(network, &matches, LOG_LINES)?;

    if lines.is_empty() {
        let what = if network { "requests" } else { "console messages" };
        return Ok(Step::text(format!("No {what} match.")));
    }

    Ok(Step::text(cut(&lines.join("\n"))))
}

async fn dialog(tab: &Arc<dyn Tab>, input: &Value) -> Result<Step, String> {
    let Some(open) = tab.dialog() else {
        return Err("no dialog is open on this page".into());
    };
    let accept = input["accept"].as_bool().unwrap_or(true);
    tab.answer_dialog(accept, input["text"].as_str()).await?;

    let answer = if accept { "OK" } else { "Cancel" };
    Ok(Step::text(format!(
        "Answered the {} \"{}\" with {answer}.",
        open.kind, open.message
    )))
}

async fn viewport(tab: &Arc<dyn Tab>, input: &Value) -> Result<Step, String> {
    let size = size(input)?;
    tab.viewport(size).await?;

    let text = match size {
        None => "The page is back at the browser's own size and was reloaded.".to_string(),
        Some(size) => {
            let phone = if size.mobile { " as a phone, with touch," } else { "" };
            format!(
                "The page is laid out at {}x{}{phone} and was reloaded.",
                size.width, size.height
            )
        }
    };

    Ok(Step::text(text))
}

/// The size asked for: a preset or a width and height; `None` is the browser's own.
pub(super) fn size(input: &Value) -> Result<Option<Size>, String> {
    let preset = input["preset"].as_str();
    let custom = input["width"].as_u64().zip(input["height"].as_u64());
    let sized = |width, height, mobile| Ok(Some(Size { width, height, mobile }));

    match (preset, custom) {
        (Some("desktop"), _) => Ok(None),
        (Some("tablet"), _) => sized(768, 1024, false),
        (Some("mobile"), _) => sized(390, 844, true),
        (Some(other), _) => Err(format!("`{other}` is not a preset; use desktop, tablet or mobile")),
        (None, Some((width, height))) => sized(width, height, width < 768),
        (None, None) => Err("`preset`, or `width` and `height`, is required".into()),
    }
}

/// Text cut to what a result carries, saying how much was left out.
fn cut(text: &str) -> String {
    if text.len() <= MAX_TEXT {
        return text.to_string();
    }

    let mut end = MAX_TEXT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }

    format!(
        "{}\n\n({} more characters were cut; use a snapshot with `interactive` or a narrower expression)",
        &text[..end],
        text.len() - end
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn presets_and_sizes() {
        assert!(size(&json!({ "preset": "desktop" })).unwrap().is_none());
        assert!(size(&json!({ "preset": "mobile" })).unwrap().unwrap().mobile);
        let narrow = size(&json!({ "width": 500, "height": 900 })).unwrap().unwrap();
        assert!(narrow.mobile, "under 768 wide is a phone");
        assert!(size(&json!({ "preset": "watch" })).is_err());
        assert!(size(&json!({})).is_err());
    }
}
