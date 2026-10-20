//! Each browser action: find the conversation's browser and page, act, and say what changed.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use grep::matcher::Matcher;
use grep::regex::RegexMatcher;
use serde_json::{Value, json};

use super::super::Context;
use super::super::image::Image;
use super::Step;
use crate::browser::{Instance, Page, Settled, go, parse_chord, viewport};

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
            let instance = browser(context, action).await?;
            instance.touch();
            let page = instance.page(input["page"].as_str())?;
            if let Some(dialog) = page.dialog().filter(|_| !PAST_DIALOG.contains(&action)) {
                return Err(format!(
                    "the page is showing a {} dialog (\"{}\"); answer it with the dialog action first",
                    dialog.kind, dialog.message
                ));
            }

            on_page(context, &instance, &page, action, input).await
        }
    }
}

/// The conversation's browser; navigating starts one, other actions need one already open.
async fn browser(context: &Context, action: &str) -> Result<Arc<Instance>, String> {
    if let Some(instance) = context.engine.browsers.get(&context.session_id) {
        return Ok(instance);
    }
    if action != "navigate" {
        return Err("this conversation has no browser open; `open` a page first".into());
    }

    let (instance, _) = context.engine.open_browser(&context.session_id).await?;

    Ok(instance)
}

async fn on_page(
    context: &Context,
    instance: &Arc<Instance>,
    page: &Arc<Page>,
    action: &str,
    input: &Value,
) -> Result<Step, String> {
    match action {
        "snapshot" => snapshot(page, input).await,
        "screenshot" => screenshot(page).await,
        "logs" => logs(page, input).await,
        "eval" => eval(page, input).await,
        "wait" => wait(context, page, input).await,
        "dialog" => dialog(page, input).await,
        "viewport" => viewport::set(instance, page, input).await.map(Step::text),
        "select" => {
            instance.select(&page.target);
            Ok(Step::text(format!("Tab {} is active: {}", page.target, describe(page))))
        }
        "close" => {
            instance.close_page(&page.target).await?;
            Ok(Step::text(format!("Closed tab {}.", page.target)))
        }
        _ => act(instance, page, action, input).await,
    }
}

/// Actions that can change the page: they report the address, title, dialog or tab they led to.
async fn act(instance: &Arc<Instance>, page: &Arc<Page>, action: &str, input: &Value) -> Result<Step, String> {
    let url_before = page.url();
    let tabs_before = instance.pages().len();

    let done = match action {
        "navigate" => {
            let url = input["url"].as_str().ok_or("`url` is required")?;
            go(page, url).await?;
            "Loaded.".to_string()
        }
        "click" | "hover" => pointer(page, action, input).await?,
        "fill" => fill(page, input).await?,
        "type" => {
            let text = input["text"].as_str().ok_or("`text` is required")?;
            page.insert_text(text).await.map_err(|error| error.to_string())?;
            format!("Typed {} characters.", text.chars().count())
        }
        "press" => {
            let chord = input["key"].as_str().ok_or("`key` is required")?;
            let (key, modifiers) = parse_chord(chord)?;
            page.press(&key, modifiers).await.map_err(|error| error.to_string())?;
            format!("Pressed {chord}.")
        }
        "scroll" => scroll(page, input).await?,
        other => return Err(format!("unknown action `{other}`")),
    };

    let mut settled = page.settle(&url_before).await;
    instance.refresh().await;
    settled.title = page.record(false).title;
    let mut text = done;
    report(&mut text, &settled, action == "navigate");
    report_tabs(&mut text, instance, tabs_before);

    Ok(Step::text(text))
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

fn report_tabs(text: &mut String, instance: &Instance, before: usize) {
    let pages = instance.pages();
    if pages.len() <= before {
        return;
    }
    if let Some(page) = pages.last() {
        let _ = write!(
            text,
            " A new tab opened ({}): {}; it is now the active tab.",
            page.target,
            page.url()
        );
    }
}

async fn open(context: &Context, input: &Value) -> Result<Step, String> {
    let url = crate::browser::with_scheme(input["url"].as_str().unwrap_or("about:blank"));
    let started = context.engine.browsers.get(&context.session_id).is_none();
    let (instance, note) = context.engine.open_browser(&context.session_id).await?;
    instance.touch();

    // A browser just started has one blank tab; the first address goes there rather than beside it.
    let blank = instance
        .pages()
        .into_iter()
        .find(|page| page.url() == "about:blank" && instance.is_active(page));
    let page = match blank.filter(|_| started) {
        Some(page) => {
            page.navigate(&url).await?;
            page
        }
        None => instance.new_page(&url).await?,
    };
    instance.refresh().await;

    let mut text = String::new();
    if started {
        let _ = write!(
            text,
            "Started {} with {} profile. ",
            instance.kind.label(),
            match instance.storage {
                crate::browser::Storage::Isolated => "its own (signed out)",
                crate::browser::Storage::Shared => "the workspace's shared",
            }
        );
    }
    let _ = write!(text, "Tab {} shows {}.", page.target, describe(&page));
    if let Some(note) = note {
        let _ = write!(text, " {note}");
    }
    let settled = page.settle(&page.url()).await;
    report(&mut text, &settled, false);

    Ok(Step::text(text))
}

fn stop(context: &Context) -> Step {
    let Some(instance) = context.engine.browsers.get(&context.session_id) else {
        return Step::text("This conversation has no browser open.");
    };
    instance.stop();

    Step::text("The browser is closing.")
}

fn pages(context: &Context) -> Step {
    let Some(instance) = context.engine.browsers.get(&context.session_id) else {
        return Step::text("This conversation has no browser open.");
    };

    let lines: Vec<String> = instance
        .pages()
        .iter()
        .map(|page| {
            let marker = if instance.is_active(page) { "*" } else { " " };
            let popup = if page.popup { " (opened by the page)" } else { "" };
            format!("{marker} {}  {}{popup}", page.target, describe(page))
        })
        .collect();
    if lines.is_empty() {
        return Step::text("The browser has no tabs open.");
    }

    Step::text(format!("Tabs (* is active):\n{}", lines.join("\n")))
}

fn describe(page: &Page) -> String {
    let record = page.record(false);
    if record.title.is_empty() {
        return record.url;
    }

    format!("\"{}\" at {}", record.title, record.url)
}

async fn snapshot(page: &Page, input: &Value) -> Result<Step, String> {
    let options = json!({ "interactive": input["interactive"].as_bool().unwrap_or(false) });
    let tree = page.helper(&format!("__drift.snapshot({options})")).await?;
    let tree = tree.as_str().unwrap_or_default();
    let body = if tree.trim().is_empty() {
        "(the page shows nothing to read)"
    } else {
        tree
    };

    Ok(Step::text(format!("{}\n\n{}", describe(page), cut(body))))
}

async fn screenshot(page: &Page) -> Result<Step, String> {
    let (jpeg, width, height) = page.screenshot().await?;
    let text = format!(
        "Screenshot of {}, {width}x{height}. Click or hover at `x`/`y` in this image's pixels.",
        describe(page)
    );

    Ok(Step {
        text,
        images: vec![Image::from_bytes("image/jpeg", &jpeg)],
    })
}

/// The element's centre, or the screenshot point, then the click or hover there.
async fn pointer(page: &Page, action: &str, input: &Value) -> Result<String, String> {
    let (x, y, what) = match input["ref"].as_u64() {
        Some(reference) => {
            let point = page.helper(&format!("__drift.point({reference})")).await?;
            let x = point["x"].as_f64().ok_or("the element has no position")?;
            let y = point["y"].as_f64().ok_or("the element has no position")?;
            (x, y, format!("ref {reference}"))
        }
        None => {
            let x = input["x"].as_f64().ok_or("`ref`, or `x` and `y`, is required")?;
            let y = input["y"].as_f64().ok_or("`ref`, or `x` and `y`, is required")?;
            let (css_x, css_y) = page.to_css(x, y);
            (css_x, css_y, format!("{x},{y}"))
        }
    };

    if action == "hover" {
        page.mouse("mouseMoved", x, y, 0)
            .await
            .map_err(|error| error.to_string())?;
        return Ok(format!("Hovered {what}."));
    }
    let double = input["double"].as_bool().unwrap_or(false);
    page.click(x, y, double).await.map_err(|error| error.to_string())?;

    Ok(format!("Clicked {what}."))
}

async fn fill(page: &Page, input: &Value) -> Result<String, String> {
    let reference = input["ref"].as_u64().ok_or("`ref` is required")?;
    let text = input["text"].as_str().unwrap_or_default();
    let checked = input["checked"]
        .as_bool()
        .map_or("undefined".to_string(), |checked| checked.to_string());
    let call = format!("__drift.prepare({reference}, {}, {checked})", json!(text));
    let prepared = page.helper(&call).await?;

    if let Some(done) = prepared["done"].as_str() {
        return Ok(format!("Ref {reference}: {done}."));
    }
    if let Some(point) = prepared.get("click") {
        let x = point["x"].as_f64().unwrap_or_default();
        let y = point["y"].as_f64().unwrap_or_default();
        page.click(x, y, false).await.map_err(|error| error.to_string())?;
        return Ok(format!("Clicked ref {reference} to change it."));
    }

    // The field's text is selected, so typing replaces it; an empty value deletes it.
    if text.is_empty() {
        let (delete, _) = parse_chord("Delete")?;
        page.press(&delete, 0).await.map_err(|error| error.to_string())?;
        return Ok(format!("Cleared ref {reference}."));
    }
    page.insert_text(text).await.map_err(|error| error.to_string())?;

    Ok(format!("Filled ref {reference}."))
}

async fn scroll(page: &Page, input: &Value) -> Result<String, String> {
    if let Some(reference) = input["ref"].as_u64() {
        page.helper(&format!("__drift.scroll({reference})")).await?;
        return Ok(format!("Scrolled ref {reference} into view."));
    }

    let dy = input["dy"].as_f64().ok_or("`ref` or `dy` is required")?;
    page.wheel(400.0, 300.0, dy).await.map_err(|error| error.to_string())?;

    Ok(format!("Scrolled {dy} pixels."))
}

async fn wait(context: &Context, page: &Page, input: &Value) -> Result<Step, String> {
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
        let shown = page.helper("__drift.text()").await?;
        if shown.as_str().is_some_and(|shown| shown.contains(wanted)) {
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

async fn eval(page: &Page, input: &Value) -> Result<Step, String> {
    let expression = input["expression"].as_str().ok_or("`expression` is required")?;
    let value = page.evaluate(expression).await?;
    let text = match &value {
        Value::Null => "undefined or null".to_string(),
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    };

    Ok(Step::text(cut(&text)))
}

async fn logs(page: &Page, input: &Value) -> Result<Step, String> {
    if let Some(request) = input["request"].as_str() {
        let body = page.response_body(request).await?;
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
    let lines = page.logs(network, &matches, LOG_LINES);

    if lines.is_empty() {
        let what = if network { "requests" } else { "console messages" };
        return Ok(Step::text(format!("No {what} match.")));
    }

    Ok(Step::text(cut(&lines.join("\n"))))
}

async fn dialog(page: &Page, input: &Value) -> Result<Step, String> {
    let Some(open) = page.dialog() else {
        return Err("no dialog is open on this page".into());
    };
    let accept = input["accept"].as_bool().unwrap_or(true);
    page.answer_dialog(accept, input["text"].as_str()).await?;

    let answer = if accept { "OK" } else { "Cancel" };
    Ok(Step::text(format!(
        "Answered the {} \"{}\" with {answer}.",
        open.kind, open.message
    )))
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
