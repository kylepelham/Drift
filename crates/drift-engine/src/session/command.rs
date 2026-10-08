//! User commands retain execution settings and use the existing permission and worker machinery.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};

use super::turn::{Admission, Prompt, Receipt, TurnError};
use super::types::{ModelRef, Part};
use crate::Engine;
use crate::config::AgentKind;

#[derive(Clone)]
pub(crate) struct Bootstrap {
    pub tool: String,
    pub input: Value,
    pub command: String,
    /// The command's model for a delegated task; carried in the call's metadata, never in its input.
    pub model: Option<ModelRef>,
}

struct CommandCalls<'a> {
    command: &'a crate::config::Command,
    arguments: &'a str,
    agent: &'a str,
    delegated: bool,
    model: &'a Option<ModelRef>,
    text: &'a str,
    lines: Vec<String>,
}

#[derive(Debug)]
pub(crate) enum CommandError {
    Missing,
    Invalid(String),
    Mcp(String),
    Turn(TurnError),
    Store(rusqlite::Error),
}

fn command_calls(input: CommandCalls<'_>) -> Vec<Bootstrap> {
    let name = &input.command.name;
    if let Some(skill) = &input.command.skill {
        return vec![Bootstrap {
            tool: "skill".into(),
            input: json!({ "name": skill, "arguments": input.arguments }),
            command: name.clone(),
            model: None,
        }];
    }

    if input.delegated {
        let arguments = json!({
            "description": name,
            "prompt": input.text,
            "subagent_type": input.agent,
            "run_in_background": false,
        });
        return vec![Bootstrap {
            tool: "task".into(),
            input: arguments,
            command: name.clone(),
            model: input.model.clone(),
        }];
    }

    input
        .lines
        .into_iter()
        .map(|line| Bootstrap {
            tool: "bash".into(),
            input: json!({ "command": line, "description": format!("/{name}") }),
            command: name.clone(),
            model: None,
        })
        .collect()
}

impl From<rusqlite::Error> for CommandError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<TurnError> for CommandError {
    fn from(error: TurnError) -> Self {
        Self::Turn(error)
    }
}

impl Engine {
    async fn command_text(
        &self,
        command: &crate::config::Command,
        workspace: &Path,
        arguments: &str,
    ) -> Result<String, CommandError> {
        let Some(server) = &command.server else {
            return Ok(command.expand(arguments));
        };

        let name = command
            .name
            .split_once(':')
            .map_or(command.name.as_str(), |(_, name)| name);

        self.mcp
            .get_prompt(server, Some(workspace), name, command.named_arguments(arguments))
            .await
            .map_err(|error| CommandError::Mcp(error.to_string()))
    }

    pub(crate) async fn execute_command(
        self: &Arc<Self>,
        id: &str,
        name: &str,
        arguments: &str,
        model: Option<ModelRef>,
    ) -> Result<Receipt, CommandError> {
        let session = self.store.session(id)?.ok_or(TurnError::NoSession)?;
        let workspace = self
            .store
            .workspace(&session.workspace_id)?
            .ok_or(TurnError::NoWorkspace)?;
        let workspace = crate::tool::canonical(Path::new(&workspace.path));
        let config = self.command_config(id, &workspace);
        if let Some(problem) = config.problems.first() {
            return Err(TurnError::Config(problem.clone()).into());
        }

        let command = config
            .commands
            .iter()
            .find(|command| command.name == name)
            .ok_or(CommandError::Missing)?;
        let agent = command.agent.as_deref().unwrap_or(&session.agent);
        let definition = config.agent(agent).ok_or(TurnError::UnknownAgent)?;
        if definition.kind == AgentKind::Action {
            return Err(CommandError::Invalid(
                "commands cannot select engine-only action agents".into(),
            ));
        }
        definition
            .usable()
            .map_err(|error| TurnError::Config(error.to_string()))?;

        let model = model.or_else(|| command.model.clone());
        let delegated = command.subtask == Some(true) || definition.kind == AgentKind::Subagent;
        let text = self.command_text(command, &workspace, arguments).await?;
        let (text, lines) = if command.server.is_none() && command.skill.is_none() {
            shell_lines(&text)
        } else {
            (text, Vec::new())
        };
        if delegated && !lines.is_empty() {
            return Err(CommandError::Invalid(
                "shell lines (!`...`) run in the conversation, so a command that delegates cannot use them".into(),
            ));
        }

        let routed = command.skill.is_some() || delegated;
        let bootstraps = command_calls(CommandCalls {
            command,
            arguments,
            agent,
            delegated,
            model: &model,
            text: &text,
            lines,
        });

        // Direct commands temporarily use their chosen agent and model; skills and tasks keep the session's.
        let chosen = !routed && (command.agent.is_some() || model.is_some());
        let shown = if routed {
            format!("/{name} {arguments}").trim_end().to_string()
        } else {
            text
        };
        let mentioned = if routed {
            Vec::new()
        } else {
            mentions(&workspace, &shown)
        };
        let prompt = Prompt {
            parts: std::iter::once(Part::Text { text: shown }).chain(mentioned).collect(),
            model: if chosen {
                model.or_else(|| definition.model.clone())
            } else {
                None
            },
            variant: None,
            agent: (chosen && command.agent.is_some()).then(|| agent.to_string()),
            submission_id: None,
        };
        let how = Admission {
            bootstrap: &bootstraps,
            turn_only: chosen,
            config: Some(&config),
            ..Admission::default()
        };

        self.admit(id, prompt, how).await.map_err(CommandError::Turn)
    }
}

/// A template's shell lines are named in the text where they stood and run as the turn's first calls.
/// Each uses the bash permission check, and its output follows the prompt.
fn shell_lines(text: &str) -> (String, Vec<String>) {
    let mut lines = Vec::new();
    let mut output = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("!`") {
        let Some(length) = rest[start + 2..].find('`') else {
            break;
        };

        let line = rest[start + 2..start + 2 + length].trim();
        output.push_str(&rest[..start]);
        if line.is_empty() {
            output.push_str("!``");
        } else {
            let _ = write!(output, "`{line}` (its output follows)");
            lines.push(line.to_string());
        }
        rest = &rest[start + 3 + length..];
    }

    output.push_str(rest);
    (output, lines)
}

/// Template @path references to workspace files and directories become mentions under the usual read rules.
/// References not found in the workspace remain plain text.
fn mentions(workspace: &Path, text: &str) -> Vec<Part> {
    let mut seen = std::collections::HashSet::new();

    text.split_whitespace()
        .filter_map(|word| word.strip_prefix('@'))
        .map(|name| name.trim_end_matches(['.', ',', ';', ':', ')', '!', '?']))
        .filter(|name| !name.is_empty() && seen.insert(name.to_string()))
        .filter_map(|name| {
            let path = crate::tool::canonical(&workspace.join(name));
            (path.starts_with(crate::tool::canonical(workspace)) && path.exists()).then(|| Part::File {
                mime: "text/plain".into(),
                name: name.into(),
                url: file_url(&path),
                path: None,
            })
        })
        .collect()
}

/// A `file:` URL for `path`, as the UI sends for an @ mention.
fn file_url(path: &Path) -> String {
    const PATH: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS.add(b' ').add(b'#').add(b'%').add(b'?');
    let raw = path.to_string_lossy().replace('\\', "/");
    let encoded = percent_encoding::utf8_percent_encode(&raw, PATH).to_string();

    if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    }
}

#[cfg(test)]
mod tests;
