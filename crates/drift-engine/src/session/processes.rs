//! Background processes as a conversation sees them: started from its calls, and their ending handed back
//! to it as a prompt, so the model can carry on without waiting in a call. One that Drift's stopping ended
//! is told the same way when Drift next starts.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::turn::{Admission, Prompt, TurnError};
use super::types::{Part, Visibility};
use crate::Engine;
use crate::background::{Exit, HANDOFF_KEY, Handle, PROCESSES_KEY, Start, Watcher, Watching};
use crate::event::Event;
use crate::store::RunningProcess;

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

    /// Whether a background subagent or process the session started is still running and its end will wake the session.
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
                .any(|handle| self.will_wake(handle))
    }

    /// A running process whose end will still be told: not stopped, not yet reported, started since the last Stop,
    /// in a conversation the user sees. Only those are worth waiting for.
    fn will_wake(&self, handle: &Handle) -> bool {
        handle.exit().is_none()
            && !handle.reported()
            && self.wakes_for(handle)
            && self.scope_at(&handle.session_id, handle.generation).is_some()
    }

    /// Hands a started command to the background; it belongs to `start.session_id` from now on.
    pub(crate) fn start_process(self: &Arc<Self>, start: Start) -> Watching {
        let (_, generation) = self.worker_scope(&start.session_id);
        let log_dir = self.data_dir.join("tool-output").join(&start.session_id);

        let noting = Arc::downgrade(self);
        let publishing = Arc::downgrade(self);
        let ending = Arc::downgrade(self);
        let watcher = Watcher {
            started: Some(Box::new(move |handle| {
                if let Some(engine) = noting.upgrade() {
                    let _ = engine.store.note_process(&running(handle));
                }
            })),
            changed: Box::new(move |process| {
                if let Some(engine) = publishing.upgrade() {
                    engine.hub.publish(Event::ProcessUpdated { process });
                }
            }),
            ended: Box::new(move |handle| {
                if let Some(engine) = ending.upgrade() {
                    // One ended by Drift quitting stays noted, for the next start to tell.
                    if !engine.processes.closing() {
                        let _ = engine.store.forget_process(&handle.id);
                    }
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

        let submission = format!("process:{}", handle.id);
        self.tell(&handle.session_id, vec![exit_part(&handle)], submission, &scope)
            .await;
    }

    /// Tells each conversation, once on startup, which of its background processes ended when Drift last
    /// stopped, unless the user pressed Stop since they started.
    pub fn report_cut_off_processes(self: &Arc<Self>) {
        let cut_off = std::mem::take(&mut *self.turns.cut_off_processes.lock().unwrap());
        let mut by_session: BTreeMap<String, Vec<RunningProcess>> = BTreeMap::new();
        for process in cut_off {
            by_session.entry(process.session_id.clone()).or_default().push(process);
        }

        for (session_id, processes) in by_session {
            let waited_on: Vec<_> = processes
                .into_iter()
                .filter_map(|process| Some((self.scope_at(&session_id, process.generation)?, process)))
                .collect();
            let Some((scope, first)) = waited_on.first().filter(|_| self.wakes(&session_id)) else {
                continue;
            };

            let submission = format!("process-restart:{}", first.id);
            let (engine, scope) = (self.clone(), scope.clone());
            let parts = waited_on.iter().map(|(_, process)| cut_off_part(process)).collect();
            tokio::spawn(async move { engine.tell(&session_id, parts, submission, &scope).await });
        }
    }

    /// Hands `parts` to the conversation as its next prompt, steering into a running turn; a conversation
    /// busy with another job is offered them every 30 s, ten times.
    async fn tell(self: &Arc<Self>, session_id: &str, parts: Vec<Part>, submission: String, scope: &CancellationToken) {
        for _ in 0..BUSY_TRIES {
            let prompt = Prompt {
                parts: parts.clone(),
                model: None,
                variant: None,
                agent: None,
                submission_id: Some(submission.clone()),
                worktree: false,
            };
            let how = Admission {
                parent: Some(scope),
                ..Admission::default()
            };

            match self.admit(session_id, prompt, how).await {
                Err(TurnError::Busy) => tokio::time::sleep(BUSY_RETRY).await,
                _ => return,
            }
        }
    }

    fn wakes_for(&self, handle: &Handle) -> bool {
        self.wakes(&handle.session_id)
    }

    /// A subagent's processes end with its run, so only a conversation the user sees is woken.
    fn wakes(&self, session_id: &str) -> bool {
        let session = self.store.session(session_id).ok().flatten();

        session.is_some_and(|session| session.visibility != Visibility::Hidden && session.archived_at.is_none())
    }
}

fn running(handle: &Handle) -> RunningProcess {
    RunningProcess {
        id: handle.id.clone(),
        session_id: handle.session_id.clone(),
        command: handle.command.clone(),
        description: handle.description.clone(),
        log: handle.record().log,
        generation: handle.generation,
    }
}

fn cut_off_part(process: &RunningProcess) -> Part {
    Part::ProcessExit {
        process_id: process.id.clone(),
        command: process.command.clone(),
        description: process.description.clone(),
        exit_code: None,
        output: log_tail(&process.log, EXIT_TAIL_BYTES),
        log: process.log.clone(),
        restarted: true,
    }
}

/// The end of a log on disk; empty when it is gone.
fn log_tail(path: &str, bytes: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let length = file.metadata().map_or(0, |metadata| metadata.len());
    let mut tail = Vec::new();
    let _ = file
        .seek(SeekFrom::Start(length.saturating_sub(bytes as u64)))
        .and_then(|_| file.read_to_end(&mut tail));

    String::from_utf8_lossy(&tail).into_owned()
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
        restarted: false,
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
        restarted,
    } = part
    else {
        return String::new();
    };
    let ended = match exit_code {
        _ if *restarted => "ended when Drift restarted, before it finished; anything it held (a lock, a \
            half-written file) may need cleaning up before it runs again"
            .to_string(),
        Some(code) => format!("exited with code {code}"),
        None => "was stopped".to_string(),
    };

    format!(
        "<process-exit id=\"{process_id}\">\n\
         The background process `{command}` ({description}) {ended}. The end of its output:\n\
         {}\n\
         Everything it printed is in {log}.\n\
         </process-exit>",
        output.trim_end()
    )
}
