//! What a workspace tells the engine: drift.json, agents, commands, skills and instruction files.

mod frontmatter;
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
const SKILL_DIRS: [&str; 3] = [".drift/skills", ".agents/skills", ".claude/skills"];
const MAX_INSTRUCTION_CHARS: usize = 40_000;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct File {
    /// Default model for new sessions.
    pub model: Option<ModelRef>,
    pub permissions: Vec<Rule>,
    /// Extra instruction files, relative to the file's directory.
    pub instructions: Vec<String>,
    /// Formatter overrides by name; `false` disables a built-in.
    pub formatters: BTreeMap<String, FormatterConfig>,
    /// Turn limits; each field set here replaces the one before it.
    pub limits: LimitsFile,
    /// Time limits per provider route (`ollama`, `anthropic`, ...), for slow local models or gateways.
    pub timeouts: BTreeMap<String, RouteTimeouts>,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub name: String,
    pub description: String,
    /// Appended to the system prompt when this agent runs.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    /// Tool names this agent may use; empty means every tool.
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
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    /// Picked in the composer to run a conversation; can also take a `task`.
    #[default]
    Primary,
    /// Only runs `task` subagents; never offered in the composer.
    Subagent,
    /// Does one engine job (titles, compaction, branch handoffs); never runs a conversation.
    Action,
}

impl AgentKind {
    /// Front matter `mode: subagent` marks a workspace agent for delegation only.
    fn from_mode(mode: Option<&str>) -> Self {
        match mode {
            Some("subagent") => Self::Subagent,
            _ => Self::Primary,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Command {
    pub name: String,
    pub description: String,
    /// The prompt; `$ARGUMENTS` is replaced with what follows the command.
    pub template: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// Directory holding SKILL.md and whatever it references.
    pub path: String,
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
    pub permissions: Vec<Rule>,
    pub agents: Vec<Agent>,
    pub commands: Vec<Command>,
    pub skills: Vec<Skill>,
    pub instructions: Vec<Instruction>,
    pub formatters: BTreeMap<String, FormatterConfig>,
    pub limits: Limits,
    pub timeouts: BTreeMap<String, RouteTimeouts>,
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
            config.apply_file(root);
            config.apply_dir(&root.join(DIR));
        }
        for root in roots.iter().rev() {
            for dir in SKILL_DIRS {
                config.add_skills(&root.join(dir));
            }
        }
        config.add_instructions(workspace);
        config
    }

    pub fn policy(&self) -> Policy {
        Policy { rules: self.permissions.clone() }
    }

    pub fn agent(&self, name: &str) -> Option<&Agent> {
        self.agents.iter().find(|a| a.name == name)
    }

    /// The model an agent is pinned to, if any; unpinned agents inherit from whatever runs them.
    pub fn agent_model(&self, name: &str) -> Option<ModelRef> {
        self.agent(name).and_then(|agent| agent.model.clone())
    }

    pub fn skill(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
    }

    fn apply_file(&mut self, root: &Path) {
        let Ok(text) = std::fs::read_to_string(root.join(FILE)) else { return };
        let Ok(file) = serde_json::from_str::<File>(&text) else { return };
        if file.model.is_some() {
            self.model = file.model;
        }
        // Later files' rules come first, so a project rule beats a home rule for the same pattern.
        let mut rules = file.permissions;
        rules.append(&mut self.permissions);
        self.permissions = rules;
        self.formatters.extend(file.formatters);
        self.timeouts.extend(file.timeouts);
        let limits = file.limits;
        self.limits.steps = limits.steps.unwrap_or(self.limits.steps).max(1);
        self.limits.repeats = limits.repeats.unwrap_or(self.limits.repeats).max(2);
        self.limits.polls = limits.polls.unwrap_or(self.limits.polls).max(2);
        for relative in file.instructions {
            if let Ok(text) = std::fs::read_to_string(root.join(&relative)) {
                self.instructions.push(Instruction { name: relative, text: clip(&text) });
            }
        }
    }

    /// A file named after an action customises that action; otherwise `mode` decides, then the agent it replaces.
    fn workspace_kind(&self, name: &str, mode: Option<&str>) -> AgentKind {
        match self.agent(name).map(|existing| existing.kind) {
            Some(AgentKind::Action) => AgentKind::Action,
            existing if mode.is_none() => existing.unwrap_or_default(),
            _ => AgentKind::from_mode(mode),
        }
    }

    fn apply_dir(&mut self, dir: &Path) {
        for (name, doc) in markdown_files(&dir.join("agents")) {
            let agent = Agent {
                description: doc.field("description").unwrap_or_default(),
                prompt: doc.body.trim().into(),
                model: doc.field("model").and_then(|m| parse_model(&m)),
                tools: doc.field("tools").map(|t| t.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()).unwrap_or_default(),
                builtin: false,
                kind: self.workspace_kind(&name, doc.field("mode").as_deref()),
                steps: doc.field("steps").and_then(|s| s.trim().parse().ok()).filter(|s: &u32| *s > 0),
                background: doc.field("background").and_then(|b| b.trim().parse().ok()),
                name: name.clone(),
            };
            self.agents.retain(|a| a.name != name);
            self.agents.push(agent);
        }
        for (name, doc) in markdown_files(&dir.join("commands")) {
            self.commands.retain(|c| c.name != name);
            self.commands.push(Command { name, description: doc.field("description").unwrap_or_default(), template: doc.body.trim().into() });
        }
    }

    fn add_skills(&mut self, dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(text) = std::fs::read_to_string(path.join("SKILL.md")) else { continue };
            let doc = frontmatter::parse(&text);
            let name = doc.field("name").unwrap_or_else(|| entry.file_name().to_string_lossy().into_owned());
            if self.skills.iter().any(|s| s.name == name) {
                continue;
            }
            self.skills.push(Skill { name, description: doc.field("description").unwrap_or_default(), path: path.to_string_lossy().into_owned() });
        }
    }

    /// The first of AGENTS.md and CLAUDE.md at the workspace root.
    fn add_instructions(&mut self, workspace: &Path) {
        for name in INSTRUCTION_FILES {
            if let Ok(text) = std::fs::read_to_string(workspace.join(name)) {
                self.instructions.push(Instruction { name: name.into(), text: clip(&text) });
                return;
            }
        }
    }
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
    };
    vec![
        agent("build", "Reads, edits and runs code.", "", &[], AgentKind::Primary),
        agent("plan", "Explores and proposes; changes nothing.", include_str!("prompts/plan.txt"), &["read", "glob", "grep", "webfetch", "question", "todowrite"], AgentKind::Primary),
        agent("general", "General-purpose subagent for multi-step work: researching, and making changes. The default for task.", include_str!("prompts/general.txt"), &[], AgentKind::Subagent),
        agent("explore", "Fast read-only subagent for finding files and code and answering questions about a codebase.", include_str!("prompts/explore.txt"), &["read", "glob", "grep", "bash", "webfetch"], AgentKind::Subagent),
        agent("title", "Names new conversations. Default model: a small one from the conversation's provider.", include_str!("prompts/title.txt"), &[], AgentKind::Action),
        agent("compaction", "Summarises long conversations to free context. Default model: the conversation's.", include_str!("prompts/compaction.txt"), &[], AgentKind::Action),
        agent("handoff", "Drafts the carried context for a /spawn branch. Default model: the source conversation's.", include_str!("prompts/handoff.txt"), &[], AgentKind::Action),
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

fn parse_model(text: &str) -> Option<ModelRef> {
    let (provider, model) = text.trim().split_once('/')?;
    Some(ModelRef { provider: provider.into(), model: model.into() })
}

fn clip(text: &str) -> String {
    if text.chars().count() <= MAX_INSTRUCTION_CHARS {
        return text.trim().into();
    }
    format!("{}\n\n(truncated)", text.chars().take(MAX_INSTRUCTION_CHARS).collect::<String>())
}

fn home() -> Option<PathBuf> {
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
    fn project_config_layers_over_home_and_discovers_everything() {
        let root = std::env::temp_dir().join(format!("drift-config-{}", crate::random_hex(4)));
        let home = root.join("home");
        let ws = root.join("ws");
        write(&home, ".config/drift/drift.json", r#"{ "model": { "provider": "anthropic", "model": "haiku" }, "permissions": [{ "kind": "bash", "pattern": "git *", "decision": "allow" }] }"#);
        write(&home, ".agents/skills/review/SKILL.md", "---\nname: review\ndescription: Reviews code\n---\nHow to review.");
        write(&ws, "drift.json", r#"{ "permissions": [{ "kind": "bash", "pattern": "git push*", "decision": "deny" }], "instructions": ["docs/rules.md"] }"#);
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
        assert_eq!(names, ["build", "general", "compaction", "handoff", "explore", "plan", "reviewer", "title"]);
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
        assert_eq!(skills, [("review", "Project review"), ("deploy", "Deploys")], "project skills shadow home skills of the same name");

        assert_eq!(config.instructions.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["docs/rules.md", "AGENTS.md"]);
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
                ("title", AgentKind::Action),
                ("compaction", AgentKind::Action),
                ("handoff", AgentKind::Action),
            ]
        );
        assert!(!config.agent("explore").unwrap().tools.contains(&"edit".to_string()), "explore is read-only");
        assert!(config.agent("plan").unwrap().tools.contains(&"read".to_string()));
        assert!(config.commands.is_empty() && config.skills.is_empty() && config.instructions.is_empty());
        std::fs::remove_dir_all(ws).ok();
    }
}
