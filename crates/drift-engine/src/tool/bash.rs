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
            cmd.creation_flags(0x0800_0000 | 0x0000_0004);
            crate::platform::process::prepare(&mut cmd);
            let mut child = cmd.spawn().map_err(|e| ToolError(format!("could not start shell: {e}")))?;
            // Dropping 	ree for any reason, including this future being dropped, kills every descendant.
            let tree = child.id().and_then(|pid| crate::platform::process::Tree::adopt(pid).ok());
            #[cfg(windows)]
            resume_main_thread(&child);
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
                    kill_tree(&tree, &mut child).await;
                    return Err(ToolError("aborted".into()));
                }
            };
            let status = match outcome {
                Ok(Ok(status)) => status,
                Ok(Err(error)) => return Err(ToolError(error.to_string())),
                Err(_) => {
                    kill_tree(&tree, &mut child).await;
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
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32};
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
    let Some(pid) = child.id() else { return };
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

#[cfg(test)]
mod tree_tests {
    use super::super::tests::Sandbox;
    use super::*;

    #[tokio::test]
    async fn aborting_the_shell_stops_its_descendants() {
        let sandbox = Sandbox::new("bash-tree");
        let bash = Bash::detect();
        let command = match bash.shell {
            Shell::Bash(_) => "(sleep 2; echo late > late.txt) & sleep 30",
            Shell::PowerShell(_) => "Start-Process pwsh -WindowStyle Hidden -ArgumentList '-NoProfile','-Command','Start-Sleep 2; Set-Content late.txt late'; Start-Sleep 30",
        };
        let ctx = sandbox.ctx_clone();
        let abort = ctx.abort.clone();
        let running = tokio::spawn(async move { Bash::detect().run(&ctx, json!({ "command": command })).await });
        tokio::time::sleep(Duration::from_millis(600)).await;
        abort.cancel();
        let result = running.await.unwrap();
        assert_eq!(result.unwrap_err().0, "aborted");
        tokio::time::sleep(Duration::from_millis(3000)).await;
        assert!(!sandbox.ctx.workspace.join("late.txt").exists(), "a descendant kept running after Stop");
    }

    #[tokio::test]
    async fn dropping_the_run_future_also_stops_descendants() {
        let sandbox = Sandbox::new("bash-drop");
        let bash = Bash::detect();
        let command = match bash.shell {
            Shell::Bash(_) => "(sleep 2; echo late > dropped.txt) & sleep 30",
            Shell::PowerShell(_) => "Start-Process pwsh -WindowStyle Hidden -ArgumentList '-NoProfile','-Command','Start-Sleep 2; Set-Content dropped.txt late'; Start-Sleep 30",
        };
        let ctx = sandbox.ctx_clone();
        let handle = tokio::spawn(async move { Bash::detect().run(&ctx, json!({ "command": command })).await });
        tokio::time::sleep(Duration::from_millis(600)).await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(3000)).await;
        assert!(!sandbox.ctx.workspace.join("dropped.txt").exists(), "a descendant survived the future being dropped");
    }
}
