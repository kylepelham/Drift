//! What a workspace tells the engine: drift.json, agents, commands, skills and instruction files.

mod agent;
mod arguments;
mod command;
mod discovery;
mod engine;
mod file;
mod frontmatter;
pub mod jsonc;
mod overrides;
pub mod plugins;
mod project_commands;
pub mod skills;
pub mod sources;
mod user;

#[cfg(test)]
mod errors_tests;
#[cfg(test)]
mod tests;

pub use agent::{Agent, AgentError, AgentKind};
pub use command::{Command, highest_placeholder, split_arguments};
pub(crate) use discovery::{HOME_SKILL_DIRS, skill_files, skill_folders};
pub use discovery::{body, home, in_repository, nested_instructions};
pub use engine::RegistryError;
pub use file::{
    CheckConfig, File, FormatterConfig, Limits, LimitsFile, LspConfig, ProviderConfig, ProviderModel, RouteTimeouts,
};
pub use overrides::{AgentOverride, ModelPin};
pub use user::{user_plugins, user_providers};

use crate::permission::{Policy, Rule};
use crate::session::types::ModelRef;
use discovery::{ancestors_to_repo_root, clip, instruction_file, instruction_files, markdown_files};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use utoipa::ToSchema;

pub const FILE: &str = "drift.json";
const DIR: &str = ".drift";

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
        if let Some(configured) = self.timeouts.get(provider) {
            if let Some(seconds) = configured.headers_seconds.filter(|seconds| *seconds > 0) {
                timeouts.headers = std::time::Duration::from_secs(seconds);
            }
            if let Some(seconds) = configured.idle_seconds.filter(|seconds| *seconds > 0) {
                timeouts.idle = std::time::Duration::from_secs(seconds);
            }
        }

        timeouts
    }

    /// The limits a turn run by `agent` works under.
    pub fn limits_for(&self, agent: &str) -> Limits {
        let steps = self
            .agent(agent)
            .and_then(|agent| agent.steps)
            .unwrap_or(self.limits.steps);

        Limits { steps, ..self.limits }
    }

    pub fn load(workspace: &Path) -> Self {
        Self::load_with_home(workspace, home().as_deref())
    }

    /// As [`Self::load`], leaving out the skills in folders the user switched off.
    pub fn load_skipping(workspace: &Path, off: &[PathBuf]) -> Self {
        Self::load_from(workspace, home().as_deref(), off)
    }

    pub fn load_with_home(workspace: &Path, home: Option<&Path>) -> Self {
        Self::load_from(workspace, home, &[])
    }

    fn load_from(workspace: &Path, home: Option<&Path>, off: &[PathBuf]) -> Self {
        let mut config = Self {
            agents: builtin_agents(),
            ..Self::default()
        };
        let mut roots = Vec::new();
        if let Some(home) = home {
            roots.push(home.join(".config").join("drift"));
        }
        roots.push(workspace.to_path_buf());

        for root in &roots {
            config.apply_file(root, root == workspace, home);
            // Workspace definitions live under .drift; home definitions sit beside the user's drift.json.
            let definitions = if root == workspace {
                root.join(DIR)
            } else {
                root.clone()
            };
            config.apply_dir(&definitions);
        }

        let listed = std::mem::take(&mut config.skill_paths);
        for directory in skill_folders(workspace, home, listed) {
            config.add_skills(&directory, off);
        }
        config.add_skill_commands();
        config.add_instructions(workspace, home);

        config
    }

    /// Each skill is a command unless one already has its name. A command that calls exactly one skill
    /// (`skill({ name: "design" })`, under any name) offers that skill's choices with its own template.
    fn add_skill_commands(&mut self) {
        for command in self.commands.iter_mut().filter(|command| command.server.is_none()) {
            let referenced = arguments::referenced_skill(&command.template);
            if let Some(skill) = referenced.and_then(|name| self.skills.iter().find(|skill| skill.name == name)) {
                command.document(skill);
            }
        }

        for skill in &self.skills {
            if self.commands.iter().any(|command| command.name == skill.name) {
                continue;
            }

            let mut command = Command::new(
                skill.name.clone(),
                skill.description.clone(),
                skill.instructions.clone(),
            );
            command.skill = Some(skill.name.clone());
            command.document(skill);
            self.commands.push(command);
        }
    }

    pub fn policy(&self) -> Policy {
        Policy {
            rules: self.permissions.clone(),
        }
    }

    /// An agent's rules, from its file or Settings, are kept as written and resolve as in opencode: the last match wins.
    pub fn agent_policy(&self, agent: &str) -> Policy {
        let rules = self
            .agent(agent)
            .map(|agent| agent.permissions.iter().rev().cloned().collect())
            .unwrap_or_default();

        Policy { rules }
    }

    pub fn agent(&self, name: &str) -> Option<&Agent> {
        self.agents.iter().find(|agent| agent.name == name)
    }

    /// The agent a new session runs as: drift.json's `defaultAgent` when it names one that runs
    /// conversations, else `build`, else (`build` disabled) the first visible one that does, as opencode falls back.
    pub fn default_agent(&self) -> &str {
        let runs = |agent: &&Agent| agent.kind.runs_conversations() && agent.problem.is_none();
        let named = self
            .default_agent
            .as_deref()
            .and_then(|name| self.agent(name))
            .filter(runs);
        let fallback = || {
            self.agent("build")
                .filter(runs)
                .or_else(|| self.agents.iter().filter(|agent| !agent.hidden).find(runs))
        };

        named.or_else(fallback).map_or("build", |agent| agent.name.as_str())
    }

    /// The model an agent is pinned to, if any; unpinned agents inherit from whatever runs them.
    pub fn agent_model(&self, name: &str) -> Option<ModelRef> {
        self.agent(name).and_then(|agent| agent.model.clone())
    }

    pub fn skill(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|skill| skill.name == name)
    }

    fn apply_file(&mut self, root: &Path, project: bool, home: Option<&Path>) {
        let path = root.join(FILE);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        let file = match serde_json::from_str::<File>(&jsonc::strip(&text)) {
            Ok(file) => file,
            Err(error) => {
                let problem = format!(
                    concat!(
                        "{} could not be read ({}), so none of its settings or permission rules apply; ",
                        "fix it to carry on"
                    ),
                    path.display(),
                    error,
                );
                self.problems.push(problem);
                return;
            }
        };

        if file.model.is_some() {
            self.model = file.model;
        }
        if file.default_agent.is_some() {
            self.default_agent = file.default_agent;
        }
        // Project rules precede home rules so the more specific configuration wins.
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
        self.limits.steps = file.limits.steps.unwrap_or(self.limits.steps).max(1);
        self.limits.repeats = file.limits.repeats.unwrap_or(self.limits.repeats).max(2);
        self.limits.polls = file.limits.polls.unwrap_or(self.limits.polls).max(2);

        let resolve = |listed: &String| match (listed.strip_prefix("~/"), home) {
            (Some(relative), Some(home)) => home.join(relative),
            _ => root.join(listed),
        };
        for listed in &file.instructions {
            for (name, path) in instruction_files(listed, &resolve(listed)) {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    self.instructions.push(Instruction {
                        name,
                        text: clip(&text),
                    });
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
                LspConfig::Custom { .. } if project => {
                    let warning = format!(
                        concat!(
                            "lsp {}: a project's drift.json can only turn a language server off; ",
                            "set its command in ~/.config/drift/drift.json"
                        ),
                        name
                    );
                    self.warnings.push(warning);
                }
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
        for (name, document) in markdown_files(&dir.join("agents")) {
            self.apply_agent(name, document);
        }

        for (name, document) in markdown_files(&dir.join("commands")) {
            self.commands.retain(|command| command.name != name);
            let mut command = Command::new(
                name,
                document.field("description").unwrap_or_default(),
                document.body.trim().into(),
            );
            command.agent = document.field("agent");
            command.model = document.field("model").and_then(|model| parse_model(&model));
            command.subtask = document.field("subtask").and_then(|value| value.parse().ok());
            command.arguments = (1..=highest_placeholder(&command.template))
                .filter(|number| command.template.contains(&format!("${number}")))
                .map(|number| format!("arg{number}"))
                .collect();
            self.commands.push(command);
        }
    }

    fn apply_agent(&mut self, name: String, document: frontmatter::Document) {
        // Engine actions cannot be disabled because title generation and compaction depend on them.
        if document.field("disable").is_some_and(|value| value.trim() == "true") {
            if self.agent(&name).is_some_and(|agent| agent.kind == AgentKind::Action) {
                self.warnings
                    .push(format!("agent {name}: disable ignored; Drift needs it for {name}s"));
            } else {
                self.agents.retain(|agent| agent.name != name);
            }
            return;
        }

        let ignored: Vec<&str> = ["temperature", "top_p", "topP", "options", "provider_options"]
            .into_iter()
            .filter(|key| document.fields.contains_key(*key))
            .collect();
        if !ignored.is_empty() {
            self.warnings.push(format!(
                "agent {name}: {} ignored; Drift uses the model's own sampling",
                ignored.join(", ")
            ));
        }

        let (permissions, problem) = match document.permissions() {
            Ok(rules) => (rules, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
        let agent = Agent {
            description: document.field("description").unwrap_or_default(),
            prompt: document.body.trim().into(),
            model: document.field("model").and_then(|model| parse_model(&model)),
            tools: agent_tools(&document),
            builtin: false,
            kind: self.workspace_kind(&name, document.field("mode").as_deref()),
            steps: document
                .field("steps")
                .and_then(|steps| steps.trim().parse().ok())
                .filter(|steps: &u32| *steps > 0),
            background: document.field("background").and_then(|value| value.trim().parse().ok()),
            read_only: document.field("read_only").is_some_and(|value| value.trim() == "true"),
            name: name.clone(),
            permissions,
            variant: document.field("variant"),
            problem,
            hidden: document.field("hidden").is_some_and(|value| value.trim() == "true"),
        };

        self.agents.retain(|agent| agent.name != name);
        self.agents.push(agent);
    }

    /// Every `SKILL.md` under `dir`, at any depth up to [`discovery::SKILL_DEPTH`]; a name already found nearer wins.
    fn add_skills(&mut self, dir: &Path, off: &[PathBuf]) {
        for path in skill_files(dir) {
            let folder = path.parent().unwrap_or(dir);
            if off.contains(&crate::tool::canonical(folder)) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let document = frontmatter::parse(&text);
            let name = document
                .field("name")
                .unwrap_or_else(|| folder.file_name().unwrap_or_default().to_string_lossy().into_owned());
            if self.skills.iter().any(|skill| skill.name == name) {
                continue;
            }

            let instructions = body(&text);
            let argument_hint = document.field("argument-hint").filter(|hint| !hint.trim().is_empty());
            self.skills.push(Skill {
                name,
                description: document.field("description").unwrap_or_default(),
                path: folder.to_string_lossy().into_owned(),
                instructions,
                argument_hint,
            });
        }
    }

    /// The user's own file, then each directory's from the repository root down to the workspace,
    /// before what drift.json lists: general rules first, the most specific last.
    fn add_instructions(&mut self, workspace: &Path, home: Option<&Path>) {
        let mut found = Vec::new();
        if let Some(home) = home {
            let global = [
                (home.join(".config/drift/AGENTS.md"), "~/.config/drift/AGENTS.md"),
                (home.join(".claude/CLAUDE.md"), "~/.claude/CLAUDE.md"),
            ];
            if let Some((text, name)) = global
                .iter()
                .find_map(|(path, name)| std::fs::read_to_string(path).ok().map(|text| (text, *name)))
            {
                found.push(Instruction {
                    name: name.into(),
                    text: clip(&text),
                });
            }
        }

        let chain = ancestors_to_repo_root(workspace);
        for (depth, dir) in chain.iter().enumerate().rev() {
            if let Some((name, text)) = instruction_file(dir) {
                let shown = format!("{}{name}", "../".repeat(depth));
                found.push(Instruction {
                    name: shown,
                    text: clip(&text),
                });
            }
        }

        found.append(&mut self.instructions);
        self.instructions = found;
    }
}

/// An agent file's tools. An explicit empty list (`tools: []`) means none, written `!*`; leaving
/// the field out, or an empty map (`tools: {}`), means every tool.
fn agent_tools(document: &frontmatter::Document) -> Vec<String> {
    let tools = document.list("tools").unwrap_or_default();
    let empty_list = document.field("tools").is_some_and(|raw| raw.replace(' ', "") == "[]");
    if tools.is_empty() && empty_list {
        return vec!["!*".into()];
    }

    tools
}

fn builtin_agents() -> Vec<Agent> {
    let agent = |name: &str, description: &str, prompt: &str, tools: &[&str], kind| Agent {
        name: name.into(),
        description: description.into(),
        prompt: prompt.trim().into(),
        model: None,
        tools: tools.iter().map(ToString::to_string).collect(),
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
    let read_only = |agent: Agent| Agent {
        read_only: true,
        ..agent
    };

    vec![
        agent("build", "Reads, edits and runs code.", "", &[], AgentKind::Primary),
        // Plan keeps build's tools to preserve the cached prefix, but refuses mutating calls.
        read_only(agent(
            "plan",
            "Explores and proposes; changes nothing.",
            include_str!("prompts/plan.txt"),
            &[],
            AgentKind::Primary,
        )),
        agent(
            "general",
            "General-purpose subagent for multi-step work: researching, and making changes. The default for task.",
            include_str!("prompts/general.txt"),
            &[],
            AgentKind::Subagent,
        ),
        read_only(agent(
            "explore",
            "Fast read-only subagent for finding files and code and answering questions about a codebase.",
            include_str!("prompts/explore.txt"),
            &["read", "glob", "grep", "bash", "webfetch", "skill"],
            AgentKind::Subagent,
        )),
        // Orchestrator delegates mutations and therefore receives no tools that write or run code directly.
        agent(
            "orchestrator",
            "Drives a goal to completion by delegating to subagents, verifying results, and correcting course",
            include_str!("prompts/orchestrator.txt"),
            &[
                "read",
                "glob",
                "grep",
                "webfetch",
                "todowrite",
                "skill",
                "question",
                "task",
                "task_output",
                "task_stop",
                "read_thread",
            ],
            AgentKind::Primary,
        ),
        agent(
            "title",
            "Names new conversations. Default model: a small one from the conversation's provider.",
            include_str!("prompts/title.txt"),
            &[],
            AgentKind::Action,
        ),
        agent(
            "compaction",
            "Summarises long conversations to free context. Default model: the conversation's.",
            include_str!("prompts/compaction.txt"),
            &[],
            AgentKind::Action,
        ),
    ]
}

pub(crate) fn parse_model(text: &str) -> Option<ModelRef> {
    let (provider, model) = text.trim().split_once('/')?;

    Some(ModelRef {
        provider: provider.into(),
        model: model.into(),
    })
}
