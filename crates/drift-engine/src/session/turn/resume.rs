//! Turns that were under way when Drift stopped start again when it starts, unless the user turned that
//! off. The model is told Drift restarted mid-turn and that an interrupted call may or may not have
//! taken effect, so it checks before repeating one. A prompt is resumed at most twice. Background workers
//! cut off the same way are queued again at open (`Store::interrupt_unfinished_tasks`) and started here,
//! before the turns, continuing their own conversation; their result reaches the parent as usual.

use std::sync::Arc;

use super::Prompt;
use crate::Engine;
use crate::session::tasks::TaskState;
use crate::session::types::{Part, Visibility};

pub const RESUME_KEY: &str = "resumeTurns";
const MAX_RESUMES: u32 = 2;
const RESTART_NOTE: &str = "Drift restarted while this turn was running. Calls it interrupted may or may not \
    have taken effect: check what they changed before repeating any of them, then carry on with the task.";
const WORKERS_NOTE: &str = " Your background subagents that were running were restarted too and will report \
    when they finish, as before: do not launch them again.";
const WORKER_NOTE: &str = "Drift restarted while you were working on this task. Calls it interrupted may or \
    may not have taken effect: check what they changed before repeating any of them, then finish the task.";
const NOT_RESTARTED: &str = "Drift stopped while the subagent ran; it was not restarted.";

fn continuation(text: String) -> Prompt {
    Prompt {
        parts: vec![Part::Continuation { text }],
        model: None,
        variant: None,
        agent: None,
        submission_id: None,
        worktree: false,
    }
}

impl Engine {
    pub fn resumes_turns(&self) -> bool {
        self.store.setting(RESUME_KEY).ok().flatten().unwrap_or(true)
    }

    /// Starts the background workers a restart cut off, once on startup. One whose owner was stopped since it
    /// launched, or whose conversation is gone, ends interrupted and wakes no one, as it would have before.
    pub async fn restart_workers(self: &Arc<Self>) {
        let restarting = std::mem::take(&mut *self.turns.restarting.lock().unwrap());
        for id in restarting {
            let Some(task) = self.store.task(&id).ok().flatten() else {
                continue;
            };
            let Some(token) = self.scope_at(&task.parent_session_id, task.generation) else {
                self.drop_restart(&task.id);
                continue;
            };

            self.turns
                .restarted_owners
                .lock()
                .unwrap()
                .insert(task.parent_session_id.clone());
            self.publish_task(&task.id);
            let prompt = continuation(WORKER_NOTE.into());
            if self.admit_worker(&task, prompt, token.child_token()).await.is_err() {
                // It could not start (its model is gone, say): the parent hears why, as from a launch.
                self.deliver(&task.id).await;
            }
        }
    }

    fn drop_restart(&self, task_id: &str) {
        self.end_task(task_id, TaskState::Interrupted, NOT_RESTARTED);
        let _ = self.store.mark_task_delivered(task_id);
        self.publish_task(task_id);
    }

    /// Starts again each turn Drift stopped in the middle of, once on startup.
    pub async fn resume_turns(self: &Arc<Self>) {
        let cut_off = std::mem::take(&mut *self.turns.cut_off.lock().unwrap());
        let restarted_owners = std::mem::take(&mut *self.turns.restarted_owners.lock().unwrap());
        for claim in cut_off {
            let id = claim.session_id;
            if self.turns.is_running(&id) {
                continue;
            }
            let session = self.store.session(&id).ok().flatten();
            let resumable = session
                .is_some_and(|session| session.visibility != Visibility::Hidden && session.archived_at.is_none());
            if !self.resumes_turns() || !resumable || claim.resumes >= MAX_RESUMES {
                let _ = self.store.release_turn(&id);
                continue;
            }

            let _ = self.store.count_resume(&id);
            let workers = if restarted_owners.contains(&id) {
                WORKERS_NOTE
            } else {
                ""
            };
            if let Err(error) = self.submit(&id, continuation(format!("{RESTART_NOTE}{workers}"))).await {
                eprintln!("drift: could not resume {id}'s turn after the restart: {error}");
                let _ = self.store.release_turn(&id);
            }
        }
    }
}
