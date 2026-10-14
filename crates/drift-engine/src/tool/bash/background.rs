//! Commands that go on after their call: started with `background: true`, or still running at their limit.

use std::time::Duration;

use super::Started;
use super::output::without_blank_ends;
use crate::background::{Exit, Handle, Start, Watching};
use crate::session::types::Visibility;
use crate::tool::spool::Spooled;
use crate::tool::{Context, Output, ToolMetadata, add_note};

/// How long a background start waits, so a command that fails at once says so in its own result.
const SETTLE: Duration = Duration::from_secs(1);

/// Hands the command to the background and reports how its first second went.
pub(super) async fn start(context: &Context, started: Started, seen: Vec<u8>) -> Output {
    let title = started.title.clone();
    // Watched until the result is written, so an exit within the first second is said here and not also later.
    let handle = hand_over(context, started, seen);
    handle.wait(None, SETTLE, &context.abort).await;

    let unread = handle.unread();
    let mut output = Output {
        title,
        output: without_blank_ends(&unread.text).to_string(),
        metadata: metadata(&handle),
    };

    match handle.exit() {
        Some(Exit::Code(code)) if handle.claim_report() => {
            output.metadata.exit = Some(code.into());
            note(&mut output, &format!("It exited at once, with code {code}."));
        }
        _ => note(&mut output, &running_note(context, &handle)),
    }

    output
}

/// The command reached its limit: what it printed so far, and where it goes on.
pub(super) async fn moved(context: &Context, started: Started, spooled: Spooled, limit: Option<Duration>) -> Output {
    let title = started.title.clone();
    let mut output = Output {
        title,
        output: without_blank_ends(&spooled.text).to_string(),
        metadata: ToolMetadata::default(),
    };
    let handle = hand_over(context, started, spooled.text.into_bytes());
    output.metadata = ToolMetadata {
        shell_timeout_ms: Some(limit.map(|limit| limit.as_millis() as u64)),
        output_bytes: Some(spooled.total),
        output_file: spooled.file.map(|file| file.to_string_lossy().into_owned()),
        ..metadata(&handle)
    };

    let seconds = limit.map_or(0, |limit| limit.as_secs());
    note(
        &mut output,
        &format!(
            "It was still running after {seconds} s, so it moved to the background. {}",
            running_note(context, &handle)
        ),
    );

    output
}

fn hand_over(context: &Context, started: Started, seen: Vec<u8>) -> Watching {
    let Started {
        command,
        title,
        running,
    } = started;

    context.engine.start_process(Start {
        session_id: context.session_id.clone(),
        command,
        description: title,
        child: running.child,
        tree: running.tree,
        pipes: running.pipes,
        seen,
        started_at: running.started_at,
    })
}

fn metadata(handle: &Handle) -> ToolMetadata {
    ToolMetadata {
        process_id: Some(handle.id.clone()),
        output_file: Some(handle.record().log),
        ..ToolMetadata::default()
    }
}

fn running_note(context: &Context, handle: &Handle) -> String {
    let url = handle
        .record()
        .url
        .map(|url| format!(" It printed {url}."))
        .unwrap_or_default();

    format!(
        "It runs in the background as {}.{url} {} Use `process` to read its output, wait for a line, type into it \
         or stop it.",
        handle.id,
        after_turn(context)
    )
}

/// What happens once the call returns: a conversation the user sees is told when the process ends, while a
/// subagent's processes stop with its run and nothing wakes it.
fn after_turn(context: &Context) -> &'static str {
    let subagent = context
        .engine
        .store
        .session(&context.session_id)
        .ok()
        .flatten()
        .is_some_and(|session| session.visibility == Visibility::Hidden);

    if subagent {
        "You are a subagent: you are not told when it ends, and it is stopped when your run ends, so wait for it \
         with `process` before you report."
    } else {
        "This conversation is told when it ends, so keep working or end your turn rather than waiting."
    }
}

fn note(output: &mut Output, text: &str) {
    add_note(&mut output.output, &mut output.metadata, text);
}
