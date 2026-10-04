use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

use super::spool::{Spool, Spooled};
use super::{command, required_str, Ask, Context, Output, Progress, RunFuture, Tool, ToolError};
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

    pub(crate) fn dialect(&self) -> command::Dialect {
        match self.shell {
            Shell::Bash(_) => command::Dialect::Bash,
            Shell::PowerShell(_) => command::Dialect::PowerShell,
        }
    }
}

/// `DRIFT_SHELL` names the shell outright. Otherwise Git's bash is checked before PATH, because
/// Windows ships a WSL stub named bash.exe in System32; PATH is read as it is now, not at startup.
fn detect_shell() -> Shell {
    if let Some(chosen) = std::env::var_os("DRIFT_SHELL").map(PathBuf::from).filter(|path| path.is_file()) {
        return shell_for(chosen);
    }
    let which = crate::platform::process::which;
    let real = |path: PathBuf| (!path.to_string_lossy().to_ascii_lowercase().contains("system32")).then_some(path);
    if let Some(bash) = git_bash().or_else(|| which("bash").and_then(real)) {
        return Shell::Bash(bash);
    }
    let pwsh = which("pwsh").or_else(|| which("powershell")).unwrap_or_else(|| "pwsh".into());
    Shell::PowerShell(pwsh)
}

/// A shell named by path: anything called bash, sh or zsh speaks bash; anything else is taken for PowerShell.
fn shell_for(path: PathBuf) -> Shell {
    let stem = path.file_stem().map(|stem| stem.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if matches!(stem.as_str(), "bash" | "sh" | "zsh") { Shell::Bash(path) } else { Shell::PowerShell(path) }
}

/// Git for Windows wherever it is installed: beside the `git` on PATH (Git puts only `cmd` there),
/// then the machine-wide, per-user and Scoop locations.
fn git_bash() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let env = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let scoop = env("SCOOP").or_else(|| env("USERPROFILE").map(|home| home.join("scoop")));
    let known = [
        Some(PathBuf::from("C:/Program Files/Git/bin/bash.exe")),
        Some(PathBuf::from("C:/Program Files (x86)/Git/bin/bash.exe")),
        env("LOCALAPPDATA").map(|local| local.join("Programs/Git/bin/bash.exe")),
        scoop.map(|scoop| scoop.join("apps/git/current/bin/bash.exe")),
    ];
    crate::platform::process::which("git").and_then(|git| bash_beside(&git)).or_else(|| known.into_iter().flatten().find(|candidate| candidate.is_file()))
}

/// `git.exe` sits in `<root>/cmd`, `<root>/bin` or `<root>/mingw64/bin`; its bash is `<root>/bin/bash.exe`.
fn bash_beside(git: &std::path::Path) -> Option<PathBuf> {
    git.ancestors().skip(1).take(3).map(|dir| dir.join("bin").join("bash.exe")).find(|candidate| candidate.is_file())
}

/// What the model must know about the shell, which differs most on Windows: Git's bash there is still Unix bash.
fn shell_note(shell: &Shell, windows: bool) -> &'static str {
    match shell {
        Shell::Bash(_) if windows => {
            "Git Bash on Windows. It is Unix bash, not cmd: discard output with `/dev/null`, never `NUL`; change directory with `cd`, never `cd /d`; write paths as `C:/dir/file` or `/c/dir/file`"
        }
        Shell::Bash(_) => "bash",
        Shell::PowerShell(path) if windows_powershell(path) => "Windows PowerShell 5.1 (powershell.exe, not PowerShell 7); use PowerShell 5.1 syntax, not bash",
        Shell::PowerShell(_) => "PowerShell 7 (pwsh); use PowerShell syntax, not bash",
    }
}

/// How dependent steps are chained: Windows PowerShell 5.1 has no `&&`.
fn chain_note(shell: &Shell) -> &'static str {
    match shell {
        Shell::PowerShell(path) if windows_powershell(path) => {
            "separate dependent steps with `;` and stop on failure yourself (`if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }`), since `&&` does not exist in this shell"
        }
        _ => "chain dependent steps with `&&`",
    }
}

/// `powershell.exe` is Windows PowerShell 5.1; PowerShell 7 is `pwsh`.
fn windows_powershell(path: &std::path::Path) -> bool {
    path.file_stem().is_some_and(|stem| stem.eq_ignore_ascii_case("powershell"))
}

impl Tool for Bash {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "bash".into(),
            description: include_str!("prompts/bash.txt").trim().replace("{shell}", shell_note(&self.shell, cfg!(windows))).replace("{chain}", chain_note(&self.shell)),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The command to run." },
                    "timeout": { "type": "integer", "description": "Milliseconds before the command is stopped. Default: the user's setting. Max 86400000." },
                    "description": { "type": "string", "description": "Five to ten words saying what the command does, shown to the user." },
                    "workdir": { "type": "string", "description": "Directory to run in, inside the workspace, instead of changing directory first. Default: the workspace root." }
                },
                "required": ["command"]
            }),
        }
    }

    fn ask(&self, ctx: &Context, input: &Value) -> Option<Ask> {
        let command = input["command"].as_str()?;
        let mut ask = Ask::shell(self.dialect(), command, input["description"].as_str().unwrap_or(command));
        // A workdir outside the workspace is refused when the call runs, so moves are judged from inside.
        let dir = workdir(ctx, input).unwrap_or_else(|_| ctx.workspace.clone());
        drop_moves_within(ctx, &dir, &mut ask);
        ask.default_allow = command::reads_only(self.dialect(), command) && reads_inside(ctx, &dir, &ask);
        Some(ask)
    }

    fn mutates(&self) -> bool {
        true
    }

    /// A line made only of commands known to read (`git status`, `ls`, `rg`, ...) with no redirection
    /// that writes is not captured before and after; anything else, or anything unclear, is.
    fn call_mutates(&self, input: &Value) -> bool {
        input["command"].as_str().is_none_or(|line| !command::reads_only(self.dialect(), line))
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let command = required_str(&input, "command")?;
            let dir = workdir(ctx, &input)?;
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
            cmd.current_dir(&dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
            crate::platform::process::use_current_path(&mut cmd, &Default::default());
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
                let collecting = bounded(limit, collect(&mut child, &mut spool, &ctx.progress));
                tokio::select! {
                    ended = collecting => ended.unwrap_or(Ended::TimedOut),
                    () = ctx.abort.cancelled() => Ended::Stopped,
                }
            };
            if !matches!(ended, Ended::Exited { lingering: false, .. }) {
                kill_tree(&tree, &mut child).await;
            }
            // A file the line printed has been seen, as a `read` would have shown it, so it may be edited.
            if matches!(ended, Ended::Exited { code: 0, .. }) {
                for file in command::files_read(self.dialect(), command).iter().map(|file| super::canonical(&dir.join(file))).filter(|file| file.is_file()) {
                    ctx.files.mark_read(&file);
                }
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
/// How often a running command's output so far is shown, and how much of its end.
const SHOW_EVERY: Duration = Duration::from_millis(500);
const SHOWN_BYTES: usize = 4 * 1024;

/// Reads stdout and stderr into one spool in arrival order until both close and the shell exits.
async fn collect(child: &mut tokio::process::Child, spool: &mut Spool, progress: &Progress) -> Ended {
    let (Some(mut stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Ended::Failed("the shell's output was not captured".into());
    };
    let (mut out_buf, mut err_buf) = ([0u8; 8192], [0u8; 8192]);
    let (mut out_open, mut err_open) = (true, true);
    let mut exited: Option<i32> = None;
    let drain = tokio::time::sleep(Duration::MAX);
    tokio::pin!(drain);
    let mut tick = tokio::time::interval(SHOW_EVERY);
    let mut shown = 0;
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
            _ = tick.tick(), if spool.total() != shown => {
                shown = spool.total();
                progress.show(json!({ "output": spool.recent(SHOWN_BYTES) }));
            }
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

const MOVES: [&str; 6] = ["cd", "chdir", "set-location", "sl", "pushd", "push-location"];

/// Readers of file contents under a whole directory: they would read `.env` and its kin too, which the
/// `grep` tool skips, so a line using one always asks.
const SEARCHERS: [&str; 3] = ["grep", "rg", "select-string"];

/// Whether a line already known to only read stays inside the workspace and names no file that may
/// hold secrets: no move is left (one leaving the workspace, or one that cannot be read, stays in the
/// ask), no recursive content search, no glob, no variable, and every argument that names a path
/// resolves inside the workspace. `--flag=value` and `rev:path` are judged by their path parts too.
fn reads_inside(ctx: &Context, dir: &std::path::Path, ask: &Ask) -> bool {
    let Some(commands) = &ask.commands else { return false };
    commands.iter().all(|command| {
        let words: Vec<&str> = command.split(' ').collect();
        let program = words[0].rsplit(['/', '\\']).next().unwrap_or(words[0]).trim_end_matches(".exe").to_ascii_lowercase();
        let searches = SEARCHERS.contains(&program.as_str()) || (program == "git" && words.get(1) == Some(&"grep"));
        !MOVES.contains(&program.as_str()) && !searches && words[1..].iter().all(|word| word_inside(ctx, dir, word))
    })
}

fn word_inside(ctx: &Context, dir: &std::path::Path, word: &str) -> bool {
    let word = word.trim_matches(['\'', '"']);
    if word.starts_with('~') || word.contains(['$', '%', '*', '?', '[', '`']) {
        return false;
    }
    let value = word.split_once('=').map_or(word, |(_, value)| value);
    let parts = [word, value, value.rsplit_once(':').map_or(value, |(_, path)| path)];
    parts.iter().filter(|part| !part.is_empty()).all(|part| {
        let path = super::canonical(&dir.join(part));
        // Anything on disk is judged where it resolves, so a plain `notes` linking outside the workspace still asks.
        let exists = std::fs::symlink_metadata(dir.join(part)).is_ok();
        let names_path = exists || part.contains(['/', '\\']) || part.starts_with('.') || std::path::Path::new(part).is_absolute();
        !super::sensitive::is_sensitive(&path) && (!names_path || ctx.inside_workspace(&path))
    })
}

/// Where a call runs: its `workdir`, which must be a directory inside the workspace, else the workspace.
fn workdir(ctx: &Context, input: &Value) -> Result<PathBuf, ToolError> {
    let Some(asked) = input["workdir"].as_str().filter(|dir| !dir.is_empty()) else { return Ok(ctx.workspace.clone()) };
    let dir = ctx.resolve(asked);
    if !ctx.inside_workspace(&dir) {
        return Err(ToolError(format!("workdir {asked} is outside the workspace; use `cd` in the command instead, which asks")));
    }
    if !dir.is_dir() {
        return Err(ToolError(format!("workdir {asked} is not a directory")));
    }
    Ok(dir)
}

/// Drops `cd` steps that stay inside the workspace: moving around it changes nothing, so it needs no
/// approval of its own and `cd crates && cargo test` asks only about `cargo test`. The directory is
/// followed along the chain from `start`; once a move leaves the workspace or cannot be read (`~`,
/// `-`, a variable, a glob), it and every later move still ask.
fn drop_moves_within(ctx: &Context, start: &std::path::Path, ask: &mut Ask) {
    let mut here = Some(start.to_path_buf());
    ask.retain_commands(|command| {
        let Some(from) = &here else { return true };
        match move_within(ctx, from, command) {
            Move::Inside(next) => {
                here = Some(next);
                false
            }
            Move::Elsewhere => {
                here = None;
                true
            }
            Move::None => true,
        }
    });
}

enum Move {
    Inside(PathBuf),
    Elsewhere,
    None,
}

fn move_within(ctx: &Context, from: &std::path::Path, command: &str) -> Move {
    let words: Vec<&str> = command.split(' ').collect();
    if !MOVES.contains(&words[0].to_ascii_lowercase().as_str()) {
        return Move::None;
    }
    let [_, target] = words[..] else { return Move::Elsewhere };
    if target.starts_with(['-', '~']) || target.contains(['$', '%', '*', '?', '[']) {
        return Move::Elsewhere;
    }
    let next = super::canonical(&from.join(target));
    if ctx.inside_workspace(&next) {
        Move::Inside(next)
    } else {
        Move::Elsewhere
    }
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
    async fn a_workdir_inside_the_workspace_is_where_it_runs_and_one_outside_is_refused() {
        let sandbox = Sandbox::new("bash-workdir");
        let file = sandbox.file("sub/here.txt", "here\n");
        let bash = Bash::detect();
        let print = match bash.shell {
            Shell::Bash(_) => "cat here.txt",
            Shell::PowerShell(_) => "Get-Content here.txt",
        };
        let out = bash.run(&sandbox.ctx, json!({ "command": print, "workdir": "sub" })).await.unwrap();
        assert!(out.output.contains("here"), "{}", out.output);
        assert!(sandbox.ctx.files.was_read(&file), "a file it printed is found from the workdir");
        let outside = bash.run(&sandbox.ctx, json!({ "command": print, "workdir": ".." })).await.unwrap_err();
        assert!(outside.0.contains("outside the workspace"), "{}", outside.0);
        let ask = bash.ask(&sandbox.ctx, &json!({ "command": "cd .. && cargo test", "workdir": "sub" })).unwrap();
        assert_eq!(ask.commands.unwrap(), ["cargo test"], "a move from the workdir that stays inside asks nothing");
    }

    #[tokio::test]
    async fn a_file_printed_by_a_command_that_succeeded_counts_as_read() {
        let sandbox = Sandbox::new("bash-reads");
        let shown = sandbox.file("shown.txt", "one\n");
        let missed = sandbox.file("missed.txt", "two\n");
        let bash = Bash::detect();
        let (print, fail) = match bash.shell {
            Shell::Bash(_) => ("cat shown.txt", "cat missed.txt && exit 1"),
            Shell::PowerShell(_) => ("Get-Content shown.txt", "Get-Content missed.txt; exit 1"),
        };
        bash.run(&sandbox.ctx, json!({ "command": print })).await.unwrap();
        bash.run(&sandbox.ctx, json!({ "command": fail })).await.unwrap();
        assert!(sandbox.ctx.files.was_read(&shown), "edit may follow a shell read");
        assert!(!sandbox.ctx.files.was_read(&missed), "a line that failed is not trusted to have shown it");
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

    #[test]
    fn reading_lines_inside_the_workspace_run_without_asking_and_anything_else_asks() {
        let sandbox = Sandbox::new("bash-reads");
        std::fs::create_dir_all(sandbox.ctx.workspace.join("src")).unwrap();
        let bash = Bash::with(Shell::Bash("bash".into()));
        let decide = |line: &str| sandbox.ctx.engine.permissions.decide_now("ses_test", &crate::permission::Policy::default(), &bash.ask(&sandbox.ctx, &json!({ "command": line })).unwrap());
        for line in ["git status", "git log --oneline -10", "ls src", "cat README.md", "cd src && ls", "git diff HEAD~1..HEAD -- src/a.rs", "wc -l src/a.rs"] {
            assert_eq!(decide(line), crate::permission::Decision::Allow, "{line}");
        }
        for line in ["cat .env", "ls ..", "cat /etc/passwd", "grep -r token .", "rg token", "ls *", "git show HEAD:.env", "echo $HOME", "cargo test", "cd .. && ls", "ls > out.txt", "cat ~/.ssh/config"] {
            assert_eq!(decide(line), crate::permission::Decision::Ask, "{line}");
        }
        let outside = sandbox.ctx.workspace.parent().unwrap().join("outside-dir");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("private.txt"), "private").unwrap();
        let link = sandbox.ctx.workspace.join("linkdir");
        // A directory junction needs no elevation on Windows, unlike a symlink, so this always runs there.
        #[cfg(windows)]
        let made = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&link).arg(&outside).output().unwrap().status.success();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&outside, &link).is_ok();
        assert!(made, "the link could not be made");
        assert_eq!(decide("ls linkdir"), crate::permission::Decision::Ask, "a bare name linking outside the workspace asks");
        assert_eq!(decide("cat linkdir/private.txt"), crate::permission::Decision::Ask);
        sandbox.ctx.engine.permissions.set_policy(crate::permission::Policy { rules: vec![crate::permission::Rule { kind: "bash".into(), pattern: "git log*".into(), decision: crate::permission::Decision::Ask }] });
        assert_eq!(decide("git log --oneline"), crate::permission::Decision::Ask, "a rule still decides first");
    }

    #[test]
    fn git_bash_is_found_beside_any_git_and_a_named_shell_is_taken_at_its_word() {
        let root = std::env::temp_dir().join(format!("drift-git-{}", crate::random_hex(4)));
        for dir in ["cmd", "bin", "mingw64/bin"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("bin/bash.exe"), "").unwrap();
        for git in ["cmd/git.exe", "bin/git.exe", "mingw64/bin/git.exe"] {
            assert_eq!(bash_beside(&root.join(git)), Some(root.join("bin").join("bash.exe")), "{git}");
        }
        assert_eq!(bash_beside(&std::env::temp_dir().join("elsewhere/git.exe")), None);
        std::fs::remove_dir_all(&root).unwrap();
        assert!(matches!(shell_for("D:/tools/Git/bin/bash.exe".into()), Shell::Bash(_)));
        assert!(matches!(shell_for("/usr/bin/zsh".into()), Shell::Bash(_)));
        assert!(matches!(shell_for("C:/Program Files/PowerShell/7/pwsh.exe".into()), Shell::PowerShell(_)));
    }

    #[test]
    fn the_model_is_told_git_bash_on_windows_is_unix_bash() {
        let bash = Shell::Bash("bash".into());
        let windows = shell_note(&bash, true);
        assert!(windows.contains("Unix bash") && windows.contains("/dev/null") && windows.contains("never `NUL`") && windows.contains("never `cd /d`"), "{windows}");
        assert_eq!(shell_note(&bash, false), "bash");
        assert!(shell_note(&Shell::PowerShell("pwsh".into()), true).starts_with("PowerShell 7"));
        let legacy = Shell::PowerShell("C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe".into());
        assert!(shell_note(&legacy, true).starts_with("Windows PowerShell 5.1"));
        let legacy_spec = Bash::with(legacy).spec().description;
        assert!(legacy_spec.contains("`&&` does not exist") && !legacy_spec.contains("chain dependent steps with `&&`"), "{legacy_spec}");
        assert!(Bash::with(Shell::PowerShell("pwsh".into())).spec().description.contains("chain dependent steps with `&&`"));
        let spec = Bash::with(bash).spec();
        assert!(spec.description.contains("/dev/null") == cfg!(windows), "{}", spec.description);
        assert!(!spec.description.contains('{'), "every placeholder is filled: {}", spec.description);
    }

    #[test]
    fn moving_around_the_workspace_asks_nothing_and_leaving_it_still_asks() {
        let sandbox = Sandbox::new("bash-cd");
        sandbox.file("crates/a/Cargo.toml", "");
        let bash = Bash::with(Shell::Bash("bash".into()));
        let commands = |line: &str| bash.ask(&sandbox.ctx, &json!({ "command": line })).unwrap().commands;
        assert_eq!(commands("cd crates && cargo test").unwrap(), ["cargo test"]);
        assert_eq!(commands("cd crates && cd a && cargo build && cd ../.. && git status").unwrap(), ["cargo build", "git status"], "followed along the chain");
        assert_eq!(commands("cd src").unwrap(), Vec::<String>::new(), "a move alone changes nothing");
        assert_eq!(commands("cd .. && cargo test").unwrap(), ["cd ..", "cargo test"], "leaving the workspace asks");
        assert_eq!(commands("cd crates && cd ../.. && cd ws && ls").unwrap(), ["cd ../..", "cd ws", "ls"], "once outside, every later move asks");
        for line in ["cd ~ && ls", "cd - && ls", "cd $HOME && ls", "cd /etc && ls", "cd -P crates && ls", "cd cra* && ls"] {
            assert_eq!(commands(line).unwrap().len(), 2, "{line}");
        }
        let pwsh = Bash::with(Shell::PowerShell("pwsh".into()));
        let ask = pwsh.ask(&sandbox.ctx, &json!({ "command": "Set-Location crates; cargo test" })).unwrap();
        assert_eq!(ask.commands.unwrap(), ["cargo test"]);
        assert_eq!(ask.pattern, "Set-Location crates; cargo test", "the user still sees the whole line");
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
