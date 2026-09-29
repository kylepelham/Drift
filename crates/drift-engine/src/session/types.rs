//! What a conversation is made of. These shapes cross the API, so changes here change the client.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

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
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<i64>,
    /// Whether a turn is in flight right now; set by the API, never stored.
    #[serde(default)]
    pub running: bool,
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
    pub usage: Usage,
    pub cost: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
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
        metadata: Option<Value>,
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
    },
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

    #[test]
    fn part_serialises_tagged_and_flat_in_row() {
        let row = PartRow {
            id: "prt_1".into(),
            message_id: "msg_1".into(),
            session_id: "ses_1".into(),
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
}
