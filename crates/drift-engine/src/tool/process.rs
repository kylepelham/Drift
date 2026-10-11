//! The model's handle on the background processes its conversation started: list them, read or search what
//! they print, wait for a line or their end, type into them, interrupt or stop them.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use grep::matcher::Matcher;
use grep::regex::RegexMatcher;
use serde_json::{Value, json};

use super::{Ask, Context, Output, RunFuture, Tool, ToolError, ToolMetadata, required_str};
use crate::background::{Exit, Handle, ProcessState, Waited};
use crate::llm::ToolSpec;

/// The longest one call waits; the conversation is told when a process ends, so a longer wait is never needed.
const MAX_WAIT: Duration = Duration::from_secs(600);
const DEFAULT_WAIT: Duration = Duration::from_secs(60);
/// The most lines a search returns, newest last.
const SEARCH_LINES: usize = 200;

pub struct Process;

impl Tool for Process {
    fn permissions(&self) -> &'static [&'static str] {
        &["bash"]
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "process".into(),
            description: include_str!("prompts/process.txt").trim().into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "output", "wait", "input", "interrupt", "stop"],
                        "description": "list: this conversation's processes. output: what a process printed since you last looked, or the lines matching `search`. wait: block until a new line matches `until`, or until it ends. input: type `text` into it. interrupt: Ctrl+C. stop: end it and everything it started."
                    },
                    "id": { "type": "string", "description": "The process id bash returned. Required except for list." },
                    "search": { "type": "string", "description": "For output: a regex; returns the matching lines of its recent output instead of the new output." },
                    "until": { "type": "string", "description": "For wait: a regex a new line must match, such as `listening|ready|error`. Without it, wait waits for the process to end." },
                    "wait_seconds": { "type": "integer", "description": "For wait: the longest to wait, at most 600. Default 60." },
                    "text": { "type": "string", "description": "For input: what to type; end it with \\n to press Enter." }
                },
                "required": ["action"]
            }),
        }
    }

    /// Typing into a process runs whatever the program does with it, so it asks as a shell line would.
    fn ask(&self, context: &Context, input: &Value) -> Option<Ask> {
        if input["action"] != "input" {
            return None;
        }
        let text = input["text"].as_str().unwrap_or_default();
        let command = input["id"]
            .as_str()
            .and_then(|id| owned(context, id).ok())
            .map_or_else(String::new, |handle| handle.command.clone());

        Some(Ask::new("bash", text.trim_end(), format!("Type into `{command}`")))
    }

    fn mutates(&self) -> bool {
        true
    }

    fn call_mutates(&self, input: &Value) -> bool {
        matches!(input["action"].as_str(), Some("input" | "interrupt" | "stop"))
    }

    fn run<'a>(&'a self, context: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let action = required_str(&input, "action")?;
            if action == "list" {
                return Ok(list(context));
            }

            let handle = owned(context, required_str(&input, "id")?)?;
            match action {
                "output" => output(&handle, input["search"].as_str()),
                "wait" => wait(context, &handle, &input).await,
                "input" => type_into(&handle, required_str(&input, "text")?).await,
                "interrupt" => Ok(interrupt(&handle)),
                "stop" => Ok(stop(context, &handle).await),
                other => Err(ToolError(format!(
                    "unknown action `{other}`; use list, output, wait, input, interrupt or stop"
                ))),
            }
        })
    }
}

/// A process this conversation started; another conversation's are not its to touch.
fn owned(context: &Context, id: &str) -> Result<Arc<Handle>, ToolError> {
    context
        .engine
        .processes
        .get(id)
        .filter(|handle| handle.session_id == context.session_id)
        .ok_or_else(|| {
            ToolError(format!(
                "no background process `{id}` in this conversation; `list` shows them"
            ))
        })
}

fn list(context: &Context) -> Output {
    let handles = context.engine.processes.of(&context.session_id);
    if handles.is_empty() {
        return Output::new("Background processes", "This conversation has no background processes.");
    }

    let lines: Vec<String> = handles
        .iter()
        .map(|handle| {
            let record = handle.record();
            let last = record
                .last_line
                .map(|line| format!("\n    last: {line}"))
                .unwrap_or_default();
            format!("{}  {}  `{}`{last}", record.id, state(handle), record.command)
        })
        .collect();

    Output::new("Background processes", lines.join("\n"))
}

/// New output, or the lines matching `search`, under a line saying how the process is.
fn output(handle: &Handle, search: Option<&str>) -> Result<Output, ToolError> {
    let Some(pattern) = search else {
        return Ok(report(handle, ""));
    };

    let matcher = RegexMatcher::new(pattern).map_err(|error| ToolError(format!("invalid regex: {error}")))?;
    let matches = |line: &str| matcher.is_match(line.as_bytes()).unwrap_or(false);
    let found = handle.search(&matches, SEARCH_LINES);
    let text = if found.is_empty() {
        "No line of its recent output matches.".to_string()
    } else {
        found.join("\n")
    };

    Ok(described(handle, format!("{}\n\n{text}", status(handle))))
}

async fn wait(context: &Context, handle: &Handle, input: &Value) -> Result<Output, ToolError> {
    let limit = input["wait_seconds"]
        .as_u64()
        .map_or(DEFAULT_WAIT, Duration::from_secs)
        .min(MAX_WAIT);
    let matcher = input["until"]
        .as_str()
        .map(RegexMatcher::new)
        .transpose()
        .map_err(|error| ToolError(format!("invalid regex: {error}")))?;
    let matches = |line: &str| {
        matcher
            .as_ref()
            .is_some_and(|matcher| matcher.is_match(line.as_bytes()).unwrap_or(false))
    };
    let until = matcher.as_ref().map(|_| &matches as &(dyn Fn(&str) -> bool + Sync));

    let waited = handle.wait(until, limit, &context.abort).await;
    let why = match waited {
        Waited::Matched => "A line matched.",
        Waited::Ended => "",
        Waited::TimedOut => "The wait ran out; it is still running.",
        Waited::Aborted => "The wait was stopped.",
    };

    Ok(report(handle, why))
}

async fn type_into(handle: &Handle, text: &str) -> Result<Output, ToolError> {
    if !handle.input(text).await {
        return Err(ToolError(format!(
            "{} takes no input: it has ended, or it was started in the foreground and moved to the background",
            handle.id
        )));
    }

    Ok(described(handle, format!("Typed into {}.", handle.id)))
}

fn interrupt(handle: &Handle) -> Output {
    let said = if handle.interrupt() {
        format!("Sent Ctrl+C to {}.", handle.id)
    } else {
        format!(
            "{} cannot be sent Ctrl+C on Windows, so it is being stopped instead.",
            handle.id
        )
    };

    described(handle, said)
}

async fn stop(context: &Context, handle: &Handle) -> Output {
    if handle.exit().is_none() {
        handle.stop();
        handle.wait(None, Duration::from_secs(5), &context.abort).await;
    }
    handle.claim_report();

    described(handle, format!("{} {}.", handle.id, state(handle)))
}

/// How the process is, then what it printed since the model last looked.
fn report(handle: &Handle, why: &str) -> Output {
    let unread = handle.unread();
    let mut text = status(handle);
    if !why.is_empty() {
        text = format!("{why} {text}");
    }

    if unread.skipped > 0 {
        let log = handle.record().log;
        let _ = write!(
            text,
            "

({} earlier bytes of new output are in {log}.)",
            unread.skipped
        );
    }
    let printed = unread.text.trim_end();
    text.push_str(if printed.is_empty() {
        "\n\nNothing new printed."
    } else {
        "\n\n"
    });
    text.push_str(printed);

    described(handle, text)
}

/// A line saying whether it runs or how it ended; saying it ended means the conversation is not told again.
fn status(handle: &Handle) -> String {
    if handle.exit().is_some() {
        handle.claim_report();
    }
    let url = handle
        .record()
        .url
        .map(|url| format!(" It printed {url}."))
        .unwrap_or_default();

    format!("{} {}.{url}", handle.id, state(handle))
}

fn state(handle: &Handle) -> String {
    let record = handle.record();
    let now = record.ended_at.unwrap_or_else(crate::id::now_ms);
    let elapsed = elapsed((now - record.started_at).max(0));

    match (record.state, handle.exit()) {
        (ProcessState::Running, _) => format!("is running ({elapsed})"),
        (_, Some(Exit::Code(code))) => format!("exited with code {code} after {elapsed}"),
        _ => format!("was stopped after {elapsed}"),
    }
}

fn elapsed(milliseconds: i64) -> String {
    let seconds = milliseconds / 1000;
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3600 => format!("{} min {} s", seconds / 60, seconds % 60),
        _ => format!("{} h {} min", seconds / 3600, seconds % 3600 / 60),
    }
}

fn described(handle: &Handle, text: String) -> Output {
    Output {
        title: handle.description.clone(),
        output: text,
        metadata: ToolMetadata {
            process_id: Some(handle.id.clone()),
            state: Some(format!("{:?}", handle.record().state).to_lowercase()),
            ..ToolMetadata::default()
        },
    }
}

#[cfg(test)]
mod tests;
