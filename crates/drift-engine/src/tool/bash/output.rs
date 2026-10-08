use crate::tool::spool::{Spool, Spooled};
use crate::tool::{Output, Progress, ToolMetadata, add_note};
use std::time::Duration;
use tokio::io::AsyncReadExt;

/// After the shell exits, output still in flight gets this long to arrive; a pipe open past it is
/// held by a background process, which does not outlive the call.
const DRAIN: Duration = Duration::from_millis(500);
/// How often a running command's output so far is shown, and how much of its end.
pub(super) const SHOW_EVERY: Duration = Duration::from_millis(500);
const SHOWN_BYTES: usize = 4 * 1024;

/// How a command's run ended.
pub(super) enum Ended {
    /// `lingering`: a background process still held its output open after the shell exited.
    Exited {
        code: i32,
        lingering: bool,
    },
    TimedOut,
    Stopped,
    Failed(String),
}

/// Reads stdout and stderr into one spool in arrival order until both close and the shell exits.
pub(super) async fn collect(child: &mut tokio::process::Child, spool: &mut Spool, progress: &Progress) -> Ended {
    let (Some(mut stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Ended::Failed("the shell's output was not captured".into());
    };
    let (mut stdout_buffer, mut stderr_buffer) = ([0u8; 8192], [0u8; 8192]);
    let (mut stdout_open, mut stderr_open) = (true, true);
    let mut exited: Option<i32> = None;
    let drain = tokio::time::sleep(Duration::MAX);
    tokio::pin!(drain);
    let mut tick = tokio::time::interval(SHOW_EVERY);
    // Missed intervals must not produce a burst of progress updates after a busy loop.
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut shown = 0;

    loop {
        if let (Some(code), false, false) = (exited, stdout_open, stderr_open) {
            return Ended::Exited { code, lingering: false };
        }
        tokio::select! {
            read = stdout.read(&mut stdout_buffer), if stdout_open => stdout_open = take(read, &stdout_buffer, spool),
            read = stderr.read(&mut stderr_buffer), if stderr_open => stderr_open = take(read, &stderr_buffer, spool),
            status = child.wait(), if exited.is_none() => {
                exited = Some(status.map_or(-1, |status| status.code().unwrap_or(-1)));
                drain.as_mut().reset(tokio::time::Instant::now() + DRAIN);
            }
            () = &mut drain, if exited.is_some() => return Ended::Exited { code: exited.unwrap_or(-1), lingering: true },
            _ = tick.tick(), if spool.total() != shown => {
                shown = spool.total();
                progress.show(ToolMetadata { output: Some(spool.recent(SHOWN_BYTES)), ..Default::default() });
            }
        }
    }
}

/// Spools what a read returned; `false` once the pipe is closed or broken.
fn take(read: std::io::Result<usize>, buffer: &[u8], spool: &mut Spool) -> bool {
    match read {
        Ok(0) | Err(_) => false,
        Ok(bytes) => {
            spool.push(&buffer[..bytes]);
            true
        }
    }
}

/// The output without blank lines at either end; the first printed line keeps its indentation.
pub(super) fn without_blank_ends(text: &str) -> &str {
    let text = text.trim_end();
    let first = text
        .find(|character: char| !character.is_whitespace())
        .unwrap_or(text.len());
    let start = text[..first].rfind('\n').map_or(0, |newline| newline + 1);

    &text[start..]
}

pub(super) fn report(title: String, spooled: Spooled, ended: Ended, limit: Option<Duration>) -> Output {
    let mut text = without_blank_ends(&spooled.text).to_string();
    let mut metadata = ToolMetadata {
        shell_timeout_ms: Some(limit.map(|duration| duration.as_millis() as u64)),
        output_bytes: Some(spooled.total),
        ..Default::default()
    };
    if let Some(file) = &spooled.file {
        metadata.output_file = Some(file.to_string_lossy().into_owned());
    }

    let notes: Vec<String> = match ended {
        Ended::Exited { code, lingering } => {
            metadata.exit = Some(code.into());
            let lingered = lingering.then(|| {
                concat!(
                    "Background processes still held the output open when the command finished; ",
                    "they were stopped. Run long-lived processes outside Drift."
                )
                .to_string()
            });
            lingered
                .into_iter()
                .chain((code != 0).then(|| format!("exit code {code}")))
                .collect()
        }
        Ended::TimedOut => {
            metadata.timed_out = Some(true);
            let seconds = limit.map_or(0, |duration| duration.as_secs());
            vec![format!(
                concat!(
                    "The command and its child processes were stopped after {} s. ",
                    "If it needs longer and is not waiting for input, run it again with a larger `timeout` in milliseconds."
                ),
                seconds
            )]
        }
        Ended::Stopped => {
            metadata.stopped = Some(true);
            vec!["The user stopped the command and its child processes.".into()]
        }
        Ended::Failed(error) => vec![error],
    };

    for note in &notes {
        add_note(&mut text, &mut metadata, note);
    }

    Output {
        title,
        output: text,
        metadata,
    }
}
