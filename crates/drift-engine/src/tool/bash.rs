use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

use super::{required_str, Ask, Context, Output, RunFuture, Tool, ToolError};
use crate::llm::ToolSpec;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_OUTPUT_CHARS: usize = 30_000;

#[derive(Clone, Debug, PartialEq)]
pub enum Shell {
    Bash(PathBuf),
    PowerShell(PathBuf),
}

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
}

/// Git's bash is checked before PATH because Windows ships a WSL stub named bash.exe in System32.
fn detect_shell() -> Shell {
    let real = |path: PathBuf| (!path.to_string_lossy().to_ascii_lowercase().contains("system32")).then_some(path);
    if let Some(bash) = git_bash().or_else(|| find_on_path("bash").and_then(real)) {
        return Shell::Bash(bash);
    }
    let pwsh = find_on_path("pwsh").or_else(|| find_on_path("powershell")).unwrap_or_else(|| "pwsh".into());
    Shell::PowerShell(pwsh)
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    std::env::var_os("PATH")?
        .to_str()?
        .split(if cfg!(windows) { ';' } else { ':' })
        .map(|dir| PathBuf::from(dir).join(&exe))
        .find(|candidate| candidate.is_file())
}

fn git_bash() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    ["C:/Program Files/Git/bin/bash.exe", "C:/Program Files (x86)/Git/bin/bash.exe"]
        .into_iter()
        .map(PathBuf::from)
        .find(|candidate| candidate.is_file())
}

impl Tool for Bash {
    fn spec(&self) -> ToolSpec {
        let shell = match &self.shell {
            Shell::Bash(_) => "bash",
            Shell::PowerShell(_) => "PowerShell 7 (pwsh); use PowerShell syntax, not bash",
        };
        ToolSpec {
            name: "bash".into(),
            description: include_str!("prompts/bash.txt").trim().replace("{shell}", shell),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The command to run." },
                    "timeout": { "type": "integer", "description": "Milliseconds before the command is killed. Default 120000, max 600000." },
                    "description": { "type": "string", "description": "Five to ten words saying what the command does, shown to the user." }
                },
                "required": ["command"]
            }),
        }
    }

    fn ask(&self, _ctx: &Context, input: &Value) -> Option<Ask> {
        let command = input["command"].as_str()?;
        Some(Ask { kind: "bash".into(), pattern: command.into(), title: input["description"].as_str().unwrap_or(command).into() })
    }

    fn mutates(&self) -> bool {
        true
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let command = required_str(&input, "command")?;
            let timeout = input["timeout"].as_u64().map_or(DEFAULT_TIMEOUT, Duration::from_millis).min(MAX_TIMEOUT);
            let mut cmd = match &self.shell {
                Shell::Bash(bash) => {
                    let mut cmd = tokio::process::Command::new(bash);
                    cmd.arg("-c").arg(command);
                    cmd
                }
                Shell::PowerShell(pwsh) => {
                    let mut cmd = tokio::process::Command::new(pwsh);
                    cmd.args(["-NoProfile", "-NonInteractive", "-Command", command]);
                    cmd
                }
            };
            cmd.current_dir(&ctx.workspace).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
            #[cfg(windows)]
            cmd.creation_flags(0x0800_0000);
            let mut child = cmd.spawn().map_err(|e| ToolError(format!("could not start shell: {e}")))?;
            let mut stdout = child.stdout.take().unwrap();
            let mut stderr = child.stderr.take().unwrap();
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let run = async {
                let (a, b, status) = tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err), child.wait());
                a?;
                b?;
                status
            };
            let outcome = tokio::select! {
                status = tokio::time::timeout(timeout, run) => status,
                () = ctx.abort.cancelled() => {
                    let _ = child.kill().await;
                    return Err(ToolError("aborted".into()));
                }
            };
            let status = match outcome {
                Ok(Ok(status)) => status,
                Ok(Err(error)) => return Err(ToolError(error.to_string())),
                Err(_) => {
                    let _ = child.kill().await;
                    return Err(ToolError(format!("command timed out after {} s", timeout.as_secs())));
                }
            };
            let mut text = String::from_utf8_lossy(&out).into_owned();
            if !err.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&String::from_utf8_lossy(&err));
            }
            let text = clip(text.trim_end());
            let code = status.code().unwrap_or(-1);
            let output = if status.success() { text } else { format!("{text}\n\nexit code {code}").trim_start().into() };
            Ok(Output {
                title: input["description"].as_str().unwrap_or(command).into(),
                output,
                metadata: json!({ "exit": code }),
            })
        })
    }
}

fn clip(text: &str) -> String {
    if text.chars().count() <= MAX_OUTPUT_CHARS {
        return text.to_string();
    }
    let half = MAX_OUTPUT_CHARS / 2;
    let head: String = text.chars().take(half).collect();
    let tail: String = text.chars().rev().take(half).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{head}\n\n... output clipped ...\n\n{tail}")
}

#[cfg(test)]
mod tests {
    use super::super::tests::Sandbox;
    use super::*;

    #[tokio::test]
    async fn runs_a_command_in_the_workspace_and_reports_exit_codes() {
        let sandbox = Sandbox::new("bash");
        sandbox.file("hello.txt", "hi");
        let bash = Bash::detect();
        let list = match bash.shell {
            Shell::Bash(_) => "ls && exit 3",
            Shell::PowerShell(_) => "Get-ChildItem -Name; exit 3",
        };
        let out = bash.run(&sandbox.ctx, json!({ "command": list, "description": "list files" })).await.unwrap();
        assert!(out.output.contains("hello.txt"), "{}", out.output);
        assert!(out.output.ends_with("exit code 3"));
        assert_eq!(out.title, "list files");
        assert_eq!(out.metadata["exit"], 3);
    }

    #[tokio::test]
    async fn times_out_and_aborts() {
        let sandbox = Sandbox::new("bash-timeout");
        let bash = Bash::detect();
        let sleep = match bash.shell {
            Shell::Bash(_) => "sleep 5",
            Shell::PowerShell(_) => "Start-Sleep 5",
        };
        let err = bash.run(&sandbox.ctx, json!({ "command": sleep, "timeout": 300 })).await.unwrap_err();
        assert!(err.0.contains("timed out"));
        sandbox.ctx.abort.cancel();
        let err = bash.run(&sandbox.ctx, json!({ "command": sleep })).await.unwrap_err();
        assert_eq!(err.0, "aborted");
    }

    #[test]
    fn long_output_keeps_head_and_tail() {
        let long = "x".repeat(MAX_OUTPUT_CHARS * 2);
        let clipped = clip(&long);
        assert!(clipped.contains("output clipped"));
        assert!(clipped.len() < long.len());
    }
}
