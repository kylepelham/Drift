//! Background processes: shell commands that outlive the call that started them. Each belongs to the
//! conversation that started it, keeps its output, and ends when it exits, is stopped, or its
//! conversation is archived; Drift's quitting ends it with the job that holds it.

mod buffer;
mod monitor;
mod plain;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;

use crate::platform::process::Tree;
pub use buffer::Unread;
use buffer::{Buffer, url_in};
pub(crate) use monitor::Watcher;

/// Ended processes a conversation keeps listed; older ones are forgotten.
const ENDED_KEPT: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProcessState {
    Running,
    Exited,
    Stopped,
}

/// A background process as the dock and the API show it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRecord {
    pub id: String,
    pub session_id: String,
    pub command: String,
    pub description: String,
    pub state: ProcessState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub started_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_line: Option<String>,
    /// The first web address it printed, such as a dev server's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Everything it printed, up to 64 MB.
    pub log: String,
}

/// The output pipes of a spawned shell, kept apart from the child so a read can be dropped and resumed.
pub struct Pipes {
    pub stdout: Option<tokio::process::ChildStdout>,
    pub stderr: Option<tokio::process::ChildStderr>,
}

impl Pipes {
    pub fn take(child: &mut tokio::process::Child) -> Self {
        Self {
            stdout: child.stdout.take(),
            stderr: child.stderr.take(),
        }
    }
}

/// A command handed to the background: the running shell and what the model was already shown.
pub struct Start {
    pub session_id: String,
    pub command: String,
    pub description: String,
    pub child: tokio::process::Child,
    pub tree: Option<Tree>,
    pub pipes: Pipes,
    /// Output the call already returned, so the next read starts after it.
    pub seen: Vec<u8>,
    /// When the shell started, in ms; a command moved here at its limit started before the move.
    pub started_at: i64,
}

/// How a process ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Exit {
    Code(i32),
    /// The user or the model stopped it, or its conversation was archived.
    Stopped,
}

/// What a wait for a process found.
#[derive(Debug, PartialEq)]
pub enum Waited {
    Matched,
    Ended,
    TimedOut,
    Aborted,
}

/// One background process.
pub struct Handle {
    pub id: String,
    pub session_id: String,
    pub command: String,
    pub description: String,
    pub started_at: i64,
    pub generation: i64,
    state: Mutex<State>,
    changed: tokio::sync::watch::Sender<()>,
    stdin: tokio::sync::Mutex<Option<tokio::process::ChildStdin>>,
    stop: CancellationToken,
    /// The job or process group holding everything it started, until it has ended.
    tree: Mutex<Option<Tree>>,
    #[cfg(unix)]
    pid: Option<u32>,
    /// Its ending reached the model, from a call or as a prompt; it is said once.
    reported: AtomicBool,
}

struct State {
    buffer: Buffer,
    exit: Option<Exit>,
    ended_at: Option<i64>,
    url: Option<String>,
}

#[derive(Default)]
pub struct Processes {
    all: Mutex<HashMap<String, Arc<Handle>>>,
}

impl Processes {
    /// Registers a started process and watches it until it ends; `ended` is told once it has.
    /// `generation` is the owner's Stop count now; a Stop since keeps the exit from waking it.
    pub(crate) fn start(&self, mut start: Start, generation: i64, log_dir: PathBuf, watcher: Watcher) -> Arc<Handle> {
        let id = crate::id::new("proc");
        let mut buffer = Buffer::new(log_dir.join(format!("{id}.log")));
        let seen = buffer.seen(&start.seen);

        let handle = Arc::new(Handle {
            id: id.clone(),
            session_id: start.session_id,
            command: start.command,
            description: start.description,
            started_at: start.started_at,
            generation,
            state: Mutex::new(State {
                buffer,
                exit: None,
                ended_at: None,
                url: None,
            }),
            changed: tokio::sync::watch::channel(()).0,
            stdin: tokio::sync::Mutex::new(start.child.stdin.take()),
            stop: CancellationToken::new(),
            tree: Mutex::new(start.tree),
            #[cfg(unix)]
            pid: start.child.id(),
            reported: AtomicBool::new(false),
        });
        handle.note_url(&seen);

        self.forget_old(&handle.session_id);
        self.all.lock().unwrap().insert(id, handle.clone());
        tokio::spawn(monitor::watch(handle.clone(), start.child, start.pipes, watcher));

        handle
    }

    pub fn get(&self, id: &str) -> Option<Arc<Handle>> {
        self.all.lock().unwrap().get(id).cloned()
    }

    /// A conversation's processes, oldest first.
    pub fn of(&self, session_id: &str) -> Vec<Arc<Handle>> {
        let mut handles: Vec<_> = self
            .all
            .lock()
            .unwrap()
            .values()
            .filter(|handle| handle.session_id == session_id)
            .cloned()
            .collect();
        handles.sort_by_key(|handle| handle.started_at);

        handles
    }

    /// Stops every process a conversation started, as when it is archived or its subagent run ends.
    pub fn stop_session(&self, session_id: &str) {
        for handle in self.of(session_id) {
            handle.stop();
        }
    }

    /// Ends every process now, without waiting for their watchers, as Drift shuts down.
    pub fn stop_all(&self) {
        for handle in self.all.lock().unwrap().values() {
            handle.stop();
            handle.kill_tree();
        }
    }

    /// Makes room for a new process by forgetting the conversation's oldest ended ones.
    fn forget_old(&self, session_id: &str) {
        let ended: Vec<_> = self
            .of(session_id)
            .into_iter()
            .filter(|handle| handle.exit().is_some())
            .collect();
        let excess = ended.len().saturating_sub(ENDED_KEPT - 1);

        let mut all = self.all.lock().unwrap();
        for handle in &ended[..excess] {
            all.remove(&handle.id);
        }
    }
}

impl Handle {
    pub fn record(&self) -> ProcessRecord {
        let state = self.state.lock().unwrap();
        let (state_name, exit_code) = match state.exit {
            None => (ProcessState::Running, None),
            Some(Exit::Code(code)) => (ProcessState::Exited, Some(code)),
            Some(Exit::Stopped) => (ProcessState::Stopped, None),
        };

        ProcessRecord {
            id: self.id.clone(),
            session_id: self.session_id.clone(),
            command: self.command.clone(),
            description: self.description.clone(),
            state: state_name,
            exit_code,
            started_at: self.started_at,
            ended_at: state.ended_at,
            last_line: state.buffer.last_line(),
            url: state.url.clone(),
            log: state.buffer.path.to_string_lossy().into_owned(),
        }
    }

    pub fn exit(&self) -> Option<Exit> {
        self.state.lock().unwrap().exit
    }

    /// Asks the process and everything it started to end; the watcher records it.
    pub fn stop(&self) {
        self.stop.cancel();
    }

    /// Ctrl+C for the process group. Windows has no console to send it through, so there it stops instead.
    pub fn interrupt(&self) -> bool {
        #[cfg(unix)]
        if let Some(pid) = self.pid.and_then(|pid| i32::try_from(pid).ok()) {
            // SAFETY: the negative pid addresses the process group the shell leads; kill takes no pointers.
            unsafe {
                libc::kill(-pid, libc::SIGINT);
            }
            return true;
        }

        self.stop();
        false
    }

    /// Types `text` into the process; `false` when it takes no input or has ended.
    pub async fn input(&self, text: &str) -> bool {
        use tokio::io::AsyncWriteExt;

        let mut stdin = self.stdin.lock().await;
        let Some(pipe) = stdin.as_mut() else {
            return false;
        };
        let written = pipe.write_all(text.as_bytes()).await.is_ok() && pipe.flush().await.is_ok();
        if !written {
            *stdin = None;
        }

        written
    }

    /// Output since the model last read, marked read.
    pub fn unread(&self) -> Unread {
        self.state.lock().unwrap().buffer.unread()
    }

    pub fn search(&self, matches: &dyn Fn(&str) -> bool, limit: usize) -> Vec<String> {
        self.state.lock().unwrap().buffer.search(matches, limit)
    }

    pub fn tail(&self, bytes: usize) -> String {
        self.state.lock().unwrap().buffer.tail(bytes)
    }

    /// Takes the right to tell the model how it ended; `true` once, for whoever asks first.
    pub fn claim_report(&self) -> bool {
        !self.reported.swap(true, Ordering::SeqCst)
    }

    /// Waits until unread output has a line `until` accepts, the process ends, `limit` passes or `abort` fires.
    pub async fn wait(
        &self,
        until: Option<&(dyn Fn(&str) -> bool + Sync)>,
        limit: Duration,
        abort: &CancellationToken,
    ) -> Waited {
        let mut changes = self.changed.subscribe();
        let deadline = tokio::time::Instant::now() + limit;

        loop {
            {
                let state = self.state.lock().unwrap();
                if until.is_some_and(|until| state.buffer.unread_matches(until)) {
                    return Waited::Matched;
                }
                if state.exit.is_some() {
                    return Waited::Ended;
                }
            }

            tokio::select! {
                _ = changes.changed() => {}
                () = tokio::time::sleep_until(deadline) => return Waited::TimedOut,
                () = abort.cancelled() => return Waited::Aborted,
            }
        }
    }

    fn push(&self, bytes: &[u8]) {
        let text = self.state.lock().unwrap().buffer.push(bytes);
        self.note_url(&text);
        self.changed.send_replace(());
    }

    fn note_url(&self, text: &str) {
        let mut state = self.state.lock().unwrap();
        if state.url.is_none() {
            state.url = text.lines().find_map(url_in);
        }
    }

    fn kill_tree(&self) {
        if let Some(tree) = self.tree.lock().unwrap().as_ref() {
            tree.kill();
        }
    }

    fn end(&self, exit: Exit) {
        let mut state = self.state.lock().unwrap();
        state.exit.get_or_insert(exit);
        state.ended_at.get_or_insert_with(crate::id::now_ms);
        drop(state);

        self.changed.send_replace(());
    }
}

#[cfg(test)]
mod tests;
