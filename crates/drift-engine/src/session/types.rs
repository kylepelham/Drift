//! What a conversation is made of. These shapes cross the API, so changes here change the client.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

// One field list keeps storage, merging and the API schema in agreement.
macro_rules! metadata_fields {
    ($($field:ident: $ty:ty => $key:literal),* $(,)?) => {
        #[derive(Clone, Debug, Default, PartialEq, Deserialize, ToSchema)]
        #[serde(from = "Value")]
        pub struct ToolMetadata {
            $(
                #[serde(rename = $key, skip_serializing_if = "Option::is_none")]
                #[schema(rename = $key)]
                pub $field: Option<$ty>,
            )*
            #[serde(flatten)]
            pub extra: serde_json::Map<String, Value>,
            #[serde(skip)]
            pub legacy: Option<Value>,
        }

        impl From<Value> for ToolMetadata {
            fn from(value: Value) -> Self {
                let Value::Object(mut fields) = value else {
                    return Self { legacy: Some(value), ..Self::default() };
                };

                Self {
                    $($field: metadata_field(&mut fields, $key),)*
                    extra: fields,
                    legacy: None,
                }
            }
        }

        impl Serialize for ToolMetadata {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeMap;

                if let Some(value) = &self.legacy {
                    return value.serialize(serializer);
                }

                let mut map = serializer.serialize_map(None)?;
                $(serialize_metadata_field(&mut map, $key, &self.$field)?;)*
                let typed = [$(($key, self.$field.is_some()),)*];
                for (key, value) in &self.extra {
                    if !typed.iter().any(|(name, present)| name == key && *present) {
                        map.serialize_entry(key, value)?;
                    }
                }

                map.end()
            }
        }

        impl ToolMetadata {
            pub fn merged(mut self, extra: Option<Self>) -> Option<Self> {
                if self.is_null() {
                    return extra;
                }
                if self.legacy.is_some() {
                    return Some(self);
                }

                if let Some(mut extra) = extra.filter(|extra| extra.legacy.is_none()) {
                    $(replace_metadata_field(&mut self.$field, &mut self.extra, &mut extra.$field, &extra.extra, $key);)*
                    self.extra.extend(extra.extra);
                }

                Some(self)
            }
        }
    };
}

metadata_fields! {
    engine_command: String => "engineCommand",
    command_model: String => "commandModel",
    shell_timeout_ms: Option<u64> => "shellTimeoutMs",
    output_bytes: u64 => "outputBytes",
    output_file: String => "outputFile",
    result_file: String => "resultFile",
    output: String => "output",
    exit: i64 => "exit",
    timed_out: bool => "timedOut",
    stopped: bool => "stopped",
    notes: Vec<String> => "notes",
    files: Vec<MetadataFile> => "files",
    diff: String => "diff",
    file_changes: Vec<ToolFileChange> => "fileChanges",
    replacements: usize => "replacements",
    created: bool => "created",
    changes: Vec<HistoryChange> => "changes",
    owner: String => "owner",
    at: String => "at",
    history_error: String => "historyError",
    unrecorded: Vec<String> => "unrecorded",
    checks: Vec<ToolCheck> => "checks",
    check_changed: Vec<String> => "checkChanged",
    check_observed: Vec<String> => "checkObserved",
    formatted: Vec<String> => "formatted",
    diagnostics: Vec<ToolDiagnostic> => "diagnostics",
    images: Vec<ToolImage> => "images",
    lines: Option<usize> => "lines",
    shown: usize => "shown",
    large: bool => "large",
    count: usize => "count",
    total: usize => "total",
    truncated: bool => "truncated",
    capped: bool => "capped",
    withheld: usize => "withheld",
    restricted: usize => "restricted",
    path: String => "path",
    open: usize => "open",
    redirect: String => "redirect",
    content_type: String => "contentType",
    bytes: usize => "bytes",
    server: String => "server",
    uri: String => "uri",
    request_id: String => "requestId",
    asynchronous: bool => "async",
    answers: Vec<Vec<String>> => "answers",
    session_id: String => "sessionId",
    task_id: String => "taskId",
    agent: String => "agent",
    outcome: String => "outcome",
    mode: String => "mode",
    reason: String => "reason",
    delivers: String => "delivers",
    state: String => "state",
    running: bool => "running",
}

fn metadata_field<T: serde::de::DeserializeOwned + Serialize>(
    fields: &mut serde_json::Map<String, Value>,
    key: &str,
) -> Option<T> {
    let value = fields.get(key)?;
    let parsed: T = serde_json::from_value(value.clone()).ok()?;
    // Imported fields can have nulls or missing nested fields that a native type would normalise.
    if serde_json::to_value(&parsed).ok().as_ref() != Some(value) {
        return None;
    }

    fields.remove(key);

    Some(parsed)
}

fn serialize_metadata_field<M: serde::ser::SerializeMap, T: Serialize>(
    map: &mut M,
    key: &str,
    field: &Option<T>,
) -> Result<(), M::Error> {
    if let Some(value) = field {
        map.serialize_entry(key, value)?;
    }

    Ok(())
}

fn replace_metadata_field<T>(
    field: &mut Option<T>,
    fields: &mut serde_json::Map<String, Value>,
    replacement: &mut Option<T>,
    extra: &serde_json::Map<String, Value>,
    key: &str,
) {
    if replacement.is_some() {
        *field = replacement.take();
        fields.remove(key);
    } else if extra.contains_key(key) {
        *field = None;
    }
}

impl ToolMetadata {
    pub fn null() -> Self {
        Self {
            legacy: Some(Value::Null),
            ..Self::default()
        }
    }

    pub fn is_null(&self) -> bool {
        self.legacy.as_ref().is_some_and(Value::is_null)
    }

    pub fn file_paths(&self) -> impl Iterator<Item = &str> {
        self.files.iter().flatten().filter_map(|file| match file {
            MetadataFile::Path(path) => Some(path.as_str()),
            MetadataFile::Imported(_) => None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum MetadataFile {
    Path(String),
    Imported(serde_json::Map<String, Value>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolFileChange {
    pub file_path: String,
    pub relative_path: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub patch: String,
    pub additions: usize,
    pub deletions: usize,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HistoryChange {
    pub path: String,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub before: Option<Option<String>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub after: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed: Option<bool>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(deserializer: D) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

impl From<super::snapshot::FileChange> for HistoryChange {
    fn from(change: super::snapshot::FileChange) -> Self {
        Self {
            path: change.path,
            before: Some(change.before),
            after: Some(change.after),
            observed: change.observed.then_some(true),
            extra: Default::default(),
        }
    }
}

impl HistoryChange {
    pub fn snapshot(&self) -> super::snapshot::FileChange {
        super::snapshot::FileChange {
            path: self.path.clone(),
            before: self.before.clone().flatten(),
            after: self.after.clone().flatten(),
            observed: self.observed.unwrap_or(false),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ToolCheck {
    pub check: String,
    pub status: CheckStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Passed,
    Problems,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ToolDiagnostic {
    pub file: String,
    pub server: String,
    pub line: u32,
    pub column: u32,
    pub message: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ToolImage {
    pub mime: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// A subagent's working session: not listed, its result flows back to the parent.
    Hidden,
    /// A spawned thread: listed beside its parent in the sidebar.
    Sibling,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub workspace_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub visibility: Visibility,
    pub title: String,
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    /// The reasoning variant the user last chose; turns the engine starts itself run with it too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<i64>,
    /// For a spawned thread, the last source message copied into it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_cutoff: Option<String>,
    /// Set while the user has undone the conversation back to a message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revert: Option<Revert>,
    /// Answers its own asks, and its subagents', except for secrets and anything outside the workspace.
    #[serde(default)]
    pub auto_accept: bool,
    /// Whether a turn is in flight right now; set by the API, never stored.
    #[serde(default)]
    pub running: bool,
}

/// An undo in progress: the user message it went back to, hidden with everything after it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Revert {
    pub message_id: String,
    /// Files the last undo or redo left alone because someone changed them after the session did.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<String>,
    /// Where the files stand when an undo kept them; absent means put back to `message_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<FilesAt>,
}

/// The files of an undo that did not put them back to its own point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum FilesAt {
    /// As the whole conversation left them.
    Current,
    /// Put back to before this prompt.
    Before(String),
}

impl Revert {
    pub fn new(message_id: &str, kept: Vec<String>, files_from: Option<&str>) -> Self {
        let files = match files_from {
            Some(from) if from == message_id => None,
            Some(from) => Some(FilesAt::Before(from.into())),
            None => Some(FilesAt::Current),
        };
        Self {
            message_id: message_id.into(),
            kept,
            files,
        }
    }

    /// The prompt whose turns and later ones are undone on disk; none when the files are as the conversation left them.
    pub fn files_from(&self) -> Option<&str> {
        match &self.files {
            None => Some(&self.message_id),
            Some(FilesAt::Current) => None,
            Some(FilesAt::Before(from)) => Some(from),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    Streaming,
    Done,
    Aborted,
    Error,
    /// No reply: the turn paused itself before this step (a limit it reached); `error` says why.
    Paused,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    /// Providers report running totals, so a later report supersedes an earlier one field by field.
    pub fn merge(&mut self, other: Usage) {
        self.input = self.input.max(other.input);
        self.output = self.output.max(other.output);
        self.cache_read = self.cache_read.max(other.cache_read);
        self.cache_write = self.cache_write.max(other.cache_write);
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: Role,
    pub status: MessageStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    /// The agent the session ran as when this was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub usage: Usage,
    pub cost: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    /// A compaction summary: from here on the model sees this instead of the history before it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub summary: bool,
    /// How a `done` reply ended when not on its own; `error` then says it in words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ending: Option<Ending>,
}

/// A finished reply that did not end by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Ending {
    /// It stopped at its output limit.
    Length,
    /// The provider's safety filter ended it.
    Refused,
    /// The turn's step or repeat limit stopped it; the reply is a write-up with tools off.
    Limit,
}

impl Ending {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Length => "length",
            Self::Refused => "refused",
            Self::Limit => "limit",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "length" => Some(Self::Length),
            "refused" => Some(Self::Refused),
            "limit" => Some(Self::Limit),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Pending,
    Running,
    Done,
    Error,
    Denied,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Part {
    Text {
        text: String,
    },
    /// Provider fields (`signature`, `redacted`) round-trip so a continued turn stays valid.
    Reasoning {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        redacted: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ToolCall {
        call_id: String,
        name: String,
        input: Value,
        status: ToolStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Box<ToolMetadata>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        started_at: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        finished_at: Option<i64>,
    },
    File {
        mime: String,
        name: String,
        /// Data URL for now; a content-addressed blob store replaces this later.
        url: String,
        /// For an `@` mention, the workspace file it was read from, so a client can open it; only the engine sets it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    /// A background worker's result, put into its parent's conversation by the engine, not typed by the user.
    #[serde(rename_all = "camelCase")]
    TaskResult {
        task_id: String,
        /// The worker's own transcript; not `session_id`, which the part row already carries.
        worker_session_id: String,
        description: String,
        /// `replied`, `failed`, `stopped` or `interrupted`.
        outcome: String,
        text: String,
    },
    /// The user's answer to a question the model asked without waiting, delivered as its own prompt.
    #[serde(rename_all = "camelCase")]
    Clarification {
        request_id: String,
        items: Vec<Clarified>,
    },
    /// A prompt the engine wrote to keep an orchestrator working toward the user's goal.
    Nudge {
        text: String,
    },
    /// What a plugin added for the model: context beside the user's prompt, or a prompt of its own that kept a turn going.
    Context {
        plugin: String,
        text: String,
    },
    /// The boundary of a compaction; its summary is the assistant message that follows.
    #[serde(rename_all = "camelCase")]
    Compaction {
        auto: bool,
        /// First message the model still sees verbatim after the summary; `None` keeps nothing.
        #[serde(skip_serializing_if = "Option::is_none")]
        tail_from: Option<String>,
    },
    /// A stored part this build cannot read (imported, or written by a newer Drift), kept exactly as stored.
    Unknown {
        raw: String,
    },
}

/// One answered question: what was asked and what the user chose or typed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Clarified {
    pub header: String,
    pub question: String,
    pub answers: Vec<String>,
}

impl Part {
    /// Parts only the engine writes: a prompt sent through the API may carry text and files, nothing else.
    pub fn is_engine_origin(&self) -> bool {
        !matches!(self, Self::Text { .. } | Self::File { .. })
    }

    /// The part a stored row holds; one that does not parse is kept as [`Part::Unknown`], never an error.
    pub fn from_stored(json: &str) -> Self {
        match serde_json::from_str(json) {
            Ok(Self::Unknown { .. }) | Err(_) => Self::Unknown { raw: json.into() },
            Ok(part) => part,
        }
    }

    /// The text to store: an unknown part goes back byte for byte.
    pub fn stored(&self) -> String {
        match self {
            Self::Unknown { raw } => raw.clone(),
            part => serde_json::to_string(part).unwrap(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Todo {
    pub content: String,
    pub status: TodoStatus,
    #[serde(default = "default_priority")]
    pub priority: String,
}

fn default_priority() -> String {
    "medium".into()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PartRow {
    pub id: String,
    pub message_id: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_signature: Option<String>,
    #[serde(flatten)]
    pub part: Part,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MessageWithParts {
    #[serde(flatten)]
    pub info: Message,
    pub parts: Vec<PartRow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const METADATA_SAMPLES: &[(&str, &str)] = &[
        (
            "bash",
            r#"{
            "shellTimeoutMs": 120000, "outputBytes": 42, "outputFile": "C:/work/command.log",
            "exit": 3, "notes": ["exit code 3"]
        }"#,
        ),
        (
            "unlimited bash",
            r#"{"shellTimeoutMs": null, "outputBytes": 0, "stopped": true}"#,
        ),
        (
            "running bash",
            r#"{"shellTimeoutMs": 400, "output": "Building application", "timedOut": true}"#,
        ),
        (
            "edit",
            r#"{
            "replacements": 1, "files": ["C:/work/main.rs"], "diff": "-old\n+new",
            "fileChanges": [{"filePath": "C:/work/main.rs", "relativePath": "main.rs",
                "type": "update", "patch": "-old\n+new", "additions": 1, "deletions": 1}]
        }"#,
        ),
        (
            "write",
            r#"{
            "created": false, "files": ["C:/work/main.rs"], "diff": "-old\n+new",
            "fileChanges": [{"filePath": "C:/work/main.rs", "relativePath": "main.rs",
                "type": "add", "patch": "+new", "additions": 1, "deletions": 0}]
        }"#,
        ),
        (
            "apply_patch",
            r#"{
            "files": ["C:/work/moved.rs"], "fileChanges": [
                {"filePath": "C:/work/moved.rs", "relativePath": "moved.rs", "type": "move",
                    "patch": "-old\n+new", "additions": 1, "deletions": 1},
                {"filePath": "C:/work/removed.rs", "relativePath": "removed.rs", "type": "delete",
                    "patch": "-old", "additions": 0, "deletions": 1}]
        }"#,
        ),
        ("read", r#"{"lines": 20, "shown": 10}"#),
        ("large read", r#"{"lines": null, "shown": 100, "large": true}"#),
        (
            "grep",
            r#"{
            "count": 200, "total": 2000, "capped": true, "truncated": true, "withheld": 1, "restricted": 2
        }"#,
        ),
        ("glob", r#"{"count": 1, "total": 1, "truncated": false}"#),
        ("webfetch", r#"{"contentType": "text/html", "bytes": 256}"#),
        ("webfetch redirect", r#"{"redirect": "https://example.org/document"}"#),
        (
            "task",
            r#"{
            "sessionId": "ses_worker", "taskId": "task_review", "agent": "explore", "outcome": "replied",
            "mode": "foreground", "delivers": "task_review"
        }"#,
        ),
        (
            "background task",
            r#"{
            "sessionId": "ses_worker", "taskId": "task_review", "agent": "explore", "outcome": "launched",
            "mode": "background", "reason": "explicit"
        }"#,
        ),
        (
            "task_output",
            r#"{"sessionId": "ses_worker", "taskId": "task_review", "state": "running"}"#,
        ),
        ("read_thread", r#"{"sessionId": "ses_worker", "running": false}"#),
        ("question", r#"{"requestId": "qst_database", "async": true}"#),
        (
            "question answers",
            r#"{"answers": [["SQLite"], ["Keep existing tables"]]}"#,
        ),
        ("mcp", r#"{"server": "documents", "uri": "file:///guide.pdf"}"#),
        (
            "checks/history",
            r#"{
            "changes": [{"path": "main.rs", "before": null, "after": "blob_new"},
                {"path": "notes.txt", "before": "blob_old", "after": null, "observed": true}],
            "owner": "ws_project", "at": "chg_written", "unrecorded": ["large.log"],
            "checks": [{"check": "lint", "status": "passed"},
                {"check": "types", "status": "problems", "output": "Missing type annotation"},
                {"check": "build", "status": "unavailable", "output": "Compiler unavailable"}],
            "checkChanged": ["main.rs"], "checkObserved": ["notes.txt"], "formatted": ["rustfmt: main.rs"],
            "diagnostics": [{"file": "main.rs", "server": "rust-analyzer", "line": 3, "column": 1,
                "message": "Missing type annotation"}], "notes": ["A formatter changed main.rs"]
        }"#,
        ),
        (
            "history failure",
            r#"{
            "changes": [], "owner": "ws_project", "unrecorded": [], "historyError": "History unavailable"
        }"#,
        ),
        (
            "returned images",
            r#"{"images": [{"mime": "image/png", "data": "iVBORw=="}]}"#,
        ),
        (
            "stored images",
            r#"{"images": [{"mime": "application/pdf", "hash": "abc123"}]}"#,
        ),
        (
            "skill",
            r#"{"path": "C:/skills/review", "files": ["C:/skills/review/guide.md"]}"#,
        ),
        ("todowrite", r#"{"count": 3, "open": 1}"#),
        (
            "command",
            r#"{"engineCommand": "review", "commandModel": "anthropic/claude"}"#,
        ),
        ("spilled result", r#"{"resultFile": "C:/work/result.log"}"#),
    ];

    #[test]
    fn tool_metadata_producers_round_trip_without_added_defaults() {
        for (producer, sample) in METADATA_SAMPLES {
            let json: Value = serde_json::from_str(sample).unwrap();
            let metadata: ToolMetadata = serde_json::from_value(json.clone()).unwrap();

            assert!(metadata.extra.is_empty(), "{producer}: native fields must be typed");
            assert_eq!(serde_json::to_value(&metadata).unwrap(), json, "{producer}");
        }

        assert_eq!(
            serde_json::to_value(ToolMetadata::default()).unwrap(),
            serde_json::json!({})
        );
    }

    #[test]
    fn tool_metadata_samples_round_trip_in_stored_parts() {
        for (producer, sample) in METADATA_SAMPLES {
            let metadata: Value = serde_json::from_str(sample).unwrap();
            let stored = serde_json::json!({
                "type": "tool_call", "callId": "call_saved", "name": producer, "input": {},
                "status": "done", "title": "Saved result", "output": "Complete", "metadata": metadata,
                "startedAt": 1000, "finishedAt": 2000
            });

            let part = Part::from_stored(&stored.to_string());
            assert!(matches!(part, Part::ToolCall { metadata: Some(_), .. }), "{producer}");
            assert_eq!(
                serde_json::from_str::<Value>(&part.stored()).unwrap(),
                stored,
                "{producer}"
            );
        }
    }

    #[test]
    fn tool_metadata_schema_declares_the_producers_wire_keys() {
        use utoipa::PartialSchema;

        let schema = serde_json::to_value(ToolMetadata::schema()).unwrap();
        let properties = schema["properties"].as_object().unwrap();

        for (producer, sample) in METADATA_SAMPLES {
            let metadata: Value = serde_json::from_str(sample).unwrap();
            for key in metadata.as_object().unwrap().keys() {
                assert!(properties.contains_key(key), "{producer}: schema lacks {key}");
            }
        }

        assert!(!properties.contains_key("legacy"));
        assert!(!properties.contains_key("extra"));
        assert!(!properties.contains_key("engine_command"));
        assert!(schema.get("additionalProperties").is_some());
    }

    #[test]
    fn typed_metadata_overwrites_legacy_keys_without_duplicate_json_fields() {
        let mut metadata = ToolMetadata::from(serde_json::json!({"exit": "unavailable", "notes": null}));
        metadata.exit = Some(0);
        metadata.notes = Some(Vec::new());

        let json = serde_json::to_string(&metadata).unwrap();
        assert_eq!(json.matches("\"exit\"").count(), 1);
        assert_eq!(json.matches("\"notes\"").count(), 1);
        assert_eq!(
            serde_json::from_str::<Value>(&json).unwrap(),
            serde_json::json!({"exit": 0, "notes": []})
        );

        let output = crate::tool::Output::new("Read directory", "No files");
        assert!(serde_json::to_value(output).unwrap().get("metadata").is_none());
    }

    #[test]
    fn imported_metadata_and_non_objects_stay_loadable() {
        let samples = [
            serde_json::json!({
                "filediff": {"file": "C:/work/main.rs", "patch": "-old\n+new"},
                "files": [{"filePath": "C:/work/main.rs", "movePath": "C:/work/new.rs", "custom": 7}],
                "changes": [{"path": "main.rs"}], "at": "msg_imported", "future": {"version": 2}
            }),
            serde_json::json!({"notes": null, "exit": "unknown", "images": [{"mime": "image/png", "data": null}]}),
            serde_json::json!({"checks": [{"check": "lint", "status": "passed", "output": null}]}),
            serde_json::json!({"changes": [{"path": "main.rs", "observed": null}]}),
            serde_json::json!(["old metadata", 7]),
            serde_json::json!("old metadata"),
            serde_json::json!(42),
            serde_json::json!(false),
        ];

        for json in samples {
            let metadata: ToolMetadata = serde_json::from_value(json.clone()).unwrap();
            assert_eq!(serde_json::to_value(metadata).unwrap(), json);

            let stored = serde_json::json!({
                "type": "tool_call", "callId": "call_imported", "name": "read", "input": {},
                "status": "done", "metadata": json
            });
            let part = Part::from_stored(&stored.to_string());
            assert!(matches!(part, Part::ToolCall { .. }));
            assert_eq!(serde_json::from_str::<Value>(&part.stored()).unwrap(), stored);
        }

        let null: ToolMetadata = serde_json::from_value(Value::Null).unwrap();
        assert!(null.is_null());
        assert_eq!(serde_json::to_value(null).unwrap(), Value::Null);
    }

    #[test]
    fn typed_metadata_merges_with_the_same_json_precedence() {
        let base = ToolMetadata::from(serde_json::json!({"exit": "unknown", "notes": ["Earlier"], "future": 1}));
        let patch = ToolMetadata::from(serde_json::json!({"exit": 0, "notes": [], "future": 2}));
        let merged = base.merged(Some(patch)).unwrap();

        assert_eq!(
            serde_json::to_value(&merged).unwrap(),
            serde_json::json!({"exit": 0, "notes": [], "future": 2})
        );
        let merged = merged
            .merged(Some(serde_json::json!({"exit": "unavailable"}).into()))
            .unwrap();
        assert!(merged.exit.is_none());
        assert_eq!(serde_json::to_value(merged).unwrap()["exit"], "unavailable");

        assert!(ToolMetadata::null().merged(None).is_none());
        let legacy = ToolMetadata::from(serde_json::json!([1]));
        assert_eq!(legacy.clone().merged(Some(ToolMetadata::default())), Some(legacy));
    }

    #[test]
    fn part_serialises_tagged_and_flat_in_row() {
        let row = PartRow {
            id: "prt_1".into(),
            message_id: "msg_1".into(),
            session_id: "ses_1".into(),
            provider_signature: None,
            part: Part::ToolCall {
                call_id: "toolu_1".into(),
                name: "read".into(),
                input: serde_json::json!({ "path": "a.rs" }),
                status: ToolStatus::Pending,
                title: None,
                output: None,
                metadata: None,
                started_at: None,
                finished_at: None,
            },
        };
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["type"], "tool_call");
        assert_eq!(json["callId"], "toolu_1");
        assert_eq!(json["messageId"], "msg_1");
        assert!(json.get("output").is_none());
        let back: PartRow = serde_json::from_value(json).unwrap();
        assert_eq!(back, row);
    }

    #[test]
    fn a_part_never_shadows_its_rows_own_fields() {
        let row = PartRow {
            id: "prt_1".into(),
            message_id: "msg_1".into(),
            session_id: "ses_parent".into(),
            provider_signature: None,
            part: Part::TaskResult {
                task_id: "task_1".into(),
                worker_session_id: "ses_worker".into(),
                description: "d".into(),
                outcome: "replied".into(),
                text: "t".into(),
            },
        };
        let text = serde_json::to_string(&row).unwrap();
        assert_eq!(text.matches("\"sessionId\"").count(), 1, "{text}");
        let back: PartRow = serde_json::from_str(&text).unwrap();
        assert_eq!(back, row);
    }
}
