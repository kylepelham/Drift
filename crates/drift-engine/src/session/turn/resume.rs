//! Turns that were under way when Drift stopped start again when it starts, unless the user turned that
//! off. The model is told Drift restarted mid-turn and that an interrupted call may or may not have
//! taken effect, so it checks before repeating one. A prompt is resumed at most twice; workers keep
//! their own recovery.

use std::sync::Arc;

use super::Prompt;
use crate::Engine;
use crate::session::types::{Part, Visibility};

pub const RESUME_KEY: &str = "resumeTurns";
const MAX_RESUMES: u32 = 2;
const RESTART_NOTE: &str = "Drift restarted while this turn was running. Calls it interrupted may or may not \
    have taken effect: check what they changed before repeating any of them, then carry on with the task.";

impl Engine {
    pub fn resumes_turns(&self) -> bool {
        self.store.setting(RESUME_KEY).ok().flatten().unwrap_or(true)
    }

    /// Starts again each turn Drift stopped in the middle of, once on startup.
    pub async fn resume_turns(self: &Arc<Self>) {
        for claim in self.store.claimed_turns().unwrap_or_default() {
            let id = claim.session_id;
            let session = self.store.session(&id).ok().flatten();
            let resumable = session
                .is_some_and(|session| session.visibility != Visibility::Hidden && session.archived_at.is_none());
            if !self.resumes_turns() || !resumable || claim.resumes >= MAX_RESUMES {
                let _ = self.store.release_turn(&id);
                continue;
            }

            let _ = self.store.count_resume(&id);
            let prompt = Prompt {
                parts: vec![Part::Continuation {
                    text: RESTART_NOTE.into(),
                }],
                model: None,
                variant: None,
                agent: None,
                submission_id: None,
            };
            if let Err(error) = self.submit(&id, prompt).await {
                eprintln!("drift: could not resume {id}'s turn after the restart: {error}");
                let _ = self.store.release_turn(&id);
            }
        }
    }
}
