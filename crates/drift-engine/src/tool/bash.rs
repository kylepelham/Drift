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

use approval::{drop_moves_within, why_it_asks, workdir};
use output::{Ended, collect, report};
pub use shell::Shell;
use shell::{chain_note, detect_shell, shell_note};

/// Until the user's Settings value arrives.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// The longest a model may ask for, the same ceiling as the Settings choice (1,440 minutes).
const MAX_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

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

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "bash".into(),
            description: include_str!("prompts/bash.txt")
                .trim()
                .replace("{shell}", shell_note(&self.shell, cfg!(windows)))
                .replace("{chain}", chain_note(&self.shell)),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The command to run." },
                    "timeout": { "type": "integer", "description": "Milliseconds before the command moves to the background. Default: the user's setting. Max 86400000." },
                    "background": { "type": "boolean", "description": "Start it in the background and return its process id at once, for servers, watchers and long builds. Default false." },
                    "description": { "type": "string", "description": "Five to ten words saying what the command does, shown to the user." },
                    "workdir": { "type": "string", "description": "Directory to run in, inside the workspace, instead of changing directory first. Default: the workspace root." }
                },
                "required": ["command"]
            }),
        }
    }

    fn ask(&self, context: &Context, input: &Value) -> Option<Ask> {
        let command = input["command"].as_str()?;
        let mut ask = Ask::shell(
            self.dialect(),
            command,
            input["description"].as_str().unwrap_or(command),
        );
        // Invalid workdirs are refused at execution, so permission analysis starts within the workspace.
        let directory = workdir(context, input).unwrap_or_else(|_| context.workspace.clone());
        drop_moves_within(context, &directory, &mut ask);
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
            let started = Started {
                command: command.to_string(),
                title: input["description"].as_str().unwrap_or(command).to_string(),
                running: spawn(&self.shell, command, &directory, background)?,
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

impl Bash {
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

        if matches!(ended, Ended::TimedOut) {
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

/// Starts the shell; a background command gets an input pipe so the model can type into it.
fn spawn(shell: &Shell, line: &str, directory: &Path, background: bool) -> Result<Running, ToolError> {
    let mut process = shell_command(shell, line);
    let stdin = if background { Stdio::piped() } else { Stdio::null() };
    process
        .current_dir(directory)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::platform::process::use_current_path(&mut process, &Default::default());
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
    #[cfg(windows)]
    resume_main_thread(&child);
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

/// The model's `timeout` when it gives one, otherwise the user's Settings value; `None` never stops it.
fn limit_for(context: &Context, input: &Value) -> Option<Duration> {
    match input["timeout"].as_u64() {
        Some(milliseconds) => Some(Duration::from_millis(milliseconds).min(MAX_TIMEOUT)),
        None => context.engine.shell_timeout(),
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

/// Children start suspended so they join the job before running anything; this lets them go.
#[cfg(windows)]
fn resume_main_thread(child: &tokio::process::Child) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    let Some(pid) = child.id() else { return };
    // SAFETY: entry is correctly sized; the snapshot and every opened thread handle are closed after use.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        if Thread32First(snapshot, &mut entry) != 0 {
            loop {
                if entry.th32OwnerProcessID == pid {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !thread.is_null() {
                        ResumeThread(thread);
                        CloseHandle(thread);
                    }
                }
                if Thread32Next(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
    }
}
