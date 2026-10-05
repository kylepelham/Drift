//! User commands retain execution settings and use the existing permission and worker machinery.

use std::sync::Arc;
use serde_json::{json, Value};

use super::turn::{Admission, Prompt, Receipt, TurnError};
use super::types::{ModelRef, Part};
use crate::config::AgentKind;
use crate::Engine;

#[derive(Clone)]
pub(crate) struct Bootstrap {
    pub tool: String,
    pub input: Value,
    pub command: String,
    /// The command's model for a delegated task; carried in the call's metadata, never in its input.
    pub model: Option<ModelRef>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::turn::tests::{harness, model, text, until_idle};
    use crate::session::types::ToolStatus;

    #[tokio::test]
    async fn a_commands_agent_and_model_run_that_turn_only() {
        let h = harness().await;
        h.engine.store.update_session(&h.session.id, None, Some(&model()), None).unwrap();
        std::fs::create_dir_all(h._dir.join("ws/.drift/commands")).unwrap();
        std::fs::write(h._dir.join("ws/.drift/commands/check.md"), "---\nagent: plan\nmodel: anthropic/claude-haiku-4-5\nsubtask: false\n---\nReview $1 and $2.").unwrap();
        h.provider.push(text("reviewed")).push(text("next"));
        h.engine.execute_command(&h.session.id, "check", "src tests", None).await.unwrap();
        until_idle(&h).await;
        let session = h.engine.store.session(&h.session.id).unwrap().unwrap();
        assert_eq!((session.agent.as_str(), session.model.as_ref().map(|m| m.model.as_str())), ("build", Some(model().model.as_str())), "the session keeps its own");
        let transcript = h.engine.store.transcript(&h.session.id).unwrap();
        assert_eq!(transcript[1].info.agent.as_deref(), Some("plan"), "the command's reply ran as its agent");
        h.engine.submit(&h.session.id, crate::session::turn::tests::prompt("carry on")).await.unwrap();
        until_idle(&h).await;
        let requests = h.provider.requests.lock().unwrap();
        assert_eq!(requests[0].model, "claude-haiku-4-5");
        assert!(format!("{:?}", requests[0].messages).contains("Review src and tests."));
        assert_eq!(requests[1].model, model().model, "the next prompt is back on the session's model");
    }

    #[tokio::test]
    async fn shell_lines_run_as_checked_calls_and_at_files_are_mentioned() {
        let h = harness().await;
        h.engine.store.update_session(&h.session.id, None, Some(&model()), None).unwrap();
        let ask_publish = crate::permission::Rule { kind: "bash".into(), pattern: "cargo publish*".into(), decision: crate::permission::Decision::Ask };
        h.engine.permissions.set_policy(crate::permission::Policy { rules: vec![ask_publish] });
        std::fs::create_dir_all(h._dir.join("ws/.drift/commands")).unwrap();
        std::fs::write(h._dir.join("ws/NOTES.md"), "NOTE BODY").unwrap();
        std::fs::write(h._dir.join("ws/.drift/commands/brief.md"), "Read @NOTES.md, then look at !`echo SHELL OUTPUT` and !`cargo publish`.").unwrap();
        h.provider.push(text("briefed"));
        let mut rx = h.engine.hub.attach(None).rx;
        h.engine.execute_command(&h.session.id, "brief", "", None).await.unwrap();
        let ask = loop {
            let envelope = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
            if let crate::event::Event::PermissionAsked { request } = envelope.event {
                break request;
            }
        };
        assert_eq!(ask.ask.pattern, "cargo publish", "the line a rule asks about asks; the other ran without asking");
        h.engine.permissions.reply(&h.engine.hub, &ask.id, crate::permission::ReplyBody { reply: crate::permission::Reply::Deny, pattern: None, message: None }).unwrap();
        until_idle(&h).await;
        let sent = format!("{:?}", h.provider.requests.lock().unwrap()[0].messages);
        assert!(sent.contains("`echo SHELL OUTPUT` (its output follows)") && sent.contains("ran `echo SHELL OUTPUT`, which returned:\\nSHELL OUTPUT"), "{sent}");
        assert!(sent.contains("ran `cargo publish`, which failed"), "a refused line says so: {sent}");
        assert!(sent.contains("NOTE BODY"), "@NOTES.md was read in as a mention");
        std::fs::write(h._dir.join("ws/.drift/commands/away.md"), "---\nsubtask: true\n---\nCheck !`git status`.").unwrap();
        assert!(matches!(h.engine.execute_command(&h.session.id, "away", "", None).await, Err(CommandError::Invalid(_))));
    }

    #[tokio::test]
    async fn a_prompt_steered_into_a_command_turn_is_answered_as_the_session_not_the_command() {
        use std::time::Duration;
        let h = harness().await;
        h.engine.store.update_session(&h.session.id, None, Some(&model()), None).unwrap();
        std::fs::create_dir_all(h._dir.join("ws/.drift/commands")).unwrap();
        std::fs::write(h._dir.join("ws/.drift/commands/check.md"), "---\nagent: plan\nmodel: anthropic/claude-haiku-4-5\n---\nReview it.").unwrap();
        h.provider.push_slow(Duration::from_millis(600), text("reviewed")).push(text("edited"));
        h.engine.execute_command(&h.session.id, "check", "", None).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut steered = crate::session::turn::tests::prompt("now edit it");
        steered.model = None;
        h.engine.submit(&h.session.id, steered).await.unwrap();
        until_idle(&h).await;
        let requests = h.provider.requests.lock().unwrap().clone();
        assert_eq!((requests[0].model.as_str(), requests[1].model.as_str()), ("claude-haiku-4-5", model().model.as_str()), "the steered prompt runs on the session's model");
        let transcript = h.engine.store.transcript(&h.session.id).unwrap();
        assert_eq!(transcript.last().unwrap().info.agent.as_deref(), Some("build"), "and as the session's agent, not the command's read-only one");
        let session = h.engine.store.session(&h.session.id).unwrap().unwrap();
        assert_eq!((session.agent.as_str(), session.model.unwrap().model), ("build", model().model));
    }

    #[tokio::test]
    async fn a_broken_primary_agent_cannot_be_picked_mid_turn() {
        use std::time::Duration;
        let h = harness().await;
        std::fs::create_dir_all(h._dir.join("ws/.drift/agents")).unwrap();
        std::fs::write(h._dir.join("ws/.drift/agents/hot.md"), "---\nmode: primary\npermission: { bash: often }\n---\nRuns hot.").unwrap();
        std::fs::write(h._dir.join("ws/.drift/agents/warm.md"), "---\nmode: primary\ntop_p: 0.5\ntemperature: 0.9\n---\nRuns warm.").unwrap();
        let config = h.engine.workspace_config(&h._dir.join("ws"));
        assert!(config.agent("warm").unwrap().usable().is_ok(), "sampling fields are ignored, not refused");
        assert!(config.warnings.iter().any(|w| w.contains("agent warm") && w.contains("top_p") && w.contains("temperature")), "{:?}", config.warnings);
        h.provider.push_slow(Duration::from_millis(600), text("busy")).push(text("never"));
        h.engine.submit(&h.session.id, crate::session::turn::tests::prompt("start")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut switch = crate::session::turn::tests::prompt("as hot");
        switch.agent = Some("hot".into());
        let refused = h.engine.submit(&h.session.id, switch).await.unwrap_err();
        assert!(matches!(&refused, TurnError::Config(reason) if reason.contains("agent hot") && reason.contains("allow, ask or deny")), "{refused:?}");
        until_idle(&h).await;
        assert_eq!(h.engine.store.session(&h.session.id).unwrap().unwrap().agent, "build");
    }

    #[tokio::test]
    async fn a_command_naming_a_subagent_always_delegates_and_broken_agents_are_refused_alone() {
        let h = harness().await;
        h.engine.store.update_session(&h.session.id, None, Some(&model()), None).unwrap();
        std::fs::create_dir_all(h._dir.join("ws/.drift/commands")).unwrap();
        std::fs::create_dir_all(h._dir.join("ws/.drift/agents")).unwrap();
        std::fs::write(h._dir.join("ws/.drift/commands/look.md"), "---\nagent: explore\nsubtask: false\n---\nLook at $ARGUMENTS.").unwrap();
        std::fs::write(h._dir.join("ws/.drift/agents/hot.md"), "---\nmode: subagent\npermission: { read: sometimes }\n---\nRuns hot.").unwrap();
        std::fs::write(h._dir.join("ws/.drift/commands/heat.md"), "---\nagent: hot\n---\nHeat $ARGUMENTS.").unwrap();
        h.provider.push(text("found")).push(text("done"));
        h.engine.execute_command(&h.session.id, "look", "src", None).await.unwrap();
        until_idle(&h).await;
        assert_eq!(h.engine.store.tasks_of(&h.session.id).unwrap()[0].agent, "explore");
        assert_eq!(h.engine.store.session(&h.session.id).unwrap().unwrap().agent, "build");
        let refused = h.engine.execute_command(&h.session.id, "heat", "src", None).await.unwrap_err();
        assert!(matches!(&refused, CommandError::Turn(TurnError::Config(reason)) if reason.contains("agent hot") && reason.contains("allow, ask or deny")), "{refused:?}");
    }

    #[tokio::test]
    async fn subtask_commands_use_owned_foreground_workers_and_textual_parent_results() {
        let h = harness().await;
        h.engine.store.update_session(&h.session.id, None, Some(&model()), None).unwrap();
        std::fs::create_dir_all(h._dir.join("ws/.drift/commands")).unwrap();
        std::fs::write(h._dir.join("ws/.drift/commands/inspect.md"), "---\nagent: explore\nmodel: anthropic/claude-haiku-4-5\nsubtask: true\n---\nInspect $ARGUMENTS.").unwrap();
        h.provider.push(text("worker answer")).push(text("parent answer"));
        h.engine.execute_command(&h.session.id, "inspect", "src", None).await.unwrap();
        until_idle(&h).await;
        let tasks = h.engine.store.tasks_of(&h.session.id).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].agent, "explore");
        assert_eq!(tasks[0].mode, crate::session::tasks::Mode::Foreground);
        let requests = h.provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].model, "claude-haiku-4-5", "the command's model reaches the worker through metadata");
        assert!(requests[0].tools.iter().chain(&requests[1].tools).filter(|tool| tool.name == "task").all(|tool| tool.input_schema["properties"].get("model").is_none()), "the model cannot pick one");
        assert_eq!(requests[1].model, model().model);
        assert!(requests[1].messages.iter().flat_map(|message| &message.blocks).all(|block| !matches!(block, crate::llm::Block::ToolUse { .. } | crate::llm::Block::ToolResult { .. })));
        assert!(format!("{:?}", requests[1].messages).contains("worker answer"));
    }

    #[tokio::test]
    async fn skill_commands_are_exposed_and_authorized_before_instructions_reach_the_model() {
        let h = harness().await;
        h.engine.store.update_session(&h.session.id, None, Some(&model()), None).unwrap();
        std::fs::create_dir_all(h._dir.join("ws/.drift/skills/private")).unwrap();
        std::fs::write(h._dir.join("ws/.drift/skills/private/SKILL.md"), "---\nname: private\ndescription: A private skill\n---\nPRIVATE_INSTRUCTIONS for $ARGUMENTS.").unwrap();
        assert!(h.engine.workspace_config(&h._dir.join("ws")).commands.iter().any(|command| command.skill.as_deref() == Some("private")));
        h.engine.permissions.set_policy(crate::permission::Policy { rules: vec![crate::permission::Rule {kind:"skill".into(),pattern:"private".into(),decision:crate::permission::Decision::Deny}] });
        h.provider.push(text("not loaded"));
        h.engine.execute_command(&h.session.id, "private", "src", None).await.unwrap();
        until_idle(&h).await;
        let transcript = h.engine.store.transcript(&h.session.id).unwrap();
        assert!(transcript.iter().flat_map(|message| &message.parts).any(|row| matches!(row.part, Part::ToolCall {status:ToolStatus::Denied,..})));
        assert!(!format!("{:?}", h.provider.requests.lock().unwrap()).contains("PRIVATE_INSTRUCTIONS"));
        h.engine.permissions.set_policy(crate::permission::Policy::default());
        h.provider.push(text("loaded"));
        h.engine.execute_command(&h.session.id, "private", "src", None).await.unwrap();
        until_idle(&h).await;
        assert!(format!("{:?}", h.provider.requests.lock().unwrap().last().unwrap().messages).contains("PRIVATE_INSTRUCTIONS for src."));
    }
}

#[derive(Debug)]
pub(crate) enum CommandError {
    Missing,
    Invalid(String),
    Mcp(String),
    Turn(TurnError),
    Store(rusqlite::Error),
}

impl From<rusqlite::Error> for CommandError { fn from(error: rusqlite::Error) -> Self { Self::Store(error) } }
impl From<TurnError> for CommandError { fn from(error: TurnError) -> Self { Self::Turn(error) } }

impl Engine {
    pub(crate) async fn execute_command(self: &Arc<Self>, id: &str, name: &str, arguments: &str, model: Option<ModelRef>) -> Result<Receipt, CommandError> {
        let session = self.store.session(id)?.ok_or(TurnError::NoSession)?;
        let workspace = self.store.workspace(&session.workspace_id)?.ok_or(TurnError::NoWorkspace)?;
        let workspace = crate::tool::canonical(std::path::Path::new(&workspace.path));
        let config = self.command_config(id, &workspace);
        if let Some(problem) = config.problems.first() { return Err(TurnError::Config(problem.clone()).into()); }
        let command = config.commands.iter().find(|command| command.name == name).ok_or(CommandError::Missing)?;
        let agent = command.agent.as_deref().unwrap_or(&session.agent);
        let definition = config.agent(agent).ok_or(TurnError::UnknownAgent)?;
        if definition.kind == AgentKind::Action { return Err(CommandError::Invalid("commands cannot select engine-only action agents".into())); }
        definition.usable().map_err(TurnError::Config)?;
        let model = model.or_else(|| command.model.clone());
        // A subagent never holds the conversation, so a command naming one always delegates.
        let delegated = command.subtask == Some(true) || definition.kind == AgentKind::Subagent;
        let text = match &command.server {
            Some(server) => {
                let prompt = command.name.split_once(':').map_or(command.name.as_str(), |(_, name)| name);
                self.mcp.get_prompt(server, Some(&workspace), prompt, command.named_arguments(arguments)).await.map_err(CommandError::Mcp)?
            }
            None => command.expand(arguments),
        };
        let (text, lines) = if command.server.is_none() && command.skill.is_none() { shell_lines(&text) } else { (text, Vec::new()) };
        if delegated && !lines.is_empty() {
            return Err(CommandError::Invalid("shell lines (!`...`) run in the conversation, so a command that delegates cannot use them".into()));
        }
        let routed = command.skill.is_some() || delegated;
        let bootstraps: Vec<Bootstrap> = if let Some(skill) = &command.skill {
            vec![Bootstrap { tool: "skill".into(), input: json!({ "name": skill, "arguments": arguments }), command: name.into(), model: None }]
        } else if delegated {
            let input = json!({ "description": name, "prompt": text, "subagent_type": agent, "run_in_background": false });
            vec![Bootstrap { tool: "task".into(), input, command: name.into(), model: model.clone() }]
        } else {
            lines.into_iter().map(|line| Bootstrap { tool: "bash".into(), input: json!({ "command": line, "description": format!("/{name}") }), command: name.into(), model: None }).collect()
        };
        // Its own agent and model run this turn only; a delegated or skill command runs on the session's.
        let chosen = !routed && (command.agent.is_some() || model.is_some());
        let shown = if routed { format!("/{name} {arguments}").trim_end().to_string() } else { text };
        let mentioned = if routed { Vec::new() } else { mentions(&workspace, &shown) };
        let prompt = Prompt {
            parts: std::iter::once(Part::Text { text: shown }).chain(mentioned).collect(),
            model: if chosen { model.or_else(|| definition.model.clone()) } else { None },
            variant: None,
            agent: (chosen && command.agent.is_some()).then(|| agent.to_string()),
            submission_id: None,
        };
        let how = Admission { bootstrap: &bootstraps, turn_only: chosen, config: Some(&config), ..Admission::default() };
        self.admit(id, prompt, how).await.map_err(CommandError::Turn)
    }
}

/// A template's `` !`line` `` shell lines, each named in the text where it stood; they run as the
/// turn's first calls, through the bash permission check, and their output follows the prompt.
fn shell_lines(text: &str) -> (String, Vec<String>) {
    let mut lines = Vec::new();
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("!`") {
        let Some(len) = rest[start + 2..].find('`') else { break };
        let line = rest[start + 2..start + 2 + len].trim();
        out.push_str(&rest[..start]);
        if line.is_empty() {
            out.push_str("!``");
        } else {
            out.push_str(&format!("`{line}` (its output follows)"));
            lines.push(line.to_string());
        }
        rest = &rest[start + 3 + len..];
    }
    out.push_str(rest);
    (out, lines)
}

/// A template's `@path` references to files or directories in the workspace, as @ mentions: read in
/// under the same rules as one the user typed. Anything not found there stays plain text.
fn mentions(workspace: &std::path::Path, text: &str) -> Vec<Part> {
    let mut seen = std::collections::HashSet::new();
    text.split_whitespace()
        .filter_map(|word| word.strip_prefix('@'))
        .map(|name| name.trim_end_matches(['.', ',', ';', ':', ')', '!', '?']))
        .filter(|name| !name.is_empty() && seen.insert(name.to_string()))
        .filter_map(|name| {
            let path = crate::tool::canonical(&workspace.join(name));
            (path.starts_with(crate::tool::canonical(workspace)) && path.exists()).then(|| Part::File { mime: "text/plain".into(), name: name.into(), url: file_url(&path), path: None })
        })
        .collect()
}

/// A `file:` URL for `path`, as the UI sends for an @ mention.
fn file_url(path: &std::path::Path) -> String {
    const PATH: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS.add(b' ').add(b'#').add(b'%').add(b'?');
    let raw = path.to_string_lossy().replace('\\', "/");
    let encoded = percent_encoding::utf8_percent_encode(&raw, PATH).to_string();
    if encoded.starts_with('/') { format!("file://{encoded}") } else { format!("file:///{encoded}") }
}
