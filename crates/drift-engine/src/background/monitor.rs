//! The task that owns a background process: it reads what the process prints, notices when it ends, and
//! stops it and everything it started when asked.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};

use super::{Exit, Handle, Pipes, ProcessRecord};

/// How often a running process's newest line is shown, at most.
const SHOW_EVERY: Duration = Duration::from_secs(1);

/// Who hears about a process: each change worth showing, then its end, once.
pub(crate) struct Watcher {
    pub changed: Box<dyn Fn(ProcessRecord) + Send + Sync>,
    pub ended: Box<dyn FnOnce(Arc<Handle>) + Send + Sync>,
}

/// Watches the process until its shell has exited and nothing still holds its output open, or it is stopped.
pub(super) async fn watch(handle: Arc<Handle>, mut child: tokio::process::Child, mut pipes: Pipes, watcher: Watcher) {
    (watcher.changed)(handle.record());

    let exit = follow(&handle, &mut child, &mut pipes, &watcher).await;
    if exit == Exit::Stopped {
        handle.kill_tree();
        let _ = child.kill().await;
    }
    // Closing the job ends anything still in it; nothing is left that should be.
    handle.tree.lock().unwrap().take();

    handle.end(exit);
    (watcher.changed)(handle.record());
    (watcher.ended)(handle);
}

/// Reads output until the process is done; a background child still printing after its shell exits is the work itself.
async fn follow(handle: &Handle, child: &mut tokio::process::Child, pipes: &mut Pipes, watcher: &Watcher) -> Exit {
    let (mut stdout_buffer, mut stderr_buffer) = ([0u8; 8192], [0u8; 8192]);
    let mut code = None;
    let mut tick = tokio::time::interval(SHOW_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut shown = handle.record().last_line;

    loop {
        if let (Some(code), None, None) = (code, &pipes.stdout, &pipes.stderr) {
            return Exit::Code(code);
        }

        tokio::select! {
            read = read(&mut pipes.stdout, &mut stdout_buffer) => match read {
                Some(bytes) => handle.push(&stdout_buffer[..bytes]),
                None => pipes.stdout = None,
            },
            read = read(&mut pipes.stderr, &mut stderr_buffer) => match read {
                Some(bytes) => handle.push(&stderr_buffer[..bytes]),
                None => pipes.stderr = None,
            },
            status = child.wait(), if code.is_none() => {
                code = Some(status.map_or(-1, |status| status.code().unwrap_or(-1)));
            }
            () = handle.stop.cancelled() => return Exit::Stopped,
            _ = tick.tick() => {
                let record = handle.record();
                if record.last_line != shown {
                    shown.clone_from(&record.last_line);
                    (watcher.changed)(record);
                }
            }
        }
    }
}

/// The next bytes from a pipe; `None` once it is closed. A pipe already closed never answers.
async fn read(pipe: &mut Option<impl AsyncRead + Unpin>, buffer: &mut [u8]) -> Option<usize> {
    let Some(pipe) = pipe else {
        return std::future::pending().await;
    };

    match pipe.read(buffer).await {
        Ok(0) | Err(_) => None,
        Ok(bytes) => Some(bytes),
    }
}
