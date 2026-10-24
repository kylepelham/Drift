//! The model's browser: one per conversation, run by the engine and shown in the browser pane (`docs/browser.md`).

mod actions;
mod hosts;
mod login;
mod performance;

use std::fmt::Write as _;

use percent_encoding::{AsciiSet, CONTROLS, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};

use super::image::Image;
use super::{Ask, Context, Output, RunFuture, Tool, ToolError, ToolMetadata, required_str};
use crate::llm::ToolSpec;

/// Actions that only look, so a read-only agent may make them and they may run beside other reads.
const READS: &[&str] = &["snapshot", "screenshot", "zoom", "logs", "pages", "wait"];
/// The most actions one batch runs.
const MAX_BATCH: usize = 30;
/// The URL standard's percent-encode sets, so a password an address spells encoded is still found.
const FRAGMENT: &AsciiSet = &CONTROLS.add(b' ').add(b'"').add(b'<').add(b'>').add(b'`');
const QUERY: &AsciiSet = &CONTROLS.add(b' ').add(b'"').add(b'#').add(b'<').add(b'>');
const SPECIAL_QUERY: &AsciiSet = &QUERY.add(b'\'');
const PATH: &AsciiSet = &QUERY.add(b'?').add(b'^').add(b'`').add(b'{').add(b'}');
const USERINFO: &AsciiSet = &PATH
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'=')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'|');
const COMPONENT: &AsciiSet = &USERINFO.add(b'$').add(b'%').add(b'&').add(b'+').add(b',');
const ENCODINGS: [&AsciiSet; 7] = [
    FRAGMENT,
    QUERY,
    SPECIAL_QUERY,
    PATH,
    USERINFO,
    COMPONENT,
    NON_ALPHANUMERIC,
];

pub struct Browser;

/// What one action produced: text for the model and any screenshots.
#[derive(Default)]
pub(crate) struct Step {
    pub(crate) text: String,
    pub(crate) images: Vec<Image>,
}

impl Step {
    pub(crate) fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
        }
    }
}

impl Tool for Browser {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "browser".into(),
            description: include_str!("prompts/browser.txt").trim().into(),
            input_schema: schema(),
        }
    }

    fn ask(&self, _context: &Context, _input: &Value) -> Option<Ask> {
        None
    }

    /// Acting on a site asks once per host; dev servers and intranet hosts are allowed unless a rule says otherwise.
    fn asks(&self, context: &Context, input: &Value) -> Vec<Ask> {
        hosts::of_call(context, input)
            .into_iter()
            .map(|host| {
                let ask = Ask::new("browser", host.clone(), format!("Use the browser on {host}"));
                if hosts::is_private(&host) {
                    ask.allow_by_default()
                } else {
                    ask
                }
            })
            .collect()
    }

    fn mutates(&self) -> bool {
        true
    }

    fn call_mutates(&self, input: &Value) -> bool {
        match input["action"].as_str() {
            Some("batch") => input["actions"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|step| !READS.contains(&step["action"].as_str().unwrap_or_default())),
            Some(action) => !READS.contains(&action),
            None => true,
        }
    }

    /// A browser changes no file in the workspace.
    fn touches(&self, _context: &Context, _input: &Value) -> Option<Vec<std::path::PathBuf>> {
        Some(Vec::new())
    }

    fn run<'a>(&'a self, context: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let action = required_str(&input, "action")?;
            let step = if action == "batch" {
                batch(context, &input).await?
            } else {
                actions::run(context, action, &input).await.map_err(ToolError)?
            };

            Ok(output(action, &input, hide_passwords(context, step)))
        })
    }
}

/// Runs each action in order and stops at the first that fails, saying which.
async fn batch(context: &Context, input: &Value) -> Result<Step, ToolError> {
    let steps = input["actions"]
        .as_array()
        .filter(|steps| !steps.is_empty())
        .ok_or_else(|| {
            ToolError("`actions` is required: a list of actions such as {\"action\": \"click\", \"ref\": 4}".into())
        })?;
    if steps.len() > MAX_BATCH {
        return Err(ToolError(format!("a batch runs at most {MAX_BATCH} actions")));
    }

    let mut done = Step::default();
    for (index, step) in steps.iter().enumerate() {
        let action = step["action"].as_str().unwrap_or_default();
        if action == "batch" || action.is_empty() {
            return Err(ToolError(format!(
                "step {} needs an `action` other than batch",
                index + 1
            )));
        }

        match actions::run(context, action, step).await {
            Ok(result) => {
                let _ = writeln!(done.text, "{}. {action}: {}", index + 1, result.text);
                done.images.extend(result.images);
            }
            Err(error) => {
                let skipped = steps.len() - index - 1;
                let not_run = if skipped > 0 {
                    format!(" ({skipped} later steps were not run)")
                } else {
                    String::new()
                };
                let failed = format!("{}Step {} ({action}) failed: {error}{not_run}", done.text, index + 1);
                return Err(ToolError(failed));
            }
        }
    }

    Ok(done)
}

/// A password the engine typed can show up in an address afterwards; the model never sees one.
fn hide_passwords(context: &Context, mut step: Step) -> Step {
    let (workspace, _) = context.engine.browser_project(&context.session_id);
    let Some(workspace) = workspace else {
        return step;
    };

    for password in context.engine.browser_passwords(&workspace.id) {
        step.text = mask(&step.text, &password);
    }

    step
}

/// `text` with `password` hidden as written and as any address would percent-encode it.
fn mask(text: &str, password: &str) -> String {
    ENCODINGS
        .iter()
        .map(|set| utf8_percent_encode(password, set).to_string())
        .fold(text.replace(password, "[password]"), |text, encoded| {
            text.replace(&encoded, "[password]")
        })
}

fn output(action: &str, input: &Value, step: Step) -> Output {
    let title = match input["url"].as_str() {
        Some(url) => format!("Browser: {action} {url}"),
        None => format!("Browser: {action}"),
    };
    let images = (!step.images.is_empty()).then(|| super::image::metadata(&step.images));

    Output {
        title,
        output: step.text.trim_end().to_string(),
        metadata: ToolMetadata {
            images,
            ..ToolMetadata::default()
        },
    }
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": [
                    "open", "navigate", "snapshot", "click", "hover", "fill", "type", "press", "scroll", "wait",
                    "screenshot", "zoom", "eval", "logs", "dialog", "viewport", "trace", "audit", "pages", "select", "close", "login",
                    "batch", "stop"
                ],
                "description": "open: start the browser if needed and open `url` in a new tab (a path opens on the project's baseUrl); `engine` picks another browser, replacing this conversation's. navigate: load `url` (or back, forward, reload) in the page. snapshot: the page's elements with refs. click, hover: by `ref`, or `x`/`y` in the last screenshot's pixels. fill: set a field by `ref` to `text` (a select takes an option's value or label; a checkbox takes `checked`). type: text into the focused element. press: a key or chord such as Enter or Control+A. scroll: `ref` into view, or by `dy` pixels. wait: until `text` shows, or `ms` pass (at most 30000). screenshot: an image of the page, at most 1280x800's worth of pixels. zoom: `region` of what a screenshot shows now, enlarged to read small text or check fine detail. eval: run `expression` in the page. logs: console (default) or network. dialog: answer an alert, confirm or prompt. viewport: desktop, tablet, mobile, or `width`/`height`. trace: record a performance trace (Chrome and Edge) and read its Web Vitals, long tasks, network and interactions. audit: a Lighthouse audit (Chrome and Edge) of accessibility, best practices and SEO. pages: list tabs. select, close: a tab by `page`. login: sign in as the project's drift.json sets up, with a credential the user stored. batch: run `actions` in order. stop: end the browser."
            },
            "url": { "type": "string", "description": "For open and navigate. A bare host gets https://, localhost gets http://; a path is taken from the project's baseUrl." },
            "engine": { "type": "string", "enum": ["chrome", "edge", "firefox", "camoufox", "ie"], "description": "For open: the browser to use. Leave it out to use the project's or the user's choice. ie is Edge's Internet Explorer mode; camoufox is a hardened Firefox for sites that block automation." },
            "secret": { "type": "string", "enum": ["user", "password", "domain"], "description": "For fill: type a part of a stored credential instead of `text`; you never see it." },
            "credential": { "type": "string", "description": "For fill with `secret` and for login: the stored credential's name, if not the project's sign-in one." },
            "language": { "type": "string", "enum": ["javascript", "vbscript"], "description": "For eval: vbscript runs only in IE mode." },
            "ref": { "type": "integer", "description": "An element's ref from the last snapshot." },
            "x": { "type": "number" },
            "y": { "type": "number" },
            "double": { "type": "boolean", "description": "For click: a double click." },
            "region": { "type": "array", "items": { "type": "number" }, "minItems": 4, "maxItems": 4, "description": "For zoom: [x0, y0, x1, y1] in a screenshot's pixels." },
            "text": { "type": "string", "description": "For fill and type: the text. For wait: text to wait for. For dialog: a prompt's answer." },
            "checked": { "type": "boolean", "description": "For fill on a checkbox or radio button." },
            "key": { "type": "string", "description": "For press: Enter, Tab, Escape, ArrowDown, F5, a character, or a chord such as Control+Shift+K." },
            "dy": { "type": "number", "description": "For scroll: pixels down (negative scrolls up)." },
            "ms": { "type": "integer", "description": "For wait: the longest to wait, in milliseconds." },
            "interactive": { "type": "boolean", "description": "For snapshot: only elements you can act on." },
            "expression": { "type": "string", "description": "For eval: JavaScript; the value of the last expression comes back as JSON. `await` works." },
            "kind": { "type": "string", "enum": ["console", "network"], "description": "For logs." },
            "search": { "type": "string", "description": "For logs: a regex lines must match." },
            "request": { "type": "string", "description": "For logs: a request id from the network list, to read its response body." },
            "accept": { "type": "boolean", "description": "For dialog: OK (true, the default) or Cancel." },
            "preset": { "type": "string", "enum": ["desktop", "tablet", "mobile"], "description": "For viewport." },
            "record": { "type": "string", "enum": ["reload", "start", "stop"], "description": "For trace: reload (the default) records the page loading again; start and stop record what you do in between, such as a click that feels slow." },
            "throttle": { "type": "boolean", "description": "For trace: slow the CPU 4x and the network to slow 4G while recording, as a mid-range phone would be." },
            "mode": { "type": "string", "enum": ["navigation", "snapshot"], "description": "For audit: navigation (the default) loads the page again; snapshot audits it as it is now, such as after signing in or opening a dialog." },
            "device": { "type": "string", "enum": ["desktop", "mobile"], "description": "For audit: the device to emulate (default desktop)." },
            "width": { "type": "integer" },
            "height": { "type": "integer" },
            "page": { "type": "string", "description": "A tab's id from pages; without it, actions use the active tab." },
            "actions": { "type": "array", "items": { "type": "object" }, "description": "For batch: actions, each an object with `action` and that action's fields." }
        },
        "required": ["action"]
    })
}

#[cfg(test)]
mod tests;
