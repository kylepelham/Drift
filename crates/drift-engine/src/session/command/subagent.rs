//! `/subagent [seq|async] [model] <prompt>`: the user starts a subagent as the `task` tool would.

use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use serde_json::json;
use utoipa::ToSchema;

use super::{Bootstrap, CommandError};
use crate::Engine;
use crate::config::{AgentKind, Config};
use crate::event::Event;
use crate::llm::catalog::Catalog;
use crate::permission::Decision;
use crate::session::tasks::{Mode, TaskRecord};
use crate::session::turn::{Admission, FencedPrompt, Prompt, Receipt, TurnError, read_only_reason};
use crate::session::types::{ModelRef, Part, Session, Visibility};
use crate::store::{Launch, NewSession, NewTask, Pick, QueuedPrompt};
use crate::tool::Ask;

pub(crate) const NAME: &str = "subagent";
/// The agent the `task` tool gives a job to when it names none.
const AGENT: &str = "general";
/// A worker's `reason` when the user started it, not the model.
pub(crate) const USER_REASON: &str = "user";
const USAGE: &str = "/subagent [seq|async] [model] <prompt>";
const DESCRIPTION_WORDS: usize = 6;
const BACKGROUND_OFF: &str = "background tasks are turned off in Settings, so it cannot run beside the conversation";

/// What `/subagent` did: the message it put in the conversation, or the queued prompt it waits in.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SubagentStart {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<Receipt>,
    /// A `seq` job sent while another job runs; it starts when the queue reaches it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queued: Option<QueuedPrompt>,
}

#[derive(Debug, PartialEq)]
struct Job {
    /// What was typed after `/subagent`, trimmed.
    typed: String,
    mode: Mode,
    model: Option<ModelRef>,
    prompt: String,
    description: String,
}

impl Engine {
    /// Starts the job typed after `/subagent` in a conversation.
    pub(crate) async fn start_subagent(
        self: &Arc<Self>,
        session_id: &str,
        arguments: &str,
    ) -> Result<SubagentStart, CommandError> {
        let (session, config) = self.subagent_scope(session_id)?;
        let job = self.subagent_job(&session, arguments)?;
        if job.mode == Mode::Background {
            let receipt = self.launch_subagent(&session, &config, &job, None).await?;
            return Ok(SubagentStart { receipt, queued: None });
        }

        // A command the user sends lets a queue held since a Stop go again, as a prompt does.
        self.turns.release_queue(session_id);
        match self.subagent_turn(&session, &config, &job, None).await {
            Ok(receipt) => Ok(SubagentStart {
                receipt: Some(receipt),
                queued: None,
            }),
            Err(TurnError::Busy) => Ok(SubagentStart {
                receipt: None,
                queued: Some(self.queue_prompt(session_id, waiting(arguments))?),
            }),
            Err(error) => Err(error.into()),
        }
    }

    /// Runs a queued job: `seq` as a turn (`Busy` while one runs; in the background if sent `now`), `async` at once.
    pub(crate) async fn run_queued_subagent(
        self: &Arc<Self>,
        session_id: &str,
        arguments: &str,
        submission_id: String,
        now: bool,
    ) -> Result<Option<Receipt>, TurnError> {
        let (session, config) = self.subagent_scope(session_id).map_err(turn_error)?;
        let mut job = self.subagent_job(&session, arguments).map_err(turn_error)?;
        if job.mode == Mode::Foreground {
            let submission = Some(submission_id.clone());
            match self.subagent_turn(&session, &config, &job, submission).await {
                Err(TurnError::Busy) if now && !self.background_enabled() => {
                    return Err(TurnError::Config(format!(
                        "{BACKGROUND_OFF}; send it again once the current turn ends"
                    )));
                }
                Err(TurnError::Busy) if now => job.mode = Mode::Background,
                started => return started.map(Some),
            }
        }

        self.launch_subagent(&session, &config, &job, Some(&submission_id))
            .await
            .map_err(turn_error)
    }

    /// The conversation and its config; a subagent's transcript, an archived chat and a missing agent are refused.
    fn subagent_scope(&self, session_id: &str) -> Result<(Session, Config), CommandError> {
        let session = self.store.session(session_id)?.ok_or(TurnError::NoSession)?;
        if let Some(reason) = unfit(&session) {
            return Err(CommandError::Invalid(reason.into()));
        }
        let workspace = self
            .store
            .workspace(&session.workspace_id)?
            .ok_or(TurnError::NoWorkspace)?;
        let config = self.command_config(session_id, &crate::tool::canonical(Path::new(&workspace.path)));
        if let Some(problem) = config.problems.first() {
            return Err(TurnError::Config(problem.clone()).into());
        }

        match config.agent(AGENT) {
            Some(agent) if agent.kind != AgentKind::Action => {
                agent.usable().map_err(|error| TurnError::Config(error.to_string()))?;
            }
            _ => {
                return Err(CommandError::Invalid(format!(
                    "this workspace has no {AGENT} agent to take the job"
                )));
            }
        }

        Ok((session, config))
    }

    fn subagent_job(&self, session: &Session, arguments: &str) -> Result<Job, CommandError> {
        let preferred = session.model.as_ref().map(|model| model.provider.as_str());

        parse(arguments, &self.catalog_view(), preferred).map_err(CommandError::Invalid)
    }

    /// Admits a turn whose first call is the user's foreground `task`, run before the model answers.
    async fn subagent_turn(
        self: &Arc<Self>,
        session: &Session,
        config: &Config,
        job: &Job,
        submission_id: Option<String>,
    ) -> Result<Receipt, TurnError> {
        let input = json!({
            "description": job.description,
            "prompt": job.prompt,
            "subagent_type": AGENT,
            "run_in_background": false,
        });
        let bootstrap = [Bootstrap {
            tool: "task".into(),
            input,
            command: NAME.into(),
            model: job.model.clone(),
        }];
        let prompt = Prompt {
            parts: note_parts(seq_note(job), &job.typed),
            model: None,
            variant: None,
            agent: None,
            submission_id,
            worktree: false,
        };
        let how = Admission {
            bootstrap: &bootstrap,
            config: Some(config),
            ..Admission::default()
        };

        self.admit(&session.id, prompt, how).await
    }

    /// Starts the worker in the background under the chat's Stop and notes it; `key` makes a retry the same launch.
    async fn launch_subagent(
        self: &Arc<Self>,
        parent: &Session,
        config: &Config,
        job: &Job,
        key: Option<&str>,
    ) -> Result<Option<Receipt>, CommandError> {
        if !self.background_enabled() {
            return Err(CommandError::Invalid(format!(
                "{BACKGROUND_OFF}; `/subagent seq` runs it after the current turn instead"
            )));
        }
        if let Some(reason) = self.launch_refusal(parent, config) {
            return Err(TurnError::Refused(reason).into());
        }
        let parent_model = parent.model.clone().ok_or(TurnError::NoModel)?;
        let model = job
            .model
            .clone()
            .or_else(|| config.agent_model(AGENT))
            .unwrap_or_else(|| parent_model.clone());
        let call_id = key.map_or_else(|| crate::id::new("user"), String::from);
        let title = format!("{} (@{AGENT} subagent)", job.description);
        let (scope, generation) = self.worker_scope(&parent.id);
        let new = NewTask {
            parent_session_id: &parent.id,
            call_id: &call_id,
            description: &job.description,
            agent: AGENT,
            mode: Mode::Background,
            reason: USER_REASON,
            generation,
            tags: &[],
        };
        let child = NewSession {
            workspace_id: &parent.workspace_id,
            parent_id: Some(&parent.id),
            visibility: Visibility::Hidden,
            title: &title,
            agent: AGENT,
            model: Some(&model),
        };
        let Launch { task, child, created } = self.store.launch_task(new, child)?;
        if !created {
            return Ok(None);
        }
        self.hub.publish(Event::SessionCreated { session: child.clone() });
        self.publish_task(&task.id);
        self.permissions.inherit(&child.id, &parent.id);

        let own_level = config.agent(AGENT).is_some_and(|agent| agent.variant.is_some());
        let prompt = Prompt {
            parts: vec![Part::Text {
                text: job.prompt.clone(),
            }],
            model: Some(model),
            variant: (!own_level).then(|| parent.variant.clone()),
            agent: None,
            submission_id: None,
            worktree: false,
        };
        if let Err(failure) = self.admit_worker(&task, prompt, scope.child_token()).await {
            // The user hears why it could not start; the conversation never heard of it, so nothing is owed.
            let _ = self.store.mark_task_delivered(&task.id);
            self.publish_task(&task.id);
            return Err(CommandError::Invalid(failure.to_string()));
        }

        let receipt = self.record_note(&parent.id, &parent_model, async_note(&task, job), &job.typed)?;
        Ok(Some(receipt))
    }

    /// Why the agent may not hand this off: read-only, or a rule denying `task`; typing the command answers an `ask`.
    fn launch_refusal(&self, parent: &Session, config: &Config) -> Option<String> {
        let read_only = |name: &str| config.agent(name).is_some_and(|agent| agent.read_only);
        if read_only(&parent.agent) && !read_only(AGENT) {
            let plans = self.plans_dir(&parent.workspace_id);
            return Some(read_only_reason(&parent.agent, Some(&plans)));
        }
        let ask = Ask::new("task", AGENT, format!("Delegate to {AGENT}")).allow_by_default();
        let decision =
            self.permissions
                .decide_under(&parent.id, &config.policy(), &config.agent_policy(&parent.agent), &ask);

        (decision == Decision::Deny).then(|| "A permission rule forbids this call.".to_string())
    }

    /// Puts a note in the conversation without starting a turn; a running turn reads it at its next step.
    fn record_note(
        &self,
        session_id: &str,
        model: &ModelRef,
        text: String,
        arguments: &str,
    ) -> Result<Receipt, TurnError> {
        let prompt = FencedPrompt {
            pick: Pick::model(model),
            parts: note_parts(text, arguments),
            submission: None,
            abort: None,
            delivery: None,
        };
        let admitted = self.admit_fenced(session_id, prompt)?;

        Ok(self.announce(session_id, admitted))
    }
}

fn unfit(session: &Session) -> Option<&'static str> {
    if session.visibility == Visibility::Hidden {
        Some("/subagent runs in a conversation, not in a subagent")
    } else if session.archived_at.is_some() {
        Some("this conversation is archived; restore it to start a subagent")
    } else {
        None
    }
}

/// What follows `/subagent` in a queued prompt that is one, so the queue runs the job instead of sending the text.
pub(crate) fn queued_arguments(prompt: &Prompt) -> Option<String> {
    let [Part::Text { text }] = prompt.parts.as_slice() else {
        return None;
    };
    let rest = text.trim().strip_prefix('/')?.strip_prefix(NAME)?;

    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| rest.trim().to_string())
}

/// A `seq` job waiting in the queue, as typed, so the queue shows it and editing it edits the job.
fn waiting(arguments: &str) -> Prompt {
    Prompt {
        parts: vec![Part::Text {
            text: format!("/{NAME} {}", arguments.trim()),
        }],
        model: None,
        variant: None,
        agent: None,
        submission_id: None,
        worktree: false,
    }
}

fn parse(arguments: &str, catalog: &Catalog, preferred: Option<&str>) -> Result<Job, String> {
    let (word, rest) = first_word(arguments);
    let (mode, rest) = match word.to_ascii_lowercase().as_str() {
        "seq" => (Mode::Foreground, rest),
        "async" => (Mode::Background, rest),
        _ => (Mode::Background, arguments),
    };
    let (word, after) = first_word(rest);
    let model = named_model(word, catalog, preferred)?;
    let prompt = if model.is_some() { after } else { rest }.trim();
    if prompt.is_empty() {
        return Err(format!("say what the subagent should do: {USAGE}"));
    }

    Ok(Job {
        typed: arguments.trim().into(),
        mode,
        model,
        prompt: prompt.into(),
        description: description(prompt),
    })
}

fn first_word(text: &str) -> (&str, &str) {
    let text = text.trim_start();

    text.split_at(text.find(char::is_whitespace).unwrap_or(text.len()))
}

/// The model `word` names (`provider/model` the catalog must have, or an id it knows); other words start the prompt.
fn named_model(word: &str, catalog: &Catalog, preferred: Option<&str>) -> Result<Option<ModelRef>, String> {
    if let Some(named) = crate::config::parse_model(word) {
        return match catalog.providers.get(&named.provider) {
            Some(provider) if provider.models.contains_key(&named.model) => Ok(Some(named)),
            Some(_) => Err(format!("no model `{word}` among the connected providers")),
            None => Ok(None),
        };
    }

    let offering = catalog
        .providers
        .iter()
        .filter(|(_, provider)| provider.models.contains_key(word))
        .map(|(name, _)| name);
    let provider = offering
        .clone()
        .find(|name| Some(name.as_str()) == preferred)
        .or_else(|| offering.clone().next());

    Ok(provider.map(|provider| ModelRef {
        provider: provider.clone(),
        model: word.into(),
    }))
}

/// The prompt's first words, as the job is named in the task list and the subagent's title.
fn description(prompt: &str) -> String {
    let words: Vec<&str> = prompt.split_whitespace().collect();
    let short = words[..words.len().min(DESCRIPTION_WORDS)].join(" ");

    if words.len() > DESCRIPTION_WORDS {
        format!("{short}...")
    } else {
        short
    }
}

/// The note the model reads, beside the command the transcript shows.
fn note_parts(text: String, arguments: &str) -> Vec<Part> {
    vec![
        Part::Text { text },
        Part::Command {
            name: NAME.into(),
            arguments: arguments.into(),
        },
    ]
}

fn async_note(task: &TaskRecord, job: &Job) -> String {
    format!(
        "The user started a subagent themselves with /subagent; you did not launch it, and nothing went wrong. \
         It runs in the background as task {} (@{}): {}. The prompt they gave it:\n\n{}\n\n\
         Its result will arrive in this conversation when it finishes. Carry on with what you were doing; \
         you need not wait for it or do its job yourself.",
        task.id, task.agent, task.description, job.prompt
    )
}

fn seq_note(job: &Job) -> String {
    format!(
        "The user started a subagent themselves with /subagent; the task call that follows was made for them, \
         not chosen by you, and nothing went wrong. It runs @{AGENT}: {}. The prompt they gave it:\n\n{}\n\n\
         Its result is that call's result; carry on from it.",
        job.description, job.prompt
    )
}

fn turn_error(error: CommandError) -> TurnError {
    match error {
        CommandError::Turn(error) => error,
        CommandError::Store(error) => error.into(),
        CommandError::Missing => TurnError::Config(format!("no /{NAME} command")),
        CommandError::Invalid(reason) | CommandError::Mcp(reason) => TurnError::Config(reason),
    }
}

#[cfg(test)]
mod tests;
