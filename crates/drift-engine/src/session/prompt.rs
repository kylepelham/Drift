//! The system prompt: who the model is, how to work here, and what the workspace says about itself.

use std::fmt::Write as _;
use std::path::Path;

use super::types::{MessageWithParts, Part, PartRow, Role};
use crate::config::{Agent, Config};
use crate::llm::catalog::PromptFamily;

/// The rules every family's prompt keeps, whatever replaces the rest: tools, `<system-reminder>`, the worktree, the answer's shape.
const SHARED: &str = include_str!("prompts/shared.txt");

pub fn shared_rules() -> &'static str {
    SHARED.trim()
}

/// The id of the user's replacement for every model's base prompt; a family's own replacement wins over it.
pub const ALL_MODELS: &str = "all";

/// Where the user's replacement for base prompt `id` (`all` or a family) is kept.
pub fn custom_key(id: &str) -> String {
    format!("basePrompt:{id}")
}

/// What a turn on a `family` model starts with: the user's text for that family, else theirs for every model, else Drift's.
pub fn base_for(store: &crate::store::Store, family: PromptFamily) -> std::borrow::Cow<'static, str> {
    let custom = |id: &str| {
        store
            .setting::<String>(&custom_key(id))
            .ok()
            .flatten()
            .filter(|text| !text.trim().is_empty())
    };
    custom(family.as_str())
        .or_else(|| custom(ALL_MODELS))
        .map_or(family_prompt(family).into(), Into::into)
}

/// Who the model is and how it works, written for its family (`Model::prompt`).
pub fn family_prompt(family: PromptFamily) -> &'static str {
    match family {
        PromptFamily::Codex => include_str!("prompts/codex.txt"),
        PromptFamily::Claude => include_str!("prompts/claude.txt"),
        PromptFamily::Gemini => include_str!("prompts/gemini.txt"),
        PromptFamily::Default => include_str!("prompts/default.txt"),
    }
}

/// What a turn's system prompt is built from.
pub struct Setting<'a> {
    /// The model family's prompt, or the user's replacement for it; the shared rules follow it either way.
    pub base: &'a str,
    pub workspace: &'a Path,
    pub config: &'a Config,
    pub agent: Option<&'a Agent>,
    /// Whether the `task` tool is offered; only then are the subagents listed.
    pub delegates: bool,
    /// Whether the `skill` tool is offered; only then are the skills listed.
    pub loads_skills: bool,
    /// Whether a rule denies a permission (`skill`, `task`) for a name; a denied skill or subagent is not listed.
    pub denied: &'a dyn Fn(&str, &str) -> bool,
    /// The model's name as the catalog gives it.
    pub model: &'a str,
    /// Instructions from the MCP servers whose tools are offered, by server.
    pub servers: &'a [(String, String)],
}

pub fn system(setting: &Setting) -> String {
    let Setting {
        base,
        workspace,
        config,
        agent,
        delegates,
        loads_skills,
        denied,
        model,
        servers,
    } = *setting;
    let mut prompt = format!("{}\n\n{}", base.trim(), SHARED.trim());
    // A primary agent's prompt rides on its turns' prompts instead (`remind_agents`), so the system prompt stays the same across a switch.
    if let Some(agent) = agent.filter(|a| !a.prompt.is_empty() && !a.kind.runs_conversations()) {
        let _ = write!(prompt, "\n\n{}", agent.prompt);
    }
    prompt.push_str("\n\n# Environment\n\n");
    let _ = writeln!(prompt, "Working directory: {}", workspace.display());
    let _ = writeln!(
        prompt,
        "Git repository: {}",
        if crate::config::in_repository(workspace) {
            "yes"
        } else {
            "no"
        }
    );
    let _ = writeln!(prompt, "Platform: {}", std::env::consts::OS);
    let _ = writeln!(prompt, "Date: {}", crate::platform::clock::local_date());
    let _ = writeln!(prompt, "Model: {model}");
    let _ = writeln!(
        prompt,
        "Scratch directory: {} (read and write there without asking; put temporary files there, not in the workspace)",
        crate::tool::scratch_dir().display()
    );
    for (server, text) in servers {
        let _ = writeln!(prompt, "\n# Instructions from the {server} MCP server\n\n{text}");
    }
    // Only what this agent can use is listed: a skill it may load, a subagent it may delegate to.
    let skills: Vec<&crate::config::Skill> = config
        .skills
        .iter()
        .filter(|skill| loads_skills && !denied("skill", &skill.name))
        .collect();
    if !skills.is_empty() {
        prompt.push_str("\n# Skills\n\nLoad one with the `skill` tool when its description matches the task.\n\n");
        for skill in skills {
            let _ = writeln!(prompt, "- {}: {}", skill.name, skill.description);
        }
    }
    // A broken subagent would only fail when picked, so it is not offered.
    let subagents: Vec<&Agent> = config
        .agents
        .iter()
        .filter(|a| a.kind.delegated_to() && a.problem.is_none() && !denied("task", &a.name))
        .collect();
    if delegates && !subagents.is_empty() {
        prompt.push_str("\n# Subagents\n\nPass one as `subagent_type` to the `task` tool.\n\n");
        for subagent in subagents {
            let _ = writeln!(prompt, "- {}: {}", subagent.name, subagent.description);
        }
    }
    for instruction in &config.instructions {
        let _ = writeln!(
            prompt,
            "\n# Instructions from {}\n\n{}",
            instruction.name, instruction.text
        );
    }
    prompt
}
const LEFT_ORCHESTRATOR: &str = "<system-reminder>\nThe conversation has switched from the orchestrator agent to the {agent} agent. The orchestrator's protocol no longer applies: do not end replies with an <orchestrator_status> block, even though earlier replies did.\n</system-reminder>";
const LEFT_READ_ONLY: &str = "<system-reminder>\nThe conversation has switched from the {from} agent to the {agent} agent. The {from} agent's read-only limits no longer apply: you may now change files and run commands with the tools you have. Carry out the plan agreed above.\n</system-reminder>";

/// In the request only, over what the model sees (the compaction view): the prompt that starts each
/// run of a primary agent's turns carries that agent's prompt, and a prompt after a read-only agent's
/// reply, to one that writes, is told the earlier "change nothing" turns no longer bind it. Each
/// prompt keeps its reminder turn after turn, so the cached prefix stays the same, and a run of turns
/// by one agent carries its prompt once. When a summary stands for the prompt that started the run
/// still going, the returned reminders are for the summary's turn, so the agent never forgets itself.
pub(super) fn remind_agents(config: &Config, current: &str, transcript: &mut [MessageWithParts]) -> Vec<String> {
    let (summarised, shown) = {
        let view = super::compaction::view(transcript);
        let ids: std::collections::HashSet<&str> = view.messages.iter().map(|m| m.info.id.as_str()).collect();
        let shown: Vec<usize> = (0..transcript.len())
            .filter(|i| ids.contains(transcript[*i].info.id.as_str()))
            .collect();
        (view.summary.is_some(), shown)
    };
    let agent_of = |message: &MessageWithParts| message.info.agent.clone();
    for (k, &index) in shown.iter().enumerate() {
        if transcript[index].info.role != Role::User {
            continue;
        }
        let reply = || {
            shown[k + 1..]
                .iter()
                .map(|i| &transcript[*i])
                .find(|m| m.info.role == Role::Assistant)
                .and_then(agent_of)
        };
        let ran_as = agent_of(&transcript[index])
            .or_else(reply)
            .unwrap_or_else(|| current.to_string());
        let before = shown[..k]
            .iter()
            .rev()
            .map(|i| &transcript[*i])
            .find(|m| m.info.role == Role::Assistant)
            .and_then(agent_of);
        let reminders = reminders(config, &ran_as, before.as_deref());
        let message = &mut transcript[index];
        for (at, text) in reminders.into_iter().enumerate() {
            message.parts.insert(
                at,
                PartRow {
                    id: String::new(),
                    message_id: message.info.id.clone(),
                    session_id: message.info.session_id.clone(),
                    provider_signature: None,
                    part: Part::Text { text },
                },
            );
        }
    }
    let opens_with_prompt = shown.first().is_some_and(|i| transcript[*i].info.role == Role::User);
    if !summarised || opens_with_prompt {
        return Vec::new();
    }
    let running = shown
        .first()
        .and_then(|i| agent_of(&transcript[*i]))
        .unwrap_or_else(|| current.to_string());
    agent_prompt(config, &running).into_iter().collect()
}

/// What a prompt run as `agent`, after a reply by `before`, is reminded of.
fn reminders(config: &Config, agent: &str, before: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let read_only = |name: &str| config.agent(name).is_some_and(|found| found.read_only);
    if let Some(from) = before.filter(|from| *from != agent && read_only(from) && !read_only(agent)) {
        out.push(LEFT_READ_ONLY.replace("{from}", from).replace("{agent}", agent));
    }
    if before.is_some_and(|from| from == super::drive::AGENT && agent != super::drive::AGENT) {
        out.push(LEFT_ORCHESTRATOR.replace("{agent}", agent));
    }
    if before != Some(agent) {
        out.extend(agent_prompt(config, agent));
    }
    out
}

/// A primary agent's prompt as a reminder; subagents carry theirs in the system prompt.
fn agent_prompt(config: &Config, agent: &str) -> Option<String> {
    let found = config
        .agent(agent)
        .filter(|found| found.kind.runs_conversations() && !found.prompt.is_empty())?;
    Some(format!("<system-reminder>\n{}\n</system-reminder>", found.prompt))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE_DENIED: fn(&str, &str) -> bool = |_, _| false;

    #[test]
    fn includes_identity_environment_and_agents_file() {
        let workspace = std::env::temp_dir().join(format!("drift-prompt-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("CLAUDE.md"), "claude rules").unwrap();
        std::fs::write(workspace.join("AGENTS.md"), "agent rules").unwrap();
        let config = Config::load_with_home(&workspace, None);
        let servers = [("web-test".to_string(), "Start a session first.".to_string())];
        let setting = |agent: &str, delegates: bool| Setting {
            base: family_prompt(PromptFamily::Claude),
            workspace: &workspace,
            config: &config,
            agent: config.agent(agent),
            delegates,
            loads_skills: true,
            denied: &NONE_DENIED,
            model: "Claude Opus",
            servers: &servers,
        };
        let prompt = system(&setting("plan", false));
        assert!(prompt.starts_with("You are Drift"));
        assert!(
            !prompt.contains("# Plan mode"),
            "a primary agent's prompt rides on its prompts, not here"
        );
        assert_eq!(
            prompt,
            system(&setting("build", false)),
            "plan and build share one system prompt, so a switch keeps the cache"
        );
        assert!(
            prompt.contains("Working directory: ")
                && prompt.contains("Git repository: no\n")
                && prompt.contains("Model: Claude Opus\n")
        );
        assert!(prompt.contains("# Instructions from the web-test MCP server\n\nStart a session first."));
        assert!(prompt.contains("# Instructions from AGENTS.md\n\nagent rules"));
        assert!(!prompt.contains("claude rules"));
        assert!(!prompt.contains("# Subagents"), "no task tool, no subagent list");
        std::fs::create_dir_all(workspace.join(".git")).unwrap();
        assert!(system(&setting("plan", false)).contains("Git repository: yes\n"));
        let delegating = system(&setting("build", true));
        assert!(
            delegating.contains("# Subagents")
                && delegating.contains("- general: ")
                && delegating.contains("- explore: ")
        );
        assert!(!delegating.contains("- title: "), "actions are not subagents");
        let mut broken = config.clone();
        broken.agents.iter_mut().find(|a| a.name == "explore").unwrap().problem =
            Some("uses unsupported controls (temperature)".into());
        let offered = system(&Setting {
            config: &broken,
            agent: broken.agent("build"),
            ..setting("build", true)
        });
        assert!(
            offered.contains("- general: ") && !offered.contains("- explore: "),
            "a broken subagent is not offered"
        );
        std::fs::remove_dir_all(workspace).ok();
    }

    #[test]
    fn only_skills_and_subagents_the_agent_can_use_are_listed() {
        let mut config = Config::load_with_home(&std::env::temp_dir().join("drift-no-such-ws"), None);
        let skill = |name: &str| crate::config::Skill {
            name: name.into(),
            description: format!("{name} skill"),
            path: String::new(),
            instructions: String::new(),
            argument_hint: None,
        };
        config.skills = vec![skill("review"), skill("deploy")];
        let workspace = std::env::temp_dir();
        let deny = |kind: &str, name: &str| (kind, name) == ("skill", "deploy") || (kind, name) == ("task", "explore");
        let built = |loads_skills: bool, denied: &dyn Fn(&str, &str) -> bool| {
            system(&Setting {
                base: "b",
                workspace: &workspace,
                config: &config,
                agent: None,
                delegates: true,
                loads_skills,
                denied,
                model: "m",
                servers: &[],
            })
        };
        let all = built(true, &NONE_DENIED);
        assert!(all.contains("- review: ") && all.contains("- deploy: ") && all.contains("- explore: "));
        let ruled = built(true, &deny);
        assert!(
            ruled.contains("- review: ") && !ruled.contains("- deploy: "),
            "a denied skill is not offered"
        );
        assert!(
            ruled.contains("- general: ") && !ruled.contains("- explore: "),
            "nor a subagent a task rule denies"
        );
        assert!(
            !built(false, &NONE_DENIED).contains("# Skills"),
            "no skill tool, no skill list"
        );
    }

    #[test]
    fn every_family_has_its_own_prompt_and_all_keep_the_shared_rules() {
        let config = Config::default();
        let workspace = std::env::temp_dir();
        let built = |family| {
            system(&Setting {
                base: family_prompt(family),
                workspace: &workspace,
                config: &config,
                agent: None,
                delegates: false,
                loads_skills: true,
                denied: &NONE_DENIED,
                model: "m",
                servers: &[],
            })
        };
        let prompts: Vec<String> = PromptFamily::ALL.into_iter().map(built).collect();
        for (family, prompt) in PromptFamily::ALL.into_iter().zip(&prompts) {
            assert!(prompt.starts_with("You are Drift"), "{family:?}");
            for rule in [
                "`<system-reminder>` blocks",
                "Never revert them unless asked",
                "Reference code as `path:line`",
                "Do not paste large files you wrote",
            ] {
                assert!(prompt.contains(rule), "{family:?} keeps `{rule}`");
            }
        }
        assert_eq!(
            prompts.iter().collect::<std::collections::HashSet<_>>().len(),
            4,
            "each family reads differently"
        );
        assert!(
            prompts[0].contains("`apply_patch`") && !prompts[0].contains("`edit` requires"),
            "Codex edits with the tool it is offered"
        );
        let replaced = system(&Setting {
            base: "You are my agent.",
            workspace: &workspace,
            config: &config,
            agent: None,
            delegates: false,
            loads_skills: true,
            denied: &NONE_DENIED,
            model: "m",
            servers: &[],
        });
        assert!(
            replaced.starts_with("You are my agent.\n\n# Tools") && replaced.contains("`<system-reminder>` blocks"),
            "a replacement keeps the shared rules"
        );
    }

    #[test]
    fn bundled_prompts_hold_only_their_own_text() {
        assert!(
            SHARED
                .trim_end()
                .ends_with("Do not paste large files you wrote; name their paths."),
            "the shared rules end with the Output section"
        );
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut checked = 0;
        for dir in ["session/prompts", "tool/prompts", "config/prompts"] {
            for entry in std::fs::read_dir(root.join(dir)).unwrap().flatten() {
                let text = std::fs::read_to_string(entry.path()).unwrap();
                for stray in [
                    "<invoke",
                    "</invoke>",
                    "<parameter",
                    "</parameter>",
                    "</content>",
                    "antml:",
                ] {
                    assert!(
                        !text.contains(stray),
                        "{} holds `{stray}`, which is not prompt text",
                        entry.path().display()
                    );
                }
                checked += 1;
            }
        }
        assert!(checked > 10, "every prompt directory was read");
    }

    #[test]
    fn each_prompt_keeps_the_reminder_of_the_agent_its_turn_ran_as() {
        let config = Config::load_with_home(&std::env::temp_dir().join("drift-prompt-none"), None);
        let mut transcript = numbered(vec![
            message(Role::User, None),
            message(Role::Assistant, Some("plan")),
            message(Role::User, None),
            message(Role::Assistant, Some("build")),
            message(Role::User, None),
        ]);
        assert!(
            remind_agents(&config, "build", &mut transcript).is_empty(),
            "nothing summarised, nothing to lead with"
        );
        assert!(
            texts(&transcript[0]).contains("# Plan mode"),
            "the planning turn's prompt keeps plan's reminder"
        );
        assert!(
            texts(&transcript[2]).contains("switched from the plan agent to the build agent")
                && !texts(&transcript[2]).contains("# Plan mode")
        );
        assert!(texts(&transcript[4]).is_empty(), "build after build: nothing");

        let mut left = numbered(vec![
            message(Role::User, Some("orchestrator")),
            message(Role::Assistant, Some("orchestrator")),
            message(Role::User, Some("plan")),
            message(Role::Assistant, Some("plan")),
            message(Role::User, Some("build")),
        ]);
        remind_agents(&config, "build", &mut left);
        assert!(
            texts(&left[2]).contains("<orchestrator_status> block"),
            "leaving the orchestrator ends its protocol"
        );
        assert!(
            !texts(&left[4]).contains("orchestrator"),
            "said once, where it was left"
        );

        let mut run = numbered(vec![
            message(Role::User, Some("plan")),
            message(Role::Assistant, Some("plan")),
            message(Role::User, Some("plan")),
            message(Role::Assistant, Some("plan")),
        ]);
        remind_agents(&config, "plan", &mut run);
        assert!(
            texts(&run[0]).contains("# Plan mode") && texts(&run[2]).is_empty(),
            "a run of plan turns carries plan's prompt once"
        );
    }

    #[test]
    fn a_compacted_prompt_keeps_its_agent_reminder() {
        let config = Config::load_with_home(&std::env::temp_dir().join("drift-prompt-none"), None);
        let mut boundary = message(Role::User, None);
        boundary.parts.push(PartRow {
            id: String::new(),
            message_id: String::new(),
            session_id: String::new(),
            provider_signature: None,
            part: Part::Compaction {
                auto: true,
                tail_from: None,
            },
        });
        let mut summary = message(Role::Assistant, None);
        summary.info.summary = true;
        summary.parts.push(PartRow {
            id: String::new(),
            message_id: String::new(),
            session_id: String::new(),
            provider_signature: None,
            part: Part::Text {
                text: "what happened".into(),
            },
        });
        let mut compacted = numbered(vec![
            message(Role::User, Some("plan")),
            message(Role::Assistant, Some("plan")),
            boundary,
            summary,
            message(Role::Assistant, Some("plan")),
        ]);
        let lead = remind_agents(&config, "plan", &mut compacted);
        assert!(
            lead.len() == 1 && lead[0].contains("# Plan mode"),
            "the prompt was summarised away, so the summary's turn carries plan's reminder: {lead:?}"
        );
        let target = super::super::types::ModelRef {
            provider: "anthropic".into(),
            model: "claude".into(),
        };
        let sent = format!(
            "{:?}",
            super::super::compaction::request_messages(&compacted, &target, &lead)[0]
        );
        assert!(sent.contains("what happened") && sent.contains("# Plan mode"), "{sent}");
    }

    fn message(role: Role, agent: Option<&str>) -> MessageWithParts {
        MessageWithParts {
            info: crate::session::types::Message {
                id: String::new(),
                session_id: String::new(),
                role,
                status: crate::session::types::MessageStatus::Done,
                model: None,
                agent: agent.map(str::to_string),
                usage: Default::default(),
                cost: 0.0,
                error: None,
                created_at: 0,
                finished_at: None,
                summary: false,
                ending: None,
            },
            parts: Vec::new(),
        }
    }

    fn texts(message: &MessageWithParts) -> String {
        message
            .parts
            .iter()
            .filter_map(|row| match &row.part {
                Part::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("|")
    }

    fn numbered(mut messages: Vec<MessageWithParts>) -> Vec<MessageWithParts> {
        for (index, message) in messages.iter_mut().enumerate() {
            message.info.id = format!("msg_{index:02}");
        }

        messages
    }
}
