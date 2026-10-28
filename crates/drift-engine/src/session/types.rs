//! What a conversation is made of. These shapes cross the API, so changes here change the client.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

mod metadata;

pub use metadata::{
    CheckStatus, FindingOutcome, FindingVerdict, HistoryChange, MetadataFile, ReviewFinding, ToolCheck, ToolDiagnostic,
    ToolFileChange, ToolImage, ToolMetadata,
};

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
    /// The Drift worktree it works in, by name, on branch `drift/<name>`; absent for the workspace folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    /// The provider account the user set this chat to use while it has usage left; its subagents and spawned threads
    /// start with it. Absent, the provider's account order decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
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
    /// Monotonic time from the first generated block to its last content event, excluding request and tool waits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_ms: Option<u64>,
    /// The stored account that sent the request, whose reasoning signatures only it can send back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// A compaction summary: from here on the model sees this instead of the history before it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub summary: bool,
    /// How a `done` reply ended when not on its own; `error` then says it in words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ending: Option<Ending>,
    /// What the provider said about a safety refusal, when it said anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<Refusal>,
    /// Why the request read much less of the prompt cache than the one before it left, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_miss: Option<String>,
}

/// The provider's account of a safety refusal. None of it names the text that triggered it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Refusal {
    /// The policy area, as the provider names it (Anthropic: `cyber`, `bio`, `general_harms`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    /// A model the provider suggests retrying on; Drift only offers it, never switches by itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// A finished reply that did not end by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Ending {
    /// It stopped at its output limit.
    Length,
    /// The provider's safety filter ended it.
    Refused,
    /// The repeat check stopped it; the reply is a write-up with tools off.
    Limit,
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
        /// The number the model knows a pasted image by (`[Image 2]`), counted across the conversation when it is
        /// attached; `save_image` writes it to a file by that number. Only the engine sets it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image: Option<u32>,
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
    /// How a background process ended, put into its conversation by the engine so the model can carry on.
    #[serde(rename_all = "camelCase")]
    ProcessExit {
        process_id: String,
        command: String,
        description: String,
        /// `None` when it was stopped rather than exiting.
        #[serde(skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        /// The end of what it printed.
        output: String,
        /// Where everything it printed is.
        log: String,
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
    /// What changed in the system prompt's sources since the model last heard them, written by the engine
    /// before the request it precedes and replayed as saved, so the system prompt itself never changes.
    InstructionUpdate {
        /// The baseline it was made against; after a new one it is history, never replayed.
        generation: String,
        changes: Vec<InstructionChange>,
    },
    /// The engine's word to keep going after a reply stopped at its output limit; replayed after that reply.
    Continuation {
        text: String,
    },
    /// The reasoning level the user switched to, sent as a per-message effort on models that take one
    /// (`Model::effort_updates`), so the request's top-level effort and its cache stay as they were.
    EffortUpdate {
        generation: String,
        effort: String,
    },
    /// The slash command that wrote the prompt beside it, for the transcript to show; the model reads only the prompt.
    Command {
        name: String,
        arguments: String,
    },
    /// A stored part this build cannot read (imported, or written by a newer Drift), kept exactly as stored.
    Unknown {
        raw: String,
    },
}

/// One source of the system prompt that changed: its new text, or `None` when it no longer applies.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct InstructionChange {
    pub key: String,
    /// How the user and the model hear it named.
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// One answered question: what was asked and what the user chose or typed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Clarified {
    pub header: String,
    pub question: String,
    pub answers: Vec<String>,
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

fn default_priority() -> String {
    "medium".into()
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

impl Usage {
    /// Providers report running totals, so a later report supersedes an earlier one field by field.
    pub fn merge(&mut self, other: Usage) {
        self.input = self.input.max(other.input);
        self.output = self.output.max(other.output);
        self.cache_read = self.cache_read.max(other.cache_read);
        self.cache_write = self.cache_write.max(other.cache_write);
    }
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

#[cfg(test)]
mod tests;
