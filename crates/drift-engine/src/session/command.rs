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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::turn::tests::{harness, model, text, until_idle};
    use crate::session::types::ToolStatus;

    #[tokio::test]
    async fn commands_select_agents_models_and_expand_arguments() {
        let h = harness().await;
        std::fs::create_dir_all(h._dir.join("ws/.drift/commands")).unwrap();
        std::fs::write(h._dir.join("ws/.drift/commands/check.md"), "---\nagent: plan\nmodel: anthropic/claude-haiku-4-5\nsubtask: false\n---\nReview $1 and $2.").unwrap();
        h.provider.push(text("reviewed"));
        h.engine.execute_command(&h.session.id, "check", "src tests", None).await.unwrap();
        until_idle(&h).await;
        let session = h.engine.store.session(&h.session.id).unwrap().unwrap();
        assert_eq!(session.agent, "plan");
        assert_eq!(session.model.unwrap().model, "claude-haiku-4-5");
        let requests = h.provider.requests.lock().unwrap();
        assert!(format!("{:?}", requests[0].messages).contains("Review src and tests."));
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
        assert_eq!(requests[0].model, "claude-haiku-4-5");
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
        let model = model.or_else(|| command.model.clone());
        let delegated = command.subtask.unwrap_or(command.agent.is_some() && definition.kind == AgentKind::Subagent);
        let text = match &command.server {
            Some(server) => {
                let prompt = command.name.split_once(':').map_or(command.name.as_str(), |(_, name)| name);
                self.mcp.get_prompt(server, prompt, command.named_arguments(arguments)).await.map_err(CommandError::Mcp)?
            }
            None => command.expand(arguments),
        };
        let bootstrap = if let Some(skill) = &command.skill {
            Some(Bootstrap { tool: "skill".into(), input: json!({"name":skill,"arguments":arguments}), command:name.into() })
        } else if delegated {
            let mut input = json!({"description":name,"prompt":text,"subagent_type":agent,"run_in_background":false});
            if let Some(model) = &model { input["model"] = json!(format!("{}/{}", model.provider, model.model)); }
            Some(Bootstrap { tool:"task".into(), input, command:name.into() })
        } else { None };
        let prompt = Prompt {
            parts: vec![Part::Text { text: if bootstrap.is_some() { format!("/{name} {arguments}").trim_end().into() } else { text } }],
            model: if delegated { session.model.clone().or_else(|| model.clone()).or_else(|| definition.model.clone()) } else { model.or_else(|| definition.model.clone()) },
            variant: None,
            agent: (!delegated).then(|| agent.to_string()),
            submission_id: None,
        };
        self.admit(id, prompt, Admission { bootstrap: bootstrap.as_ref(), command_agent: true, config: Some(&config), ..Admission::default() }).await.map_err(CommandError::Turn)
    }
}
