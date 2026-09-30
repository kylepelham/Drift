use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

use super::spool::{Spool, Spooled};
use super::{command, required_str, Ask, Context, Output, RunFuture, Tool, ToolError};
use crate::llm::ToolSpec;

/// Until the user's Settings value arrives.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// The longest a model may ask for, the same ceiling as the Settings choice (1,440 minutes).
const MAX_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

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
                    "timeout": { "type": "integer", "description": "Milliseconds before the command is stopped. Default: the user's setting. Max 86400000." },
                    "description": { "type": "string", "description": "Five to ten words saying what the command does, shown to the user." }
                },
                "required": ["command"]
            }),
        }
    }

    fn ask(&self, _ctx: &Context, input: &Value) -> Option<Ask> {
        let command = input["command"].as_str()?;
        let dialect = match self.shell {
            Shell::Bash(_) => command::Dialect::Bash,
            Shell::PowerShell(_) => command::Dialect::PowerShell,
        };
        Some(Ask::shell(dialect, command, input["description"].as_str().unwrap_or(command)))
    }

    fn mutates(&self) -> bool {
        true
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let command = required_str(&input, "command")?;
            let limit = limit_for(ctx, &input);
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
            // Dropping `tree` for any reason, including this future being dropped, kills every descendant.
            let tree = child.id().and_then(|pid| crate::platform::process::Tree::adopt(pid).ok());
            #[cfg(windows)]
            resume_main_thread(&child);
            let path = ctx.engine.data_dir.join("tool-output").join(&ctx.session_id).join(format!("{}.log", ctx.call_id));
            let mut spool = Spool::new(Some(path));
            let ended = {
                let collecting = bounded(limit, collect(&mut child, &mut spool));
                tokio::select! {
                    ended = collecting => ended.unwrap_or(Ended::TimedOut),
                    () = ctx.abort.cancelled() => Ended::Stopped,
                }
            };
            if !matches!(ended, Ended::Exited { lingering: false, .. }) {
                kill_tree(&tree, &mut child).await;
            }
            let title = input["description"].as_str().unwrap_or(command).to_string();
            Ok(report(title, spool.finish(), ended, limit))
        })
    }

    /// Stop ends the command at once and keeps what it printed, so the turn awaits it rather than racing it.
    fn stops_itself(&self) -> bool {
        true
    }

    /// The limit shows while the command runs, so the user can see when it will be stopped.
    fn running_metadata(&self, ctx: &Context, input: &Value) -> Option<Value> {
        Some(json!({ "shellTimeoutMs": limit_for(ctx, input).map(|d| d.as_millis() as u64) }))
    }

    /// A command stopped by its time limit or by the user failed, though its partial output still matters.
    fn failed(&self, output: &Output) -> bool {
        output.metadata["timedOut"] == true || output.metadata["stopped"] == true
    }
}

/// How a command's run ended.
enum Ended {
    /// `lingering`: a background process still held its output open after the shell exited.
    Exited { code: i32, lingering: bool },
    TimedOut,
    Stopped,
    Failed(String),
}

/// After the shell exits, output still in flight gets this long to arrive; a pipe open past it is
/// held by a background process, which does not outlive the call.
const DRAIN: Duration = Duration::from_millis(500);

/// Reads stdout and stderr into one spool in arrival order until both close and the shell exits.
async fn collect(child: &mut tokio::process::Child, spool: &mut Spool) -> Ended {
    let (Some(mut stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Ended::Failed("the shell's output was not captured".into());
    };
    let (mut out_buf, mut err_buf) = ([0u8; 8192], [0u8; 8192]);
    let (mut out_open, mut err_open) = (true, true);
    let mut exited: Option<i32> = None;
    let drain = tokio::time::sleep(Duration::MAX);
    tokio::pin!(drain);
    loop {
        if let (Some(code), false, false) = (exited, out_open, err_open) {
            return Ended::Exited { code, lingering: false };
        }
        tokio::select! {
            read = stdout.read(&mut out_buf), if out_open => out_open = take(read, &out_buf, spool),
            read = stderr.read(&mut err_buf), if err_open => err_open = take(read, &err_buf, spool),
            status = child.wait(), if exited.is_none() => {
                exited = Some(status.map_or(-1, |s| s.code().unwrap_or(-1)));
                drain.as_mut().reset(tokio::time::Instant::now() + DRAIN);
            }
            () = &mut drain, if exited.is_some() => return Ended::Exited { code: exited.unwrap_or(-1), lingering: true },
        }
    }
}

/// Spools what a read returned; `false` once the pipe is closed or broken.
fn take(read: std::io::Result<usize>, buffer: &[u8], spool: &mut Spool) -> bool {
    match read {
        Ok(0) | Err(_) => false,
        Ok(n) => {
            spool.push(&buffer[..n]);
            true
        }
    }
}

fn report(title: String, spooled: Spooled, ended: Ended, limit: Option<Duration>) -> Output {
    let text = spooled.text.trim_end().to_string();
    let mut metadata = json!({ "shellTimeoutMs": limit.map(|d| d.as_millis() as u64), "outputBytes": spooled.total });
    if let Some(file) = &spooled.file {
        metadata["outputFile"] = json!(file.to_string_lossy());
    }
    let note = match ended {
        Ended::Exited { code, lingering } => {
            metadata["exit"] = json!(code);
            let lingered = if lingering { "\n\nBackground processes still held the output open when the command finished; they were stopped. Run long-lived processes outside Drift." } else { "" };
            let failed = if code == 0 { String::new() } else { format!("\n\nexit code {code}") };
            format!("{lingered}{failed}")
        }
        Ended::TimedOut => {
            metadata["timedOut"] = json!(true);
            let seconds = limit.map_or(0, |d| d.as_secs());
            format!("\n\nThe command and its child processes were stopped after {seconds} s. If it needs longer and is not waiting for input, run it again with a larger `timeout` in milliseconds.")
        }
        Ended::Stopped => {
            metadata["stopped"] = json!(true);
            "\n\nThe user stopped the command and its child processes.".into()
        }
        Ended::Failed(error) => format!("\n\n{error}"),
    };
    Output { title, output: format!("{text}{note}").trim_start().into(), metadata }
}

/// The model's `timeout` when it gives one, otherwise the user's Settings value; `None` never stops it.
fn limit_for(ctx: &Context, input: &Value) -> Option<Duration> {
    match input["timeout"].as_u64() {
        Some(ms) => Some(Duration::from_millis(ms).min(MAX_TIMEOUT)),
        None => ctx.engine.shell_timeout(),
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
        let early_then_sleep = match bash.shell {
            Shell::Bash(_) => "echo early; sleep 5",
            Shell::PowerShell(_) => "Write-Output early; Start-Sleep 5",
        };
        let stopped = bash.run(&sandbox.ctx, json!({ "command": early_then_sleep, "timeout": 1500 })).await.unwrap();
        assert!(bash.failed(&stopped), "a command stopped by its limit is a failed call");
        assert_eq!((stopped.metadata["timedOut"].as_bool(), stopped.metadata["shellTimeoutMs"].as_u64()), (Some(true), Some(1500)));
        assert!(stopped.output.contains("stopped after"), "{}", stopped.output);
        assert!(stopped.output.starts_with("early"), "what it printed before the limit is kept: {}", stopped.output);

        sandbox.ctx.engine.set_shell_timeout(Some(Duration::from_millis(300)));
        assert_eq!(bash.running_metadata(&sandbox.ctx, &json!({ "command": sleep })).unwrap()["shellTimeoutMs"], 300);
        let by_setting = bash.run(&sandbox.ctx, json!({ "command": sleep })).await.unwrap();
        assert_eq!(by_setting.metadata["timedOut"], true, "without a `timeout` the Settings limit applies");

        sandbox.ctx.engine.set_shell_timeout(None);
        let quick = match bash.shell {
            Shell::Bash(_) => "sleep 1",
            Shell::PowerShell(_) => "Start-Sleep 1",
        };
        let unlimited = bash.run(&sandbox.ctx, json!({ "command": quick })).await.unwrap();
        assert!(!bash.failed(&unlimited) && unlimited.metadata["shellTimeoutMs"].is_null(), "no limit lets it finish");
    }

    #[tokio::test]
    async fn stop_keeps_what_the_command_printed() {
        let sandbox = Sandbox::new("bash-stop");
        let bash = Bash::detect();
        let command = match bash.shell {
            Shell::Bash(_) => "echo early; sleep 30",
            Shell::PowerShell(_) => "Write-Output early; Start-Sleep 30",
        };
        let ctx = sandbox.ctx_clone();
        let abort = ctx.abort.clone();
        let running = tokio::spawn(async move { Bash::detect().run(&ctx, json!({ "command": command })).await });
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let started = std::time::Instant::now();
        abort.cancel();
        let out = running.await.unwrap().unwrap();
        assert!(started.elapsed() < Duration::from_secs(3), "Stop is prompt");
        assert!(bash.failed(&out) && out.metadata["stopped"] == true);
        assert!(out.output.starts_with("early") && out.output.contains("stopped the command"), "{}", out.output);
    }

    #[tokio::test]
    async fn a_background_process_holding_the_output_does_not_keep_the_call_waiting() {
        let sandbox = Sandbox::new("bash-lingering");
        sandbox.ctx.engine.set_shell_timeout(None);
        let bash = Bash::detect();
        let command = match bash.shell {
            Shell::Bash(_) => "sleep 30 & echo done",
            Shell::PowerShell(_) => "Start-Process -NoNewWindow pwsh -ArgumentList '-NoProfile','-Command','Start-Sleep 30'; Write-Output done",
        };
        let started = std::time::Instant::now();
        let out = bash.run(&sandbox.ctx, json!({ "command": command })).await.unwrap();
        assert!(started.elapsed() < Duration::from_secs(10), "no limit, yet it returns: {:?}", started.elapsed());
        assert!(out.output.starts_with("done"), "{}", out.output);
        assert_eq!(out.metadata["exit"], 0);
        assert!(!bash.failed(&out));
        if matches!(bash.shell, Shell::Bash(_)) {
            assert!(out.output.contains("Background processes still held the output open"), "{}", out.output);
        }
    }

    #[tokio::test]
    async fn large_output_is_bounded_in_the_result_and_whole_on_disk() {
        let sandbox = Sandbox::new("bash-large");
        let bash = Bash::detect();
        let command = match bash.shell {
            Shell::Bash(_) => "for i in $(seq 1 40000); do echo line $i; done",
            Shell::PowerShell(_) => "1..40000 | ForEach-Object { \"line $_\" }",
        };
        let out = bash.run(&sandbox.ctx, json!({ "command": command })).await.unwrap();
        assert!(out.output.len() < super::super::spool::HEAD_BYTES + super::super::spool::TAIL_BYTES + 400, "{}", out.output.len());
        assert!(out.output.starts_with("line 1") && out.output.contains("line 40000"));
        let file = out.metadata["outputFile"].as_str().expect("the whole output is kept");
        let whole = std::fs::read_to_string(file).unwrap();
        assert_eq!(whole.lines().count(), 40000);
        assert_eq!(out.metadata["outputBytes"].as_u64(), Some(std::fs::metadata(file).unwrap().len()));
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
        let result = running.await.unwrap().unwrap();
        assert_eq!(result.metadata["stopped"], true);
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
