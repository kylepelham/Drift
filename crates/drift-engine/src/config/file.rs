use crate::permission::Rule;
use crate::session::types::ModelRef;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::ToSchema;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct File {
    /// Default model for new sessions.
    pub model: Option<ModelRef>,
    /// The agent a session created without one runs as; a primary agent's name.
    pub default_agent: Option<String>,
    pub permissions: Vec<Rule>,
    /// Extra instruction files, relative to the file's directory.
    pub instructions: Vec<String>,
    /// Formatter overrides by name; `false` disables a built-in.
    pub formatters: BTreeMap<String, FormatterConfig>,
    /// Checks run after a write, by name; `false` turns off one an earlier file set.
    pub checks: BTreeMap<String, CheckConfig>,
    /// Language servers by name: `false` turns one off; a command adds or replaces one, from the user's own file only.
    pub lsp: BTreeMap<String, LspConfig>,
    /// Turn limits; each field set here replaces the one before it.
    pub limits: LimitsFile,
    /// Time limits per provider route (`ollama`, `anthropic`, ...), for slow local models or gateways.
    pub timeouts: BTreeMap<String, RouteTimeouts>,
    /// Providers added or re-pointed. Read from the user's own `~/.config/drift/drift.json` only: a
    /// project's file is committed by others and must never send your key somewhere else.
    pub providers: BTreeMap<String, ProviderConfig>,
    /// More folders to find skills in (`SKILL.md` at any depth), relative to the file's directory or starting `~/`.
    pub skill_paths: Vec<String>,
    /// WebAssembly plugins under this directory, each a path or a path with config; read from the user's own file only.
    pub plugins: Vec<crate::hook::PluginEntry>,
}

/// A provider the user adds (any OpenAI-compatible server) or re-points (a gateway, a remote LM Studio).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderConfig {
    pub name: Option<String>,
    /// Where its requests go: an OpenAI-compatible `/v1` root for a new or local provider.
    pub base_url: Option<String>,
    /// The environment variable holding its key. A new provider without one takes no key.
    pub api_key_env: Option<String>,
    pub models: BTreeMap<String, ProviderModel>,
}

/// A model of a user's provider, as the catalog needs it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderModel {
    pub name: Option<String>,
    /// Context window in tokens; 0 when unknown.
    pub context: u64,
    /// Longest reply in tokens; 0 when unknown.
    pub output: u64,
    pub images: bool,
}

/// A route's time limits in seconds; either may be left out to keep the route's default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct RouteTimeouts {
    /// Until the response begins.
    pub headers_seconds: Option<u64>,
    /// Between two pieces of a streamed reply.
    pub idle_seconds: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct LimitsFile {
    pub steps: Option<u32>,
    pub repeats: Option<u32>,
    pub polls: Option<u32>,
}

/// When a turn pauses for the user rather than carrying on by itself.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    /// Model steps (requests) one turn may take.
    pub steps: u32,
    /// Steps in a row whose calls and results are all identical: no progress, so likely a loop.
    pub repeats: u32,
    /// The same for steps whose shell commands deliberately wait, as polling does.
    pub polls: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum FormatterConfig {
    Enabled(bool),
    Custom {
        command: Vec<String>,
        extensions: Vec<String>,
    },
}

/// A check is a command over files with the given extensions; `$FILE` runs it once per written file, without it once per call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum CheckConfig {
    Enabled(bool),
    Custom {
        command: Vec<String>,
        extensions: Vec<String>,
    },
}

/// A language server Drift starts to hear the errors an edit left: `false` turns a built-in off; a
/// command over stdio and the extensions it handles add one (`language` names their LSP language id).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum LspConfig {
    Enabled(bool),
    Custom {
        command: Vec<String>,
        extensions: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        language: Option<String>,
    },
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            steps: 200,
            repeats: 3,
            polls: 30,
        }
    }
}
