use super::spool::Spool;
use super::{Ask, Context, Output, RunFuture, Tool, ToolError, command, required_str};
use crate::background::Pipes;
use crate::llm::ToolSpec;
use serde_json::{Value, json};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

mod approval;
mod background;
mod output;
mod shell;

#[cfg(test)]
mod tests;

use approval::{drop_moves_within, enter_workdir, why_it_asks, workdir};
use output::{Ended, collect, report};
pub use shell::Shell;
use shell::{chain_note, detect_shell, shell_note};

/// Until the user's Settings value arrives.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// The longest a model may ask for, the same ceiling as the Settings choice (1,440 minutes).
const MAX_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone)]
pub struct Bash {
    shell: Shell,
}

impl Bash {
    /// Git's bash on Windows if present, so one syntax works everywhere; PowerShell 7 otherwise.
    pub fn detect() -> Self {
        Self { shell: detect_shell() }
    }

    pub fn with(shell: Shell) -> Self {
        Self { shell }
    }

    #[cfg(test)]
    pub(crate) fn shell(&self) -> &Shell {
        &self.shell
    }

    pub(crate) fn dialect(&self) -> command::Dialect {
        match self.shell {
            Shell::Bash(_) => command::Dialect::Bash,
            Shell::PowerShell(_) => command::Dialect::PowerShell,
        }
    }
}

impl Tool for Bash {
    fn permissions(&self) -> &'static [&'static str] {
        &["bash"]
    }

    /// Every option, so a call made under either setting validates; a turn is offered [`Bash::spec_with`] its settings.
    fn spec(&self) -> ToolSpec {
        self.spec_with(Limits {
            background: true,
            handoff: true,
        })
    }

    fn spec_for(&self, engine: &crate::Engine) -> ToolSpec {
        self.spec_with(Limits {
            background: engine.background_processes(),
            handoff: engine.shell_handoff(),
        })
    }

    fn ask(&self, context: &Context, input: &Value) -> Option<Ask> {
        let command = input["command"].as_str()?;
        let mut ask = Ask::shell(
            self.dialect(),
            command,
            input["description"].as_str().unwrap_or(command),
        );
        // A workdir that is not a directory is refused at execution, so analysis starts in the workspace.
        let directory = workdir(context, input).unwrap_or_else(|_| context.workspace.clone());
        drop_moves_within(context, &directory, &mut ask);
        enter_workdir(context, &directory, &mut ask);
        ask.reason = why_it_asks(context, &directory, &ask);
        ask.default_allow = ask.reason.is_none();

        Some(ask)
    }

    fn mutates(&self) -> bool {
        true
    }

    /// A line made only of commands known to read (`git status`, `ls`, `rg`, ...) with no redirection
    /// that writes is not captured before and after; anything else, or anything unclear, is.
    fn call_mutates(&self, input: &Value) -> bool {
        input["command"]
            .as_str()
            .is_none_or(|line| !command::reads_only(self.dialect(), line))
    }

    fn run<'a>(&'a self, context: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let command = required_str(&input, "command")?;
            let directory = workdir(context, &input)?;
            let background = input["background"].as_bool() == Some(true);
            if background && !context.engine.background_processes() {
                return Err(ToolError(
                    "background commands are turned off in Settings; run it in the foreground with a `timeout`".into(),
                ));
            }
            let started = Started {
                command: command.to_string(),
                title: input["description"].as_str().unwrap_or(command).to_string(),
                running: spawn(shell_for(
                    &self.shell,
                    command,
                    &directory,
                    background,
                    &context.config.programs,
                ))?,
            };

            if background {
                return Ok(background::start(context, started, Vec::new()).await);
            }

            Ok(self.foreground(context, &input, started, &directory).await)
        })
    }

    /// Stop ends the command at once and keeps what it printed, so the turn awaits it rather than racing it.
    fn stops_itself(&self) -> bool {
        true
    }

    /// The limit shows while the command runs, so the user can see when it will be stopped.
    fn running_metadata(&self, context: &Context, input: &Value) -> Option<super::ToolMetadata> {
        Some(super::ToolMetadata {
            shell_timeout_ms: Some(limit_for(context, input).map(|duration| duration.as_millis() as u64)),
            ..Default::default()
        })
    }

    /// A command stopped by its time limit or by the user failed, though its partial output still matters.
    fn failed(&self, output: &Output) -> bool {
        output.metadata.timed_out == Some(true) || output.metadata.stopped == Some(true)
    }
}

/// What the user's settings let a command do past its call.
#[derive(Clone, Copy)]
struct Limits {
    /// It may be started in the background, and seen there with `process`.
    background: bool,
    /// At its time limit it moves to the background rather than being stopped.
    handoff: bool,
}

impl Bash {
    fn spec_with(&self, limits: Limits) -> ToolSpec {
        let mut properties = json!({
            "command": { "type": "string", "description": "The command to run." },
            "timeout": { "type": "integer", "description": timeout_note(limits) },
            "description": { "type": "string", "description": "Always give one: five to ten words saying what the command does, such as \"Run the engine tests\". The user sees this in place of the command, so leave it out only for a command that already reads as plain words." },
            "workdir": { "type": "string", "description": "Directory to run in instead of changing directory first. One outside the workspace asks first. Default: the workspace root." }
        });
        if limits.background {
            properties["background"] = json!({ "type": "boolean", "description": "Start it in the background and return its process id at once. Only for servers, watchers, or work you will do something else during; never for a command you would only wait for. Default false." });
        }

        ToolSpec {
            name: "bash".into(),
            description: include_str!("prompts/bash.txt")
                .trim()
                .replace("{shell}", shell_note(&self.shell, cfg!(windows)))
                .replace("{chain}", chain_note(&self.shell))
                .replace("{limits}", limits_note(limits)),
            input_schema: json!({ "type": "object", "properties": properties, "required": ["command"] }),
        }
    }

    /// Runs the command until it exits, is stopped, or reaches its limit, where it moves to the background.
    async fn foreground(&self, context: &Context, input: &Value, mut started: Started, directory: &Path) -> Output {
        let limit = limit_for(context, input);
        let path = context
            .engine
            .data_dir
            .join("tool-output")
            .join(&context.session_id)
            .join(format!("{}.log", context.call_id));
        let mut spool = Spool::new(Some(path));

        let running = &mut started.running;
        let ended = {
            let collecting = bounded(
                limit,
                collect(&mut running.child, &mut running.pipes, &mut spool, &context.progress),
            );
            tokio::select! {
                ended = collecting => ended.unwrap_or(Ended::TimedOut),
                () = context.abort.cancelled() => Ended::Stopped,
            }
        };

        if matches!(ended, Ended::TimedOut) && context.engine.shell_handoff() {
            return background::moved(context, started, spool.finish(), limit).await;
        }
        if !matches!(ended, Ended::Exited { lingering: false, .. }) {
            kill_tree(&running.tree, &mut running.child).await;
        }
        if matches!(ended, Ended::Exited { code: 0, .. }) {
            self.mark_files_read(context, &started.command, directory);
        }

        report(started.title, spool.finish(), ended, limit)
    }

    /// Runs a line the engine itself was asked to run (a worktree's setup) in the foreground until it exits,
    /// stopping it at the user's time limit; never moved to the background.
    pub(crate) async fn run_to_end(
        &self,
        context: &Context,
        line: &str,
        directory: &Path,
        env: &[(&str, &Path)],
    ) -> Result<Output, ToolError> {
        let mut process = shell_for(&self.shell, line, directory, false, &context.config.programs);
        process.envs(env.iter().copied());
        let mut running = spawn(process)?;
        let limit = context.engine.shell_timeout();
        let mut spool = Spool::new(None);
        let unseen = super::Progress::default();
        let ended = {
            let collecting = bounded(
                limit,
                collect(&mut running.child, &mut running.pipes, &mut spool, &unseen),
            );
            tokio::select! {
                ended = collecting => ended.unwrap_or(Ended::TimedOut),
                () = context.abort.cancelled() => Ended::Stopped,
            }
        };
        if !matches!(ended, Ended::Exited { lingering: false, .. }) {
            kill_tree(&running.tree, &mut running.child).await;
        }

        Ok(report(line.to_string(), spool.finish(), ended, limit))
    }

    /// Files a command that succeeded showed in full count as read, so an edit after `cat` is not blind.
    fn mark_files_read(&self, context: &Context, command: &str, directory: &Path) {
        for file in command::files_read(self.dialect(), command)
            .iter()
            .map(|file| super::canonical(&directory.join(file)))
            .filter(|file| file.is_file())
        {
            context.files.mark_read(&file);
        }
    }
}

fn timeout_note(limits: Limits) -> &'static str {
    if limits.handoff {
        "Milliseconds before the command moves to the background. Default and most: the user's setting."
    } else {
        "Milliseconds before the command is stopped. Default: the user's setting. Max 86400000."
    }
}

/// What the description says happens at the time limit, and how to run something long.
fn limits_note(limits: Limits) -> &'static str {
    match (limits.background, limits.handoff) {
        (true, true) => concat!(
            "- Run commands in the foreground, even long builds and tests: one still running after the user's time ",
            "limit, or after a shorter `timeout` in milliseconds when you pass one, moves to the background by itself and the ",
            "result says its process id. Use `background: true` only for servers and watchers, or to run several ",
            "commands at once or keep working while one runs; a command you would only wait for costs an extra call ",
            "in the background. Use `process` to wait for a background command, read its output or stop it."
        ),
        (true, false) => concat!(
            "- Commands and their child processes are stopped after the user's time limit, or after `timeout` ",
            "milliseconds when you pass one; pass a larger `timeout` for a long build or test run. Use ",
            "`background: true` only for servers and watchers, or to run several commands at once or keep working ",
            "while one runs, then use `process` to wait for it, read its output or stop it."
        ),
        (false, _) => concat!(
            "- Commands and their child processes are stopped after the user's time limit, or after `timeout` ",
            "milliseconds when you pass one. Pass a larger `timeout` for a build or test run you expect to be long. ",
            "Never start long-lived servers or watchers here."
        ),
    }
}

/// A spawned shell: the process, the job holding everything it starts, and its output pipes.
pub(crate) struct Running {
    pub child: tokio::process::Child,
    pub tree: Option<crate::platform::process::Tree>,
    pub pipes: Pipes,
    pub started_at: i64,
}

/// A command the call started, with what the user is shown it does.
pub(crate) struct Started {
    pub command: String,
    pub title: String,
    pub running: Running,
}

/// The shell for `line`; a background command gets an input pipe so the model can type into it. `programs` (the
/// bin folders of bundles that are on) are on its PATH after the user's own.
fn shell_for(
    shell: &Shell,
    line: &str,
    directory: &Path,
    background: bool,
    programs: &[std::path::PathBuf],
) -> tokio::process::Command {
    let mut process = shell_command(shell, line);
    let stdin = if background { Stdio::piped() } else { Stdio::null() };
    process
        .current_dir(directory)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    process.env("PATH", crate::platform::process::path_with(programs));
    // Python holds its output back when it is not writing to a terminal, so a running script would look silent.
    if std::env::var_os("PYTHONUNBUFFERED").is_none() {
        process.env("PYTHONUNBUFFERED", "1");
    }

    process
}

/// Starts the shell in a job that holds everything it starts.
fn spawn(mut process: tokio::process::Command) -> Result<Running, ToolError> {
    #[cfg(windows)]
    process.creation_flags(0x0800_0000 | 0x0000_0004);
    crate::platform::process::prepare(&mut process);

    let mut child = process
        .spawn()
        .map_err(|error| ToolError(format!("could not start shell: {error}")))?;
    // Keeping the tree handle alive makes a dropped run future kill every descendant.
    let tree = child
        .id()
        .and_then(|pid| crate::platform::process::Tree::adopt(pid).ok());
    crate::platform::process::resume(&child).map_err(|error| ToolError(format!("could not start shell: {error}")))?;
    let pipes = Pipes::take(&mut child);

    Ok(Running {
        child,
        tree,
        pipes,
        started_at: crate::id::now_ms(),
    })
}

fn shell_command(shell: &Shell, line: &str) -> tokio::process::Command {
    match shell {
        Shell::Bash(path) => {
            let mut command = tokio::process::Command::new(path);
            command.arg("-c").arg(line);
            command
        }
        Shell::PowerShell(path) => {
            let mut command = tokio::process::Command::new(path);
            command.args(["-NoProfile", "-NonInteractive", "-Command", line]);
            command
        }
    }
}

/// The model's `timeout` when it gives one, otherwise the user's Settings value; `None` never stops it. When a
/// command moves to the background at its limit, the user's value is the longest the model may keep it in front.
fn limit_for(context: &Context, input: &Value) -> Option<Duration> {
    let setting = context.engine.shell_timeout();
    let Some(asked) = input["timeout"].as_u64() else {
        return setting;
    };
    let asked = Duration::from_millis(asked).min(MAX_TIMEOUT);

    match setting {
        Some(setting) if context.engine.shell_handoff() => Some(asked.min(setting)),
        _ => Some(asked),
    }
}

async fn bounded<F: std::future::Future>(limit: Option<Duration>, work: F) -> Option<F::Output> {
    match limit {
        Some(limit) => tokio::time::timeout(limit, work).await.ok(),
        None => Some(work.await),
    }
}

async fn kill_tree(tree: &Option<crate::platform::process::Tree>, child: &mut tokio::process::Child) {
    if let Some(tree) = tree {
        tree.kill();
    }
    let _ = child.kill().await;
}
