use crate::permission::Rule;
use crate::session::types::ModelRef;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub name: String,
    pub description: String,
    /// A subagent's goes in its system prompt; a primary agent's rides on the prompts of the turns it
    /// runs, so switching agents mid-conversation keeps the cached prefix.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    /// Tool names this agent may use, any case; `!name` takes one away. Empty, or only `!` entries, means every other tool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    pub builtin: bool,
    #[serde(default)]
    pub kind: AgentKind,
    /// Front matter `steps:`: this agent's own step limit, in place of the workspace's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,
    /// Front matter `background: true|false`: how a `task` for this agent runs when the call does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    /// Front matter `read_only: true`: it is offered its tools as usual, but any call that would change
    /// something (a writing tool, a shell line that is not only reads, a task to a writing subagent) is refused.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<Rule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// Why this agent cannot run (a broken file or override); only its own turns, tasks and actions are refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    /// Front matter `hidden: true`: left out of the composer's list; `task` can still run it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum AgentError {
    #[error("agent {name}: {problem}")]
    Unusable { name: String, problem: String },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    /// Picked in the composer to run a conversation; can also take a `task`.
    #[default]
    Primary,
    /// Only runs `task` subagents; never offered in the composer.
    Subagent,
    /// Both: picked in the composer, and offered to the model for delegation. A user's agent with no `mode` is this, as in opencode.
    All,
    /// Does one engine job (titles, compaction); never runs a conversation.
    Action,
}

impl Agent {
    /// The agent, unless its definition is broken.
    pub fn usable(&self) -> Result<&Self, AgentError> {
        match &self.problem {
            Some(problem) => Err(AgentError::Unusable {
                name: self.name.clone(),
                problem: problem.clone(),
            }),
            None => Ok(self),
        }
    }

    /// Whether `tool` is one this agent may be offered. Names match in any case, as Claude-style files write them; `*` is every tool.
    pub fn allows_tool(&self, tool: &str) -> bool {
        let (taken, given): (Vec<&String>, Vec<&String>) = self.tools.iter().partition(|name| name.starts_with('!'));
        let excluded = taken
            .iter()
            .any(|name| &name[1..] == "*" || name[1..].eq_ignore_ascii_case(tool));
        if excluded {
            return false;
        }

        given.is_empty() || given.iter().any(|name| *name == "*" || name.eq_ignore_ascii_case(tool))
    }
}

impl AgentKind {
    /// Front matter `mode`: `primary`, `subagent` or `all`; anything else, or none, is `all`.
    pub(super) fn from_mode(mode: Option<&str>) -> Self {
        match mode.map(str::trim) {
            Some("primary") => Self::Primary,
            Some("subagent") => Self::Subagent,
            _ => Self::All,
        }
    }

    /// Can run a conversation picked in the composer.
    pub fn runs_conversations(self) -> bool {
        matches!(self, Self::Primary | Self::All)
    }

    /// Offered to the model by name for `task`.
    pub fn delegated_to(self) -> bool {
        matches!(self, Self::Subagent | Self::All)
    }
}
