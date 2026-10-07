//! What a workspace tells the engine: drift.json, agents, commands, skills and instruction files.

mod arguments;
mod frontmatter;
pub mod jsonc;
mod overrides;

pub use overrides::{AgentOverride, ModelPin};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::permission::{Policy, Rule};
use crate::session::types::ModelRef;

pub const FILE: &str = "drift.json";
const DIR: &str = ".drift";
const INSTRUCTION_FILES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];
/// Skill roots looked up under the workspace and the home directory, in this order.
/// Skill folders in a project directory, and (without the dot for Drift's own) in the home directory.
const SKILL_DIRS: [&str; 3] = [".drift/skills", ".agents/skills", ".claude/skills"];
const HOME_SKILL_DIRS: [&str; 3] = [".config/drift/skills", ".agents/skills", ".claude/skills"];
/// How deep under a skill folder a `SKILL.md` is looked for; folders such as `node_modules` are never entered.
const SKILL_DEPTH: usize = 6;
const MAX_INSTRUCTION_CHARS: usize = 40_000;

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
    /// WebAssembly plugins (`.wasm` components under this directory), each a path or a path with
    /// its config, from the user's own file only: opening a project must never run code it ships.
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

/// The providers the user's own drift.json adds or re-points.
pub fn user_providers() -> BTreeMap<String, ProviderConfig> {
    home().map(|home| user_providers_in(&home)).unwrap_or_default()
}

fn user_providers_in(home: &Path) -> BTreeMap<String, ProviderConfig> {
    user_file(home).map(|file| file.providers).unwrap_or_default()
}

fn user_file(home: &Path) -> Option<File> {
    let text = std::fs::read_to_string(home.join(".config/drift").join(FILE)).ok()?;
    serde_json::from_str::<File>(&jsonc::strip(&text)).ok()
}

/// The plugins the user's own drift.json lists; an entry that leaves the directory or is not a `.wasm` carries the error instead of a path.
pub fn user_plugins() -> Vec<crate::hook::Listed> {
    home().map(|home| user_plugins_in(&home)).unwrap_or_default()
}

fn user_plugins_in(home: &Path) -> Vec<crate::hook::Listed> {
    let root = home.join(".config/drift");
    user_file(home)
        .map(|file| file.plugins)
        .unwrap_or_default()
        .into_iter()
        .map(|entry| crate::hook::Listed { entry: entry.path().to_owned(), path: plugin_path(&root, entry.path()), config: entry.config() })
        .collect()
}

fn plugin_path(root: &Path, entry: &str) -> Result<PathBuf, String> {
    let relative = Path::new(entry);
    if relative.components().any(|part| !matches!(part, std::path::Component::Normal(_))) {
        return Err("a plugin path must be relative and stay under the config directory".into());
    }
    if relative.extension().is_none_or(|extension| !extension.eq_ignore_ascii_case("wasm")) {
        return Err("a plugin is a .wasm component".into());
    }
    Ok(root.join(relative))
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

impl Default for Limits {
    fn default() -> Self {
        Self { steps: 200, repeats: 3, polls: 30 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum FormatterConfig {
    Enabled(bool),
    Custom { command: Vec<String>, extensions: Vec<String> },
}

/// A check is a command over files with the given extensions; `$FILE` runs it once per written file, without it once per call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum CheckConfig {
    Enabled(bool),
    Custom { command: Vec<String>, extensions: Vec<String> },
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

impl Agent {
    /// The agent, unless its definition is broken.
    pub fn usable(&self) -> Result<&Self, String> {
        match &self.problem {
            Some(problem) => Err(format!("agent {}: {problem}", self.name)),
            None => Ok(self),
        }
    }

    /// Whether `tool` is one this agent may be offered. Names match in any case, as Claude-style files write them; `*` is every tool.
    pub fn allows_tool(&self, tool: &str) -> bool {
        let (taken, given): (Vec<&String>, Vec<&String>) = self.tools.iter().partition(|name| name.starts_with('!'));
        if taken.iter().any(|name| &name[1..] == "*" || name[1..].eq_ignore_ascii_case(tool)) {
            return false;
        }
        given.is_empty() || given.iter().any(|name| *name == "*" || name.eq_ignore_ascii_case(tool))
    }
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

impl AgentKind {
    /// Front matter `mode`: `primary`, `subagent` or `all`; anything else, or none, is `all`.
    fn from_mode(mode: Option<&str>) -> Self {
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Command {
    pub name: String,
    pub description: String,
    /// The prompt; see [`Command::expand`] for how what follows the command fills it.
    pub template: String,
    /// For an MCP server's prompt (`server:prompt`): the server that fills it; the template is unused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// The prompt's arguments in order, which what follows the command fills word by word.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtask: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
    /// How to call it, from its skill's `argument-hint` (`[audit|polish] [target]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<String>,
    /// The choices its skill documents, which the slash menu offers (`config::arguments`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subcommands: Vec<arguments::Subcommand>,
}

impl Command {
    pub fn new(name: String, description: String, template: String) -> Self {
        Self { name, description, template, server: None, arguments: Vec::new(), agent: None, model: None, subtask: None, skill: None, usage: None, subcommands: Vec::new() }
    }

    /// Takes the usage and choices `skill` documents, keeping its own template and settings.
    fn document(&mut self, skill: &Skill) {
        (self.usage, self.subcommands) = arguments::skill_arguments(&skill.name, &skill.instructions, skill.argument_hint.as_deref());
    }
    /// `arguments` split for this command's named arguments: one word each, the last taking the rest.
    pub fn named_arguments(&self, arguments: &str) -> serde_json::Map<String, serde_json::Value> {
        let words = split_arguments(arguments);
        let last = self.arguments.len().saturating_sub(1);
        self.arguments
            .iter()
            .enumerate()
            .filter_map(|(i, name)| {
                let value = if i == last { words.get(i..).map(|rest| rest.join(" ")) } else { words.get(i).map(|w| w.to_string()) };
                value.filter(|v| !v.is_empty()).map(|v| (name.clone(), serde_json::Value::String(v)))
            })
            .collect()
    }

    /// The prompt for `arguments`: `$ARGUMENTS` is all of them, `$1`, `$2`, ... one argument each (quotes
    /// keep spaces, as in a shell) with the highest taking the rest, and a template that names none gets
    /// them appended so nothing typed is lost.
    pub fn expand(&self, arguments: &str) -> String {
        let arguments = arguments.trim();
        let words = split_arguments(arguments);
        let highest = highest_placeholder(&self.template);
        let mut text = self.template.replace("$ARGUMENTS", arguments);
        // Highest first, so `$1` never eats the start of `$10`.
        for n in (1..=highest).rev() {
            let word = if n == highest { words.get(n - 1..).map(|rest| rest.join(" ")).unwrap_or_default() } else { words.get(n - 1).cloned().unwrap_or_default() };
            text = text.replace(&format!("${n}"), &word);
        }
        if highest == 0 && !self.template.contains("$ARGUMENTS") && !arguments.is_empty() {
            text = format!("{}\n\n{arguments}", text.trim_end());
        }
        text
    }
}

/// The most files one `instructions` glob brings in, so `**/*.md` in a large repository stays bounded.
const MAX_INSTRUCTION_MATCHES: usize = 50;

/// The files an `instructions` entry names, with the name each is shown under: the entry itself for a
/// plain path (relative to its drift.json, absolute, or `~/`), each match for a glob, walked from the
/// part of the path before its first wildcard as git lists files, in name order.
fn instruction_files(listed: &str, resolved: &Path) -> Vec<(String, PathBuf)> {
    let text = resolved.to_string_lossy().replace('\\', "/");
    let Some(wild) = text.find(['*', '?', '[', '{']) else { return vec![(listed.to_string(), resolved.to_path_buf())] };
    let base = PathBuf::from(&text[..text[..wild].rfind('/').unwrap_or(0)]);
    let Ok(glob) = globset::GlobBuilder::new(&text).literal_separator(true).build().map(|glob| glob.compile_matcher()) else { return Vec::new() };
    let mut found: Vec<PathBuf> = crate::tool::walk(&base)
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .map(|entry| entry.into_path())
        .filter(|path| glob.is_match(path.to_string_lossy().replace('\\', "/")))
        .take(MAX_INSTRUCTION_MATCHES * 4)
        .collect();
    found.sort();
    found.truncate(MAX_INSTRUCTION_MATCHES);
    found.into_iter().map(|path| (path.strip_prefix(&base).map_or_else(|_| path.display().to_string(), |rest| rest.to_string_lossy().replace('\\', "/")), path)).collect()
}

/// The highest `$N` a template names, 0 when it names none.
pub fn highest_placeholder(template: &str) -> usize {
    template.split('$').skip(1).filter_map(|rest| rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()).max().unwrap_or(0)
}

/// What follows a command, split as a shell splits words: quotes (single or double) keep spaces and are dropped.
pub fn split_arguments(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let (mut quote, mut started) = (None, false);
    for c in text.chars() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => current.push(c),
            None if c == '"' || c == '\'' => (quote, started) = (Some(c), true),
            None if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                }
                started = false;
            }
            None => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(current);
    }
    words
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// Directory holding SKILL.md and whatever it references.
    pub path: String,
    /// SKILL.md's body as it was when the config was read, so a turn loads the skill it was offered.
    #[serde(skip)]
    pub instructions: String,
    /// Front matter `argument-hint`.
    #[serde(skip)]
    pub argument_hint: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Instruction {
    pub name: String,
    pub text: String,
}

/// Everything resolved for one workspace: home config first, project config over it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub model: Option<ModelRef>,
    /// What drift.json names as `defaultAgent`; [`Config::default_agent`] checks it can run a conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_agent: Option<String>,
    pub permissions: Vec<Rule>,
    pub agents: Vec<Agent>,
    pub commands: Vec<Command>,
    pub skills: Vec<Skill>,
    pub instructions: Vec<Instruction>,
    pub formatters: BTreeMap<String, FormatterConfig>,
    pub checks: BTreeMap<String, CheckConfig>,
    pub lsp: BTreeMap<String, LspConfig>,
    /// Formatters and checks whose command the project's own drift.json sets, as `formatter:<name>` or `check:<name>`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub project_commands: std::collections::BTreeSet<String>,
    pub limits: Limits,
    pub timeouts: BTreeMap<String, RouteTimeouts>,
    /// Config files that could not be read; a turn refuses to start rather than run without their rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
    /// Settings read but not applied (an agent's sampling fields); nothing is refused for them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// The `skillPaths` the files list, resolved; read once the files are applied.
    #[serde(skip)]
    skill_paths: Vec<PathBuf>,
}

impl Config {
    /// A route's time limits: its defaults with whatever drift.json sets for it.
    pub fn route_timeouts(&self, provider: &str) -> crate::llm::http::Timeouts {
        let mut timeouts = crate::llm::http::Timeouts::for_route(provider);
        if let Some(set) = self.timeouts.get(provider) {
            if let Some(seconds) = set.headers_seconds.filter(|s| *s > 0) {
                timeouts.headers = std::time::Duration::from_secs(seconds);
            }
            if let Some(seconds) = set.idle_seconds.filter(|s| *s > 0) {
                timeouts.idle = std::time::Duration::from_secs(seconds);
            }
        }
        timeouts
    }

    /// The limits a turn run by `agent` works under.
    pub fn limits_for(&self, agent: &str) -> Limits {
        let steps = self.agent(agent).and_then(|a| a.steps).unwrap_or(self.limits.steps);
        Limits { steps, ..self.limits }
    }

    pub fn load(workspace: &Path) -> Self {
        Self::load_with_home(workspace, home().as_deref())
    }

    pub fn load_with_home(workspace: &Path, home: Option<&Path>) -> Self {
        let mut config = Self { agents: builtin_agents(), ..Self::default() };
        let mut roots: Vec<PathBuf> = Vec::new();
        if let Some(home) = home {
            roots.push(home.join(".config").join("drift"));
        }
        roots.push(workspace.to_path_buf());
        for root in &roots {
            config.apply_file(root, root == workspace, home);
            // A workspace keeps them in `.drift/`; the user's own sit in `~/.config/drift/` itself, beside its `skills`.
            config.apply_dir(&if root == workspace { root.join(DIR) } else { root.clone() });
        }
        for dir in skill_folders(workspace, home, std::mem::take(&mut config.skill_paths)) {
            config.add_skills(&dir);
        }
        config.add_skill_commands();
        config.add_instructions(workspace, home);
        config
    }

    /// Each skill is a command unless one already has its name. A command that calls exactly one skill
    /// (`skill({ name: "design" })`, under any name) offers that skill's choices with its own template.
    fn add_skill_commands(&mut self) {
        for command in self.commands.iter_mut().filter(|command| command.server.is_none()) {
            if let Some(skill) = arguments::referenced_skill(&command.template).and_then(|name| self.skills.iter().find(|skill| skill.name == name)) {
                command.document(skill);
            }
        }
        for skill in &self.skills {
            if self.commands.iter().any(|command| command.name == skill.name) {
                continue;
            }
            let mut command = Command::new(skill.name.clone(), skill.description.clone(), skill.instructions.clone());
            command.skill = Some(skill.name.clone());
            command.document(skill);
            self.commands.push(command);
        }
    }

    pub fn policy(&self) -> Policy {
        Policy { rules: self.permissions.clone() }
    }

    /// An agent's rules, from its file or Settings, are kept as written and resolve as in opencode: the last match wins.
    pub fn agent_policy(&self, agent: &str) -> Policy {
        Policy { rules: self.agent(agent).map(|agent| agent.permissions.iter().rev().cloned().collect()).unwrap_or_default() }
    }

    pub fn agent(&self, name: &str) -> Option<&Agent> {
        self.agents.iter().find(|a| a.name == name)
    }

    /// The agent a new session runs as: drift.json's `defaultAgent` when it names one that runs
    /// conversations, else `build`, else (`build` disabled) the first visible one that does, as opencode falls back.
    pub fn default_agent(&self) -> &str {
        let runs = |agent: &&Agent| agent.kind.runs_conversations() && agent.problem.is_none();
        let named = self.default_agent.as_deref().and_then(|name| self.agent(name)).filter(runs);
        let fallback = || self.agent("build").filter(runs).or_else(|| self.agents.iter().filter(|agent| !agent.hidden).find(runs));
        named.or_else(fallback).map_or("build", |agent| agent.name.as_str())
    }

    /// The model an agent is pinned to, if any; unpinned agents inherit from whatever runs them.
    pub fn agent_model(&self, name: &str) -> Option<ModelRef> {
        self.agent(name).and_then(|agent| agent.model.clone())
    }

    pub fn skill(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
    }

    /// Records the commands a project file names, so they run only once the user trusts them; a `false` there runs nothing.
    fn note_project_commands(&mut self, formatters: &BTreeMap<String, FormatterConfig>, checks: &BTreeMap<String, CheckConfig>) {
        let named = |kind: &str, name: &String, custom: bool| (format!("{kind}:{name}"), custom);
        let entries = formatters
            .iter()
            .map(|(name, set)| named("formatter", name, matches!(set, FormatterConfig::Custom { .. })))
            .chain(checks.iter().map(|(name, set)| named("check", name, matches!(set, CheckConfig::Custom { .. }))));
        for (key, custom) in entries {
            if custom {
                self.project_commands.insert(key);
            } else {
                self.project_commands.remove(&key);
            }
        }
    }

    /// One of the project's own commands as the user is asked to trust it (`check lint: eslint $FILE`),
    /// with the extensions it runs on.
    fn project_line(&self, key: &str) -> Option<(String, &[String])> {
        let (kind, name) = key.split_once(':')?;
        let (command, extensions) = match kind {
            "check" => match self.checks.get(name)? {
                CheckConfig::Custom { command, extensions } => (command, extensions),
                CheckConfig::Enabled(_) => return None,
            },
            _ => match self.formatters.get(name)? {
                FormatterConfig::Custom { command, extensions } => (command, extensions),
                FormatterConfig::Enabled(_) => return None,
            },
        };
        Some((format!("{kind} {name}: {}", command.join(" ")), extensions))
    }

    /// The project's own commands of `kind` (`check` or `formatter`) that would run on one of `files`.
    pub fn project_command_lines(&self, kind: &str, files: &[PathBuf]) -> Vec<String> {
        let covers = |extensions: &[String]| {
            files.iter().any(|file| {
                let name = file.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
                extensions.iter().any(|ext| name.ends_with(&ext.to_lowercase()))
            })
        };
        self.project_commands
            .iter()
            .filter(|key| key.split_once(':').is_some_and(|(of, _)| of == kind))
            .filter_map(|key| self.project_line(key))
            .filter(|(_, extensions)| covers(extensions))
            .map(|(line, _)| line)
            .collect()
    }

    /// Formatters and checks without the project's own commands the user has not allowed (`allowed`
    /// judges each by its line); built-in formatters and the user's own always stay.
    pub fn only_allowed(&self, allowed: impl Fn(&str) -> bool) -> (BTreeMap<String, FormatterConfig>, BTreeMap<String, CheckConfig>) {
        let keeps = |kind: &str, name: &String| {
            let key = format!("{kind}:{name}");
            !self.project_commands.contains(&key) || self.project_line(&key).is_some_and(|(line, _)| allowed(&line))
        };
        let formatters = self.formatters.iter().filter(|(name, _)| keeps("formatter", name)).map(|(n, c)| (n.clone(), c.clone())).collect();
        let checks = self.checks.iter().filter(|(name, _)| keeps("check", name)).map(|(n, c)| (n.clone(), c.clone())).collect();
        (formatters, checks)
    }

    fn apply_file(&mut self, root: &Path, project: bool, home: Option<&Path>) {
        let path = root.join(FILE);
        let Ok(text) = std::fs::read_to_string(&path) else { return };
        let file = match serde_json::from_str::<File>(&jsonc::strip(&text)) {
            Ok(file) => file,
            Err(error) => {
                self.problems.push(format!("{} could not be read ({error}), so none of its settings or permission rules apply; fix it to carry on", path.display()));
                return;
            }
        };
        if file.model.is_some() {
            self.model = file.model;
        }
        if file.default_agent.is_some() {
            self.default_agent = file.default_agent;
        }
        // Later files' rules come first, so a project rule beats a home rule for the same pattern.
        let mut rules = file.permissions;
        rules.append(&mut self.permissions);
        self.permissions = rules;
        if project {
            self.note_project_commands(&file.formatters, &file.checks);
        }
        self.formatters.extend(file.formatters);
        self.checks.extend(file.checks);
        self.apply_lsp(file.lsp, project);
        self.timeouts.extend(file.timeouts);
        let limits = file.limits;
        self.limits.steps = limits.steps.unwrap_or(self.limits.steps).max(1);
        self.limits.repeats = limits.repeats.unwrap_or(self.limits.repeats).max(2);
        self.limits.polls = limits.polls.unwrap_or(self.limits.polls).max(2);
        let resolve = |listed: &String| match (listed.strip_prefix("~/"), home) {
            (Some(rest), Some(home)) => home.join(rest),
            _ => root.join(listed),
        };
        for listed in &file.instructions {
            for (name, path) in instruction_files(listed, &resolve(listed)) {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    self.instructions.push(Instruction { name, text: clip(&text) });
                }
            }
        }
        self.skill_paths.extend(file.skill_paths.iter().map(resolve));
    }

    /// A language server starts on its own when a file it handles is written, so a project's file
    /// (committed by others) may turn one off but never name a program to run; that is noted, not applied.
    fn apply_lsp(&mut self, servers: BTreeMap<String, LspConfig>, project: bool) {
        for (name, server) in servers {
            match server {
                LspConfig::Custom { .. } if project => self.warnings.push(format!("lsp {name}: a project's drift.json can only turn a language server off; set its command in ~/.config/drift/drift.json")),
                server => {
                    self.lsp.insert(name, server);
                }
            }
        }
    }

    /// A file named after an action customises that action; otherwise `mode` decides, then the agent it
    /// replaces, then `all`, as opencode reads a user's agent.
    fn workspace_kind(&self, name: &str, mode: Option<&str>) -> AgentKind {
        match self.agent(name).map(|existing| existing.kind) {
            Some(AgentKind::Action) => AgentKind::Action,
            Some(existing) if mode.is_none() => existing,
            _ => AgentKind::from_mode(mode),
        }
    }

    fn apply_dir(&mut self, dir: &Path) {
        for (name, doc) in markdown_files(&dir.join("agents")) {
            // `disable: true` takes the agent away, a built-in by the same name included; the engine's own jobs need theirs.
            if doc.field("disable").is_some_and(|value| value.trim() == "true") {
                if self.agent(&name).is_some_and(|agent| agent.kind == AgentKind::Action) {
                    self.warnings.push(format!("agent {name}: disable ignored; Drift needs it for {name}s"));
                } else {
                    self.agents.retain(|a| a.name != name);
                }
                continue;
            }
            let ignored: Vec<&str> = ["temperature", "top_p", "topP", "options", "provider_options"].into_iter().filter(|key| doc.fields.contains_key(*key)).collect();
            // Agents ported from opencode run; the fields they set are named once and the model's own sampling used.
            if !ignored.is_empty() {
                self.warnings.push(format!("agent {name}: {} ignored; Drift uses the model's own sampling", ignored.join(", ")));
            }
            let (permissions, problem) = match doc.permissions() {
                Ok(rules) => (rules, None),
                Err(error) => (Vec::new(), Some(error)),
            };
            let agent = Agent {
                description: doc.field("description").unwrap_or_default(),
                prompt: doc.body.trim().into(),
                model: doc.field("model").and_then(|m| parse_model(&m)),
                tools: agent_tools(&doc),
                builtin: false,
                kind: self.workspace_kind(&name, doc.field("mode").as_deref()),
                steps: doc.field("steps").and_then(|s| s.trim().parse().ok()).filter(|s: &u32| *s > 0),
                background: doc.field("background").and_then(|b| b.trim().parse().ok()),
                read_only: doc.field("read_only").is_some_and(|value| value.trim() == "true"),
                name: name.clone(),
                permissions,
                variant: doc.field("variant"),
                problem,
                hidden: doc.field("hidden").is_some_and(|value| value.trim() == "true"),
            };
            self.agents.retain(|a| a.name != name);
            self.agents.push(agent);
        }
        for (name, doc) in markdown_files(&dir.join("commands")) {
            self.commands.retain(|c| c.name != name);
            let mut command = Command::new(name, doc.field("description").unwrap_or_default(), doc.body.trim().into());
            command.agent = doc.field("agent");
            command.model = doc.field("model").and_then(|model| parse_model(&model));
            command.subtask = doc.field("subtask").and_then(|value| value.parse().ok());
            command.arguments = (1..=highest_placeholder(&command.template)).filter(|n| command.template.contains(&format!("${n}"))).map(|n| format!("arg{n}")).collect();
            self.commands.push(command);
        }
    }

    /// Every `SKILL.md` under `dir`, at any depth up to [`SKILL_DEPTH`]; a name already found nearer wins.
    fn add_skills(&mut self, dir: &Path) {
        for path in skill_files(dir) {
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let folder = path.parent().unwrap_or(dir);
            let doc = frontmatter::parse(&text);
            let name = doc.field("name").unwrap_or_else(|| folder.file_name().unwrap_or_default().to_string_lossy().into_owned());
            if self.skills.iter().any(|s| s.name == name) {
                continue;
            }
            let instructions = body(&text);
            let argument_hint = doc.field("argument-hint").filter(|hint| !hint.trim().is_empty());
            self.skills.push(Skill { name, description: doc.field("description").unwrap_or_default(), path: folder.to_string_lossy().into_owned(), instructions, argument_hint });
        }
    }

    /// The user's own file, then each directory's from the repository root down to the workspace,
    /// before what drift.json lists: general rules first, the most specific last.
    fn add_instructions(&mut self, workspace: &Path, home: Option<&Path>) {
        let mut found = Vec::new();
        if let Some(home) = home {
            let global = [(home.join(".config/drift/AGENTS.md"), "~/.config/drift/AGENTS.md"), (home.join(".claude/CLAUDE.md"), "~/.claude/CLAUDE.md")];
            if let Some((text, name)) = global.iter().find_map(|(path, name)| std::fs::read_to_string(path).ok().map(|text| (text, *name))) {
                found.push(Instruction { name: name.into(), text: clip(&text) });
            }
        }
        let chain = ancestors_to_repo_root(workspace);
        for (depth, dir) in chain.iter().enumerate().rev() {
            if let Some((name, text)) = instruction_file(dir) {
                let shown = format!("{}{name}", "../".repeat(depth));
                found.push(Instruction { name: shown, text: clip(&text) });
            }
        }
        found.append(&mut self.instructions);
        self.instructions = found;
    }
}

/// Skill folders in precedence order: project ancestors, configured paths, then home folders.
fn skill_folders(workspace: &Path, home: Option<&Path>, listed: Vec<PathBuf>) -> Vec<PathBuf> {
    let project = ancestors_to_repo_root(workspace).into_iter().flat_map(|dir| SKILL_DIRS.map(|skills| dir.join(skills)));
    let user = home.into_iter().flat_map(|home| HOME_SKILL_DIRS.map(|skills| home.join(skills)));
    project.chain(listed).chain(user).collect()
}

/// The `SKILL.md` files under `dir`, sorted, so the same tree always yields the same skills.
fn skill_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![(dir.to_path_buf(), 0)];
    while let Some((folder, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let skipped = matches!(entry.file_name().to_str(), Some("node_modules" | ".git"));
            if path.is_dir() && depth < SKILL_DEPTH && !skipped {
                pending.push((path, depth + 1));
            } else if entry.file_name() == "SKILL.md" {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The workspace and its parents up to the repository root (the nearest holding `.git`), nearest
/// first; just the workspace when it is in no repository.
fn ancestors_to_repo_root(workspace: &Path) -> Vec<PathBuf> {
    let chain: Vec<PathBuf> = workspace.ancestors().map(Path::to_path_buf).collect();
    match chain.iter().position(|dir| dir.join(".git").exists()) {
        Some(root) => chain[..=root].to_vec(),
        None => vec![workspace.to_path_buf()],
    }
}

/// Whether the workspace is in a git repository.
pub fn in_repository(workspace: &Path) -> bool {
    workspace.ancestors().any(|dir| dir.join(".git").exists())
}

/// The first of AGENTS.md and CLAUDE.md in `dir`.
fn instruction_file(dir: &Path) -> Option<(&'static str, String)> {
    INSTRUCTION_FILES.iter().find_map(|name| std::fs::read_to_string(dir.join(name)).ok().map(|text| (*name, text)))
}

/// Instruction files in the directories between the workspace (not included) and `file`, outermost
/// first: rules for a part of the tree that the system prompt does not carry.
pub fn nested_instructions(workspace: &Path, file: &Path) -> Vec<(PathBuf, String)> {
    let Ok(relative) = file.parent().unwrap_or(file).strip_prefix(workspace) else { return Vec::new() };
    let mut dir = workspace.to_path_buf();
    let mut found = Vec::new();
    for part in relative.components() {
        dir.push(part);
        if let Some((name, text)) = instruction_file(&dir) {
            found.push((dir.join(name), clip(&text)));
        }
    }
    found
}

/// An agent file's tools. An explicit empty list (`tools: []`) means none, written `!*`; leaving
/// the field out, or an empty map (`tools: {}`), means every tool.
fn agent_tools(doc: &frontmatter::Document) -> Vec<String> {
    let tools = doc.list("tools").unwrap_or_default();
    let empty_list = doc.field("tools").is_some_and(|raw| raw.replace(' ', "") == "[]");
    if tools.is_empty() && empty_list {
        return vec!["!*".into()];
    }
    tools
}

/// A Markdown file's text after its front matter.
pub fn body(text: &str) -> String {
    frontmatter::parse(text).body
}

fn builtin_agents() -> Vec<Agent> {
    let agent = |name: &str, description: &str, prompt: &str, tools: &[&str], kind| Agent {
        name: name.into(),
        description: description.into(),
        prompt: prompt.trim().into(),
        model: None,
        tools: tools.iter().map(|t| t.to_string()).collect(),
        builtin: true,
        kind,
        steps: None,
        background: None,
        read_only: false,
        permissions: Vec::new(),
        variant: None,
        problem: None,
        hidden: false,
    };
    let read_only = |agent: Agent| Agent { read_only: true, ..agent };
    vec![
        agent("build", "Reads, edits and runs code.", "", &[], AgentKind::Primary),
        // Offered build's tools, so switching between them keeps the cache; what would change something is refused.
        read_only(agent("plan", "Explores and proposes; changes nothing.", include_str!("prompts/plan.txt"), &[], AgentKind::Primary)),
        agent("general", "General-purpose subagent for multi-step work: researching, and making changes. The default for task.", include_str!("prompts/general.txt"), &[], AgentKind::Subagent),
        read_only(agent("explore", "Fast read-only subagent for finding files and code and answering questions about a codebase.", include_str!("prompts/explore.txt"), &["read", "glob", "grep", "bash", "webfetch", "skill"], AgentKind::Subagent)),
        // Delegates every change and command to subagents, so it is offered no tool that writes or runs anything itself.
        agent("orchestrator", "Drives a goal to completion by delegating to subagents, verifying results, and correcting course", include_str!("prompts/orchestrator.txt"), &["read", "glob", "grep", "webfetch", "todowrite", "skill", "question", "task", "task_output", "task_stop", "read_thread"], AgentKind::Primary),
        agent("title", "Names new conversations. Default model: a small one from the conversation's provider.", include_str!("prompts/title.txt"), &[], AgentKind::Action),
        agent("compaction", "Summarises long conversations to free context. Default model: the conversation's.", include_str!("prompts/compaction.txt"), &[], AgentKind::Action),
    ]
}

fn markdown_files(dir: &Path) -> Vec<(String, frontmatter::Document)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<(String, frontmatter::Document)> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .filter_map(|e| {
            let text = std::fs::read_to_string(e.path()).ok()?;
            Some((e.path().file_stem()?.to_string_lossy().into_owned(), frontmatter::parse(&text)))
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

pub(crate) fn parse_model(text: &str) -> Option<ModelRef> {
    let (provider, model) = text.trim().split_once('/')?;
    Some(ModelRef { provider: provider.into(), model: model.into() })
}

fn clip(text: &str) -> String {
    if text.chars().count() <= MAX_INSTRUCTION_CHARS {
        return text.trim().into();
    }
    format!("{}\n\n(truncated)", text.chars().take(MAX_INSTRUCTION_CHARS).collect::<String>())
}

pub fn home() -> Option<PathBuf> {
    // The engine's own tests never read the real user's agents, commands or providers; they pass a home.
    if cfg!(test) {
        return None;
    }
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::Decision;

    fn write(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn plugins_come_from_the_users_file_and_stay_under_its_directory() {
        let root = std::env::temp_dir().join(format!("drift-config-plugins-{}", crate::random_hex(4)));
        let (home, ws) = (root.join("home"), root.join("ws"));
        write(&home, ".config/drift/drift.json", r#"{ "plugins": [{ "path": "plugins/guard.wasm", "config": { "strict": true } }, "../escape.wasm", "C:/abs.wasm", "plugins/script.js"] }"#);
        write(&ws, "drift.json", r#"{ "plugins": ["theirs.wasm"] }"#);
        let listed = user_plugins_in(&home);
        assert_eq!(listed.len(), 4, "the project's file adds none");
        assert_eq!(listed[0].path, Ok(home.join(".config/drift").join("plugins/guard.wasm")));
        assert_eq!(listed[0].config["strict"], true);
        assert_eq!(listed[1].config, serde_json::json!({}));
        assert!(listed[1].path.as_ref().is_err_and(|error| error.contains("stay under")));
        assert!(listed[2].path.is_err());
        assert!(listed[3].path.as_ref().is_err_and(|error| error.contains(".wasm")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_commands_the_project_file_names_wait_for_the_users_say_so() {
        let root = std::env::temp_dir().join(format!("drift-config-trust-{}", crate::random_hex(4)));
        let (home, ws) = (root.join("home"), root.join("ws"));
        write(&home, ".config/drift/drift.json", r#"{ "checks": { "mine": { "command": ["tsc"], "extensions": [".ts"] }, "shared": { "command": ["eslint", "$FILE"], "extensions": [".ts"] } } }"#);
        write(&ws, "drift.json", r#"{ "checks": { "shared": false, "theirs": { "command": ["make", "lint"], "extensions": [".c"] } }, "formatters": { "prettier": { "command": ["./fmt.sh", "$FILE"], "extensions": [".ts"] }, "rustfmt": false } }"#);
        let config = Config::load_with_home(&ws, Some(&home));
        assert_eq!(config.project_command_lines("check", &[ws.join("main.C")]), ["check theirs: make lint"]);
        assert_eq!(config.project_command_lines("formatter", &[ws.join("app.ts")]), ["formatter prettier: ./fmt.sh $FILE"]);
        assert!(config.project_command_lines("check", &[ws.join("app.ts")]).is_empty(), "a check is asked about only for files it runs on");
        assert!(config.project_command_lines("formatter", &[ws.join("notes.md")]).is_empty(), "nothing of the project's would run on it, so nothing to ask");
        let (formatters, checks) = config.only_allowed(|_| false);
        assert_eq!(checks.keys().collect::<Vec<_>>(), ["mine", "shared"], "the user's own run; a project's `false` still turns one off");
        assert!(!formatters.contains_key("prettier") && formatters.contains_key("rustfmt"), "the built-in prettier comes back; a project's `false` stands");
        let (formatters, checks) = config.only_allowed(|line| line.starts_with("check "));
        assert!(checks.contains_key("theirs") && !formatters.contains_key("prettier"), "each command is judged on its own");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn skill_commands_and_the_wrappers_that_call_them_offer_the_skills_choices() {
        let ws = std::env::temp_dir().join(format!("drift-config-arguments-{}", crate::random_hex(4)));
        let skill = "---\nname: test-skill\ndescription: Skill description\nargument-hint: \"[audit|polish] [target]\"\n---\n| Command | Description |\n|---|---|\n| audit [target] | Check accessibility |\n| polish [target] | Final quality pass |";
        write(&ws, ".drift/skills/test-skill/SKILL.md", skill);
        write(&ws, ".drift/skills/ordinary/SKILL.md", &skill.replace("name: test-skill", "name: ordinary"));
        let template = r#"Call skill({ name: "test-skill" }) and follow its Commands section to handle $ARGUMENTS."#;
        write(&ws, ".drift/commands/design.md", &format!("---\ndescription: Wrapper\nagent: build\nsubtask: true\n---\n{template}"));
        write(&ws, ".drift/commands/ordinary.md", "---\ndescription: Ordinary command\n---\nDo ordinary work.");
        let config = Config::load_with_home(&ws, None);
        let command = |name: &str| config.commands.iter().find(|command| command.name == name).unwrap();
        let choices = |name: &str| command(name).subcommands.iter().map(|s| (s.name.clone(), s.description.clone(), s.usage.clone())).collect::<Vec<_>>();
        let expected = vec![("audit".to_string(), "Check accessibility".to_string(), Some("[target]".to_string())), ("polish".into(), "Final quality pass".into(), Some("[target]".into()))];
        assert_eq!((command("test-skill").usage.as_deref(), choices("test-skill")), (Some("[audit|polish] [target]"), expected.clone()));
        let design = command("design");
        assert_eq!((design.template.as_str(), design.agent.as_deref(), design.subtask), (template, Some("build"), Some(true)), "the wrapper keeps its own settings");
        assert_eq!(choices("design"), expected, "and offers the skill's choices");
        assert!(command("ordinary").subcommands.is_empty() && command("ordinary").usage.is_none(), "a same-name command that calls no skill inherits nothing");
        std::fs::remove_dir_all(ws).ok();
    }

    #[test]
    fn agent_modes_hidden_disable_and_the_default_agent_read_as_opencode_reads_them() {
        let root = std::env::temp_dir().join(format!("drift-modes-{}", crate::random_hex(4)));
        let ws = root.join("ws");
        write(&ws, ".drift/agents/helper.md", "---\ndescription: No mode\n---\nHelp.");
        write(&ws, ".drift/agents/both.md", "---\ndescription: Both\nmode: all\n---\nBoth.");
        write(&ws, ".drift/agents/lead.md", "---\ndescription: Lead\nmode: primary\n---\nLead.");
        write(&ws, ".drift/agents/quiet.md", "---\ndescription: Internal\nmode: subagent\nhidden: true\n---\nQuiet.");
        write(&ws, ".drift/agents/explore.md", "---\ndisable: true\n---\n");
        write(&ws, ".drift/agents/gone.md", "---\ndescription: Off\ndisable: true\n---\nNever.");
        write(&ws, "drift.json", r#"{ "defaultAgent": "lead" }"#);
        let config = Config::load_with_home(&ws, None);
        let kind = |name: &str| config.agent(name).map(|agent| agent.kind);
        assert_eq!(kind("helper"), Some(AgentKind::All), "no mode means both, as in opencode");
        assert_eq!((kind("both"), kind("lead"), kind("quiet")), (Some(AgentKind::All), Some(AgentKind::Primary), Some(AgentKind::Subagent)));
        assert!(AgentKind::All.runs_conversations() && AgentKind::All.delegated_to() && !AgentKind::Primary.delegated_to() && !AgentKind::Subagent.runs_conversations());
        assert!(config.agent("quiet").unwrap().hidden && !config.agent("helper").unwrap().hidden);
        assert!(config.agent("explore").is_none() && config.agent("gone").is_none(), "disable takes an agent away, a built-in included");
        assert_eq!(config.default_agent(), "lead");
        write(&ws, "drift.json", r#"{ "defaultAgent": "quiet" }"#);
        assert_eq!(Config::load_with_home(&ws, None).default_agent(), "build", "a subagent cannot run a conversation, so the default stands");
        write(&ws, ".drift/agents/build.md", "---\ndisable: true\n---\n");
        write(&ws, ".drift/agents/compaction.md", "---\ndisable: true\n---\n");
        let without_build = Config::load_with_home(&ws, None);
        assert_eq!(without_build.default_agent(), "plan", "build disabled: the first agent that runs conversations");
        assert!(without_build.agent("compaction").is_some_and(|agent| !agent.prompt.is_empty()), "the engine's own jobs keep their agents");
        assert!(without_build.warnings.iter().any(|warning| warning.starts_with("agent compaction: disable ignored")), "{:?}", without_build.warnings);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn project_config_layers_over_home_and_discovers_everything() {
        let root = std::env::temp_dir().join(format!("drift-config-{}", crate::random_hex(4)));
        let home = root.join("home");
        let ws = root.join("ws");
        write(&home, ".config/drift/drift.json", r#"{ "model": { "provider": "anthropic", "model": "haiku" }, "permissions": [{ "kind": "bash", "pattern": "git *", "decision": "allow" }] }"#);
        write(&home, ".agents/skills/review/SKILL.md", "---\nname: review\ndescription: Reviews code\n---\nHow to review.");
        write(&home, ".claude/skills/team/lint/SKILL.md", "---\nname: lint\ndescription: Lints\n---\nLint it.");
        write(&home, ".agents/skills/notes/SKILL.md", "---\ndescription: Takes notes\n---\nWrite it down.");
        write(&home, "shared/skills/release/SKILL.md", "---\ndescription: Releases\n---\nTag it.");
        write(&ws, "drift.json", r#"{ "permissions": [{ "kind": "bash", "pattern": "git push*", "decision": "deny" }], "instructions": ["docs/rules.md"], "skillPaths": ["~/shared/skills"] }"#);
        write(&ws, "docs/rules.md", "Be careful.");
        write(&ws, "AGENTS.md", "Repo rules.");
        write(&ws, "CLAUDE.md", "ignored when AGENTS.md exists");
        write(&ws, ".drift/agents/reviewer.md", "---\ndescription: Reviews PRs\nmode: subagent\nmodel: openai/gpt-5.5\ntools: read, grep\n---\nYou review.");
        write(&ws, ".drift/agents/explore.md", "---\ndescription: Our explorer\n---\nSearch our monorepo.");
        write(&ws, ".drift/agents/plan.md", "---\ndescription: My plan\n---\nCustom plan.");
        write(&ws, ".drift/agents/title.md", "---\nmodel: openai/gpt-5-nano\n---\nShort titles.");
        write(&ws, ".drift/commands/test.md", "---\ndescription: Run tests\n---\nRun the tests for $ARGUMENTS and report.");
        write(&ws, ".drift/skills/review/SKILL.md", "---\nname: review\ndescription: Project review\n---\nProject way.");
        write(&ws, ".claude/skills/deploy/SKILL.md", "---\ndescription: Deploys\n---\nShip it.");

        let config = Config::load_with_home(&ws, Some(&home));
        assert_eq!(config.model, Some(ModelRef { provider: "anthropic".into(), model: "haiku".into() }));
        assert_eq!(config.permissions.iter().map(|r| (r.pattern.as_str(), r.decision)).collect::<Vec<_>>(), [("git push*", Decision::Deny), ("git *", Decision::Allow)]);
        assert_eq!(config.policy().decide(&crate::tool::Ask::new("bash", "git push origin", "")), Decision::Deny);

        let names: Vec<&str> = config.agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["build", "general", "orchestrator", "compaction", "explore", "plan", "reviewer", "title"]);
        assert_eq!(config.agent("reviewer").unwrap().kind, AgentKind::Subagent, "mode: subagent keeps it out of the composer");
        assert_eq!(config.agent("explore").unwrap().kind, AgentKind::Subagent, "replacing a subagent without a mode keeps its kind");
        assert_eq!(config.agent("plan").unwrap().kind, AgentKind::Primary);
        let title = config.agent("title").unwrap();
        assert_eq!((title.kind, title.prompt.as_str()), (AgentKind::Action, "Short titles."), "a project file customises an action, it does not replace it");
        assert_eq!(config.agent_model("title"), Some(ModelRef { provider: "openai".into(), model: "gpt-5-nano".into() }));
        let reviewer = config.agent("reviewer").unwrap();
        assert_eq!(reviewer.tools, ["read", "grep"]);
        assert_eq!(reviewer.model, Some(ModelRef { provider: "openai".into(), model: "gpt-5.5".into() }));
        assert_eq!(reviewer.prompt, "You review.");
        assert!(!config.agent("plan").unwrap().builtin, "a project agent replaces the built-in of the same name");

        assert_eq!(config.commands[0].name, "test");
        assert!(config.commands[0].template.contains("$ARGUMENTS"));

        let skills: Vec<(&str, &str)> = config.skills.iter().map(|s| (s.name.as_str(), s.description.as_str())).collect();
        assert_eq!(
            skills,
            [("review", "Project review"), ("deploy", "Deploys"), ("release", "Releases"), ("notes", "Takes notes"), ("lint", "Lints")],
            "project skills shadow home skills of the same name; home ones come from ~/.agents and ~/.claude at any depth, and listed paths are searched too"
        );

        assert_eq!(config.instructions.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["AGENTS.md", "docs/rules.md"]);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_empty_workspace_still_has_the_builtin_agents() {
        let ws = std::env::temp_dir().join(format!("drift-config-empty-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&ws).unwrap();
        let config = Config::load_with_home(&ws, None);
        let kinds: Vec<(&str, AgentKind)> = config.agents.iter().map(|a| (a.name.as_str(), a.kind)).collect();
        assert_eq!(
            kinds,
            [
                ("build", AgentKind::Primary),
                ("plan", AgentKind::Primary),
                ("general", AgentKind::Subagent),
                ("explore", AgentKind::Subagent),
                ("orchestrator", AgentKind::Primary),
                ("title", AgentKind::Action),
                ("compaction", AgentKind::Action),
            ]
        );
        assert!(!config.agent("explore").unwrap().tools.contains(&"edit".to_string()), "explore is read-only");
        let orchestrator = config.agent("orchestrator").unwrap();
        assert!(["edit", "write", "apply_patch", "bash"].iter().all(|tool| !orchestrator.allows_tool(tool)) && orchestrator.allows_tool("task"), "the orchestrator delegates; it never changes or runs anything itself");
        assert!(orchestrator.prompt.contains("<orchestrator_status>"));
        assert!(config.agent("plan").unwrap().read_only && config.agent("explore").unwrap().read_only);
        assert!(config.commands.is_empty() && config.skills.is_empty() && config.instructions.is_empty());
        std::fs::remove_dir_all(ws).ok();
    }

    #[test]
    fn agent_tool_lists_in_every_shape_restrict_and_match_any_case() {
        let ws = std::env::temp_dir().join(format!("drift-config-tools-{}", crate::random_hex(4)));
        write(&ws, ".drift/agents/listed.md", "---\ndescription: Read-only\ntools:\n  - read\n  - grep\n---\nReview.");
        write(&ws, ".drift/agents/claude.md", "---\ndescription: Claude style\ntools: Read, Grep\n---\nReview.");
        write(&ws, ".drift/agents/opencode.md", "---\ndescription: No writes\ntools:\n  write: false\n  edit: false\n  bash: true\n---\nLook.");
        write(&ws, ".drift/agents/none.md", "---\ndescription: Talks only\ntools: []\n---\nTalk.");
        write(&ws, ".drift/agents/all.md", "---\ndescription: Everything\ntools: {}\n---\nDo.");
        let config = Config::load_with_home(&ws, None);
        for name in ["listed", "claude"] {
            let agent = config.agent(name).unwrap();
            assert!(agent.allows_tool("read") && agent.allows_tool("grep"), "{name}");
            assert!(!agent.allows_tool("edit") && !agent.allows_tool("bash"), "{name} is not handed every tool");
        }
        let opencode = config.agent("opencode").unwrap();
        assert!(opencode.allows_tool("bash") && opencode.allows_tool("read"), "a true entry does not narrow the rest");
        assert!(!opencode.allows_tool("write") && !opencode.allows_tool("edit"));
        assert!(!config.agent("none").unwrap().allows_tool("read"), "an empty list means no tools");
        assert!(config.agent("all").unwrap().allows_tool("bash"), "an empty map means every tool");
        assert!(config.agent("build").unwrap().allows_tool("anything"), "no list means every tool");
        let mut widened = config.agent("explore").unwrap().clone();
        widened.tools = vec!["*".into()];
        assert!(widened.allows_tool("edit") && widened.allows_tool("bash"), "a Settings override can widen a narrowed agent back to every tool");
        std::fs::remove_dir_all(ws).ok();
    }

    #[test]
    fn command_arguments_fill_placeholders_or_follow_the_template() {
        let command = |template: &str| Command::new("c".into(), String::new(), template.into());
        assert_eq!(command("Run tests for $ARGUMENTS.").expand(" src/a.rs  "), "Run tests for src/a.rs.");
        assert_eq!(command("Move $1 to $2").expand("a.rs lib/b c.rs"), "Move a.rs to lib/b c.rs", "the highest takes the rest");
        assert_eq!(command("Only $1").expand(""), "Only ");
        assert_eq!(command("Review the diff.\n").expand("focus on errors"), "Review the diff.\n\nfocus on errors", "not dropped");
        assert_eq!(command("Review the diff.").expand(""), "Review the diff.");
        assert_eq!(command("Commit as $1 with $2").expand(r#""Kyle P" 'fix the "parser" bug'"#), r#"Commit as Kyle P with fix the "parser" bug"#, "quotes keep spaces, as in a shell");
        let ten = command("$1|$2|$3|$4|$5|$6|$7|$8|$9|$10|$11");
        assert_eq!(ten.expand("a b c d e f g h i j k l"), "a|b|c|d|e|f|g|h|i|j|k l", "past $9, and $1 never eats the start of $10");
        assert_eq!(split_arguments(r#"one "two three"  '' four"#), ["one", "two three", "", "four"], "an empty quoted argument is still one");
        assert_eq!(highest_placeholder("cost $5 and $12, not $ARGUMENTS"), 12);
    }

    #[test]
    fn instructions_come_from_home_and_every_directory_up_to_the_repo_root() {
        let root = std::env::temp_dir().join(format!("drift-config-chain-{}", crate::random_hex(4)));
        let (home, repo) = (root.join("home"), root.join("repo"));
        let ws = repo.join("apps/web");
        write(&home, ".config/drift/AGENTS.md", "mine everywhere");
        write(&home, ".claude/CLAUDE.md", "not read when Drift's own exists");
        write(&repo, ".git/HEAD", "ref: refs/heads/main");
        write(&repo, "AGENTS.md", "repo rules");
        write(&repo, "apps/CLAUDE.md", "apps rules");
        write(&ws, "AGENTS.md", "web rules");
        write(&root, "AGENTS.md", "outside the repo, never read");
        let config = Config::load_with_home(&ws, Some(&home));
        let names: Vec<&str> = config.instructions.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["~/.config/drift/AGENTS.md", "../../AGENTS.md", "../CLAUDE.md", "AGENTS.md"]);
        assert_eq!(config.instructions[0].text, "mine everywhere");
        std::fs::remove_file(home.join(".config/drift/AGENTS.md")).unwrap();
        let fallback = Config::load_with_home(&ws, Some(&home));
        assert_eq!(fallback.instructions[0].name, "~/.claude/CLAUDE.md");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn listed_instructions_take_globs_absolute_paths_and_home() {
        let root = std::env::temp_dir().join(format!("drift-config-listed-{}", crate::random_hex(4)));
        let (home, ws, elsewhere) = (root.join("home"), root.join("ws"), root.join("shared"));
        write(&home, "notes/style.md", "home style");
        write(&ws, "docs/rules/a.md", "rule a");
        write(&ws, "docs/rules/deep/b.md", "rule b");
        write(&ws, "docs/rules/skip.txt", "not markdown");
        write(&elsewhere, "team.md", "team rules");
        let absolute = elsewhere.join("team.md").to_string_lossy().replace('\\', "/");
        write(&ws, "drift.json", &format!(r#"{{ "instructions": ["docs/rules/**/*.md", "~/notes/style.md", "{absolute}", "missing.md"] }}"#));
        let config = Config::load_with_home(&ws, Some(&home));
        let listed: Vec<(&str, &str)> = config.instructions.iter().filter(|i| !i.name.ends_with("AGENTS.md")).map(|i| (i.name.as_str(), i.text.as_str())).collect();
        assert_eq!(listed, [("a.md", "rule a"), ("deep/b.md", "rule b"), ("~/notes/style.md", "home style"), (absolute.as_str(), "team rules")]);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_jsonc_drift_json_applies_and_a_broken_one_is_reported_not_ignored() {
        let ws = std::env::temp_dir().join(format!("drift-config-jsonc-{}", crate::random_hex(4)));
        write(&ws, "drift.json", "{\n  // never push\n  \"permissions\": [{ \"kind\": \"bash\", \"pattern\": \"git push*\", \"decision\": \"deny\", },],\n}");
        let config = Config::load_with_home(&ws, None);
        assert!(config.problems.is_empty(), "{:?}", config.problems);
        assert_eq!(config.permissions.len(), 1);
        write(&ws, "drift.json", r#"{ "permissions": [{ "kind": "bash" "pattern": "*" }] }"#);
        let broken = Config::load_with_home(&ws, None);
        assert!(broken.permissions.is_empty());
        assert!(broken.problems[0].contains("drift.json could not be read") && broken.problems[0].contains("permission rules"), "{:?}", broken.problems);
        std::fs::remove_dir_all(ws).ok();
    }
}
