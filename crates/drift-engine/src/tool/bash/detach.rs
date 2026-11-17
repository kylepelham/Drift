//! A program the user wants left running on its own (a desktop app, a browser, a service): started outside the job
//! that holds a command's processes, so neither the shell ending, Stop nor Drift closing ends it.

use super::{Shell, managed_keys, shell_command};
use crate::tool::{Context, Output, ToolError};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

/// How long the launching shell is watched: a launcher such as `start` returns at once, and a failure shows by then.
const WATCH: Duration = Duration::from_secs(5);
/// How much of what the launch printed comes back.
const SHOWN_BYTES: usize = 4 * 1024;

/// Starts `line` detached, watches the shell for a moment, and says what became of it.
pub(super) async fn start(
    context: &Context,
    shell: &Shell,
    line: &str,
    directory: &Path,
    title: String,
) -> Result<Output, ToolError> {
    let log = crate::platform::temp::folder("detached").join(format!(
        "{}-{}.log",
        context.session_id,
        crate::tool::call_file_name(&context.call_id)
    ));
    let file = std::fs::File::create(&log).map_err(|error| ToolError(format!("could not start it: {error}")))?;
    let began = std::time::SystemTime::now();

    let mut command = shell_command(shell, line);
    command
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            file.try_clone().map_err(|error| ToolError(error.to_string()))?,
        ))
        .stderr(Stdio::from(file))
        .kill_on_drop(false)
        .env("PATH", crate::platform::process::path_with(&context.config.programs));
    for name in managed_keys(&context.engine) {
        command.env_remove(name);
    }
    let mut child = spawn_detached(command).map_err(|error| ToolError(format!("could not start it: {error}")))?;
    let pid = child.id();

    let exited = tokio::select! {
        status = tokio::time::timeout(WATCH, child.wait()) => status.ok().and_then(Result::ok),
        () = context.abort.cancelled() => None,
    };
    context.files.shell_ran(began, std::time::SystemTime::now());
    let printed = printed(&log);

    Ok(Output::new(
        title,
        describe(exited.map(|status| status.code()), pid, &printed, &log),
    ))
}

fn describe(exited: Option<Option<i32>>, pid: Option<u32>, printed: &str, log: &Path) -> String {
    let said = if printed.trim().is_empty() {
        String::new()
    } else {
        format!("\n\nIt printed:\n{}", printed.trim_end())
    };
    match exited {
        Some(Some(0)) => format!(
            "Started detached; the launching command finished, and what it started keeps running on its own. \
             Drift will not stop it.{said}"
        ),
        Some(code) => format!(
            "The launching command failed (exit code {}), so the program most likely did not start.{said}",
            code.map_or_else(|| "none".into(), |code| code.to_string())
        ),
        None => format!(
            "Running detached{}; Drift will not stop it or collect more of its output. It writes to {}.{said}",
            pid.map(|pid| format!(" as process {pid}")).unwrap_or_default(),
            log.display()
        ),
    }
}

/// The start of what the launch printed so far.
fn printed(log: &Path) -> String {
    let bytes = std::fs::read(log).unwrap_or_default();
    let shown = &bytes[..bytes.len().min(SHOWN_BYTES)];
    let mut text = String::from_utf8_lossy(shown).into_owned();
    if bytes.len() > SHOWN_BYTES {
        text.push_str("\n...");
    }

    text
}

/// No window of its own and out of any job Drift itself runs in, where the job allows it.
#[cfg(windows)]
fn spawn_detached(mut command: tokio::process::Command) -> std::io::Result<tokio::process::Child> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
    match command.spawn() {
        // A job around Drift that refuses breakaway refuses the flag; its own processes outlive Drift then or never.
        Err(error) if error.raw_os_error() == Some(5) => {
            command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
            command.spawn()
        }
        spawned => spawned,
    }
}

/// Its own session, so closing the terminal that started Drift sends it no hangup.
#[cfg(unix)]
fn spawn_detached(mut command: tokio::process::Command) -> std::io::Result<tokio::process::Child> {
    // SAFETY: setsid only changes this child's own session and is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    command.spawn()
}
