//! How a Drift tool call looks in the editor's tool call view: kind, title, locations and content.

use std::path::Path;

use drift_engine::permission::Request;
use drift_engine::session::types::{Part, PartRow, Todo, TodoStatus, ToolMetadata, ToolStatus};
use serde_json::{Value, json};

/// The call as a `tool_call` the first time the client hears of it, then as `tool_call_update`s.
pub(super) fn update(row: &PartRow, cwd: &Path, first: bool) -> Option<Value> {
    let Part::ToolCall {
        name,
        input,
        status,
        title,
        output,
        metadata,
        ..
    } = &row.part
    else {
        return None;
    };

    let mut update = json!({
        "sessionUpdate": if first { "tool_call" } else { "tool_call_update" },
        "toolCallId": row.id,
        "title": title_of(name, input, title.as_deref()),
        "kind": kind(name),
        "status": status_of(*status),
        "locations": locations(name, input, cwd),
    });
    if first {
        update["name"] = json!(name);
    }
    if input.is_object() {
        update["rawInput"] = input.clone();
    }
    let shown = Shown {
        name,
        input,
        output: output.as_deref(),
        metadata: metadata.as_deref(),
    };
    let content = shown.content(*status, cwd);
    if !content.is_empty() {
        update["content"] = Value::Array(content);
    }

    Some(update)
}

/// The call a permission ask is about, as `session/request_permission` names it.
pub(super) fn asked(request: &Request, tool_call_id: &str) -> Value {
    let mut call = json!({
        "toolCallId": tool_call_id,
        "title": request.ask.title,
        "kind": kind(&request.tool),
        "status": "pending",
    });
    if let Some(diff) = &request.ask.diff {
        call["content"] = json!([text(&format!("```diff\n{diff}\n```"))]);
    }

    call
}

/// What the user may answer; "always" only when answering it would allow something beyond this call.
pub(super) fn options(request: &Request) -> Value {
    let option = |id: &str, name: &str| json!({ "optionId": id, "name": name, "kind": id });
    let mut options = vec![option(ALLOW_ONCE, "Allow once")];
    if !request.always.is_empty() {
        options.push(option(ALLOW_ALWAYS, "Always allow"));
    }
    options.push(option(REJECT_ONCE, "Reject"));

    Value::Array(options)
}

pub(super) const ALLOW_ONCE: &str = "allow_once";
pub(super) const ALLOW_ALWAYS: &str = "allow_always";
pub(super) const REJECT_ONCE: &str = "reject_once";

/// The session's todo list as an agent plan; cancelled items are left out, since a plan has no such state.
pub(super) fn plan(todos: &[Todo]) -> Value {
    let entries: Vec<Value> = todos
        .iter()
        .filter_map(|todo| {
            let status = match todo.status {
                TodoStatus::Pending => "pending",
                TodoStatus::InProgress => "in_progress",
                TodoStatus::Completed => "completed",
                TodoStatus::Cancelled => return None,
            };
            let priority = match todo.priority.as_str() {
                "high" => "high",
                "low" => "low",
                _ => "medium",
            };
            Some(json!({ "content": todo.content, "priority": priority, "status": status }))
        })
        .collect();

    json!({ "sessionUpdate": "plan", "entries": entries })
}

fn kind(name: &str) -> &'static str {
    match name {
        "read" | "read_thread" | "skill" => "read",
        "write" | "edit" | "apply_patch" | "save_image" => "edit",
        "bash" | "process" => "execute",
        "glob" | "grep" | "websearch" => "search",
        "webfetch" => "fetch",
        "todowrite" | "task" | "task_output" | "task_stop" => "think",
        _ => "other",
    }
}

fn status_of(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Pending => "pending",
        ToolStatus::Running => "in_progress",
        ToolStatus::Done => "completed",
        ToolStatus::Error | ToolStatus::Denied => "failed",
    }
}

/// The engine's own title once the call has one, else the input field that says most.
fn title_of(name: &str, input: &Value, title: Option<&str>) -> String {
    if let Some(title) = title.filter(|title| !title.is_empty()) {
        return title.into();
    }

    let field = match name {
        "bash" => input["description"].as_str().or_else(|| input["command"].as_str()),
        "read" | "write" | "edit" => input["path"].as_str(),
        "glob" | "grep" => input["pattern"].as_str(),
        "webfetch" => input["url"].as_str(),
        "websearch" => input["query"].as_str(),
        "task" => input["description"].as_str(),
        _ => None,
    };

    field.unwrap_or(name).to_string()
}

/// The files the call touches, so an editor can follow along.
fn locations(name: &str, input: &Value, cwd: &Path) -> Vec<Value> {
    let paths: Vec<&str> = match name {
        "read" | "write" | "edit" | "glob" | "grep" => input["path"].as_str().into_iter().collect(),
        "apply_patch" => input["patch"].as_str().map(patched).unwrap_or_default(),
        _ => Vec::new(),
    };

    paths
        .into_iter()
        .map(|path| {
            let mut location = json!({ "path": absolute(cwd, path) });
            if name == "read"
                && let Some(line) = input["offset"].as_u64()
            {
                location["line"] = json!(line);
            }
            location
        })
        .collect()
}

/// The files a patch adds, updates, deletes or moves to.
fn patched(patch: &str) -> Vec<&str> {
    const HEADERS: [&str; 4] = [
        "*** Add File: ",
        "*** Update File: ",
        "*** Delete File: ",
        "*** Move to: ",
    ];

    patch
        .lines()
        .filter_map(|line| HEADERS.iter().find_map(|header| line.strip_prefix(header)))
        .map(str::trim)
        .collect()
}

fn absolute(cwd: &Path, path: &str) -> String {
    cwd.join(path).to_string_lossy().into_owned()
}

fn text(text: &str) -> Value {
    json!({ "type": "content", "content": { "type": "text", "text": text } })
}

/// A call's input and what it returned, for the content its view shows.
struct Shown<'a> {
    name: &'a str,
    input: &'a Value,
    output: Option<&'a str>,
    metadata: Option<&'a ToolMetadata>,
}

impl Shown<'_> {
    /// Live output while it runs, then an edit's diff or the call's result.
    fn content(&self, status: ToolStatus, cwd: &Path) -> Vec<Value> {
        let shown = match status {
            ToolStatus::Pending => None,
            ToolStatus::Running => self.metadata.and_then(|metadata| metadata.output.as_deref()).map(text),
            ToolStatus::Done => self.diff(cwd).or_else(|| self.output.map(text)),
            ToolStatus::Error | ToolStatus::Denied => self.output.map(text),
        };

        shown.into_iter().collect()
    }

    /// An edit's replacement, or a new file's whole text; an overwrite has no old text to show, so it shows its result.
    fn diff(&self, cwd: &Path) -> Option<Value> {
        let path = absolute(cwd, self.input["path"].as_str()?);
        let (old, new) = match self.name {
            "edit" => (
                json!(self.input["old_string"].as_str()?),
                self.input["new_string"].as_str()?,
            ),
            "write" if self.metadata.and_then(|metadata| metadata.created) == Some(true) => {
                (Value::Null, self.input["content"].as_str()?)
            }
            _ => return None,
        };

        Some(json!({ "type": "diff", "path": path, "oldText": old, "newText": new }))
    }
}
