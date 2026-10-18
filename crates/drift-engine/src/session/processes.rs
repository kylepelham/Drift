//! Background processes as a conversation sees them: started from its calls, and their ending handed back
//! to it as a prompt, so the model can carry on without waiting in a call.

use std::sync::Arc;
use std::time::Duration;

use super::turn::{Admission, Prompt, TurnError};
use super::types::{Part, Visibility};
use crate::Engine;
use crate::background::{Exit, HANDOFF_KEY, Handle, PROCESSES_KEY, Start, Watcher, Watching};
use crate::event::Event;

/// How much of the end of a process's output its exit prompt carries.
const EXIT_TAIL_BYTES: usize = 4 * 1024;
/// How often, and how many times, an exit is offered to a conversation busy with another job.
const BUSY_RETRY: Duration = Duration::from_secs(30);
const BUSY_TRIES: usize = 10;

impl Engine {
    /// Whether agents may run commands in the background and see them with `process` (Settings; on by default).
    pub fn background_processes(&self) -> bool {
        self.store.setting(PROCESSES_KEY).ok().flatten().unwrap_or(true)
    }

    /// Whether a command still running at its time limit moves to the background rather than being stopped.
    /// Only while background processes are on, since the model could not see it otherwise.
    pub fn shell_handoff(&self) -> bool {
        self.background_processes() && self.store.setting(HANDOFF_KEY).ok().flatten().unwrap_or(true)
    }

    /// Whether a background subagent or process the session started is still running; its end will wake the session.
    pub(crate) fn background_running(&self, session_id: &str) -> bool {
        let tasks = self.store.tasks_of(session_id).unwrap_or_default();
        let working = tasks
            .iter()
            .any(|task| task.mode == super::tasks::Mode::Background && !task.state.is_terminal());

        working
            || self
                .processes
                .of(session_id)
                .iter()
                .any(|handle| handle.exit().is_none())
    }

    /// Hands a started command to the background; it belongs to `start.session_id` from now on.
    pub(crate) fn start_process(self: &Arc<Self>, start: Start) -> Watching {
        let (_, generation) = self.worker_scope(&start.session_id);
        let log_dir = self.data_dir.join("tool-output").join(&start.session_id);

        let publishing = Arc::downgrade(self);
        let ending = Arc::downgrade(self);
        let watcher = Watcher {
            changed: Box::new(move |process| {
                if let Some(engine) = publishing.upgrade() {
                    engine.hub.publish(Event::ProcessUpdated { process });
                }
            }),
            ended: Box::new(move |handle| {
                if let Some(engine) = ending.upgrade() {
                    tokio::spawn(engine.process_ended(handle));
                }
            }),
        };

        self.processes.start(start, generation, log_dir, watcher)
    }

    /// Tells the conversation a process exited, unless the model already saw it end, someone stopped it,
    /// the user pressed Stop since it started, or it belongs to a subagent.
    async fn process_ended(self: Arc<Self>, handle: Arc<Handle>) {
        // A call waiting on it or reading it reports the end in its own result; only then is it known whether it did.
        handle.unwatched().await;
        if handle.exit() == Some(Exit::Stopped) || !self.wakes_for(&handle) || !handle.claim_report() {
            return;
        }
        let Some(scope) = self.scope_at(&handle.session_id, handle.generation) else {
            return;
        };

        for _ in 0..BUSY_TRIES {
            let prompt = Prompt {
                parts: vec![exit_part(&handle)],
                model: None,
                variant: None,
                agent: None,
                submission_id: Some(format!("process:{}", handle.id)),
            };
            let how = Admission {
                parent: Some(&scope),
                ..Admission::default()
            };

            match self.admit(&handle.session_id, prompt, how).await {
                Err(TurnError::Busy) => tokio::time::sleep(BUSY_RETRY).await,
                _ => return,
            }
        }
    }

    /// A subagent's processes end with its run, so only a conversation the user sees is woken.
    fn wakes_for(&self, handle: &Handle) -> bool {
        let session = self.store.session(&handle.session_id).ok().flatten();

        session.is_some_and(|session| session.visibility != Visibility::Hidden && session.archived_at.is_none())
    }
}

fn exit_part(handle: &Handle) -> Part {
    let record = handle.record();

    Part::ProcessExit {
        process_id: record.id,
        command: record.command,
        description: record.description,
        exit_code: record.exit_code,
        output: handle.tail(EXIT_TAIL_BYTES),
        log: record.log,
    }
}

/// How the model reads a process's ending.
pub(crate) fn exit_text(part: &Part) -> String {
    let Part::ProcessExit {
        process_id,
        command,
        description,
        exit_code,
        output,
        log,
    } = part
    else {
        return String::new();
    };
    let ended = exit_code.map_or_else(|| "was stopped".to_string(), |code| format!("exited with code {code}"));

    format!(
        "<process-exit id=\"{process_id}\">\n\
         The background process `{command}` ({description}) {ended}. The end of its output:\n\
         {}\n\
         Everything it printed is in {log}.\n\
         </process-exit>",
        output.trim_end()
    )
}
