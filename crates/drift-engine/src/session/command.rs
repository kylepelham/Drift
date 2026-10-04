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
        let config = self.command_config(id, &crate::tool::canonical(std::path::Path::new(&workspace.path)));
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
                self.mcp.get_prompt(server, prompt, command.named_arguments(arguments)).await.map_err(CommandError::Mcp)?
            }
            None => command.expand(arguments),
        };
        let bootstrap = if let Some(skill) = &command.skill {
            Some(Bootstrap { tool: "skill".into(), input: json!({ "name": skill, "arguments": arguments }), command: name.into(), model: None })
        } else if delegated {
            let input = json!({ "description": name, "prompt": text, "subagent_type": agent, "run_in_background": false });
            Some(Bootstrap { tool: "task".into(), input, command: name.into(), model: model.clone() })
        } else {
            None
        };
        // Its own agent and model run this turn only; a delegated or skill command runs on the session's.
        let chosen = bootstrap.is_none() && (command.agent.is_some() || model.is_some());
        let prompt = Prompt {
            parts: vec![Part::Text { text: if bootstrap.is_some() { format!("/{name} {arguments}").trim_end().into() } else { text } }],
            model: if chosen { model.or_else(|| definition.model.clone()) } else { None },
            variant: None,
            agent: (chosen && command.agent.is_some()).then(|| agent.to_string()),
            submission_id: None,
        };
        let how = Admission { bootstrap: bootstrap.as_ref(), turn_only: chosen, config: Some(&config), ..Admission::default() };
        self.admit(id, prompt, how).await.map_err(CommandError::Turn)
    }
}
