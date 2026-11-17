//! Retrying a cyber refusal once, off by default: the refused reply goes, a `.` is sent as a nudge, and the turn goes
//! on. Refused again, the `.` goes too and the second refusal ends the conversation as the first would have.

use super::*;
use crate::session::types::Refusal;

pub const RETRY_REFUSALS_KEY: &str = "retryRefusals";

impl Engine {
    pub fn retries_refusals(&self) -> bool {
        self.store.setting(RETRY_REFUSALS_KEY).ok().flatten().unwrap_or(false)
    }

    /// Sends the `.` that retries a refusal, when the setting is on, no retry is out yet, and it is a cyber refusal or
    /// one naming no policy area, the kind a second look may pass; false leaves the refusal to stand.
    pub(super) fn retry_refusal(&self, plan: &mut Plan, refusal: Option<&Refusal>, abort: &CancellationToken) -> bool {
        if plan.refusal_retry.is_some() || !self.retries_refusals() || !accounts::accounts_differ(refusal) {
            return false;
        }
        let prompt = FencedPrompt {
            pick: Pick {
                model: &plan.model_ref,
                variant: None,
                agent: None,
                sticky: true,
            },
            parts: vec![Part::Nudge { text: ".".into() }],
            submission: None,
            abort: Some(abort),
            delivery: None,
        };
        let Ok(admitted) = self.admit_fenced(&plan.session.id, prompt) else {
            return false;
        };
        plan.refusal_retry = Some(admitted.message.id.clone());
        self.announce(&plan.session.id, admitted);

        true
    }

    /// Once the retry is answered: refused again, the `.` goes; answered, it stays in the history.
    pub(super) fn settle_refusal_retry(&self, plan: &mut Plan, refused: bool) {
        let Some(retry) = plan.refusal_retry.take() else {
            return;
        };
        if refused && self.store.discard_prompt(&retry).unwrap_or(false) {
            self.hub.publish(Event::MessageRemoved {
                session_id: plan.session.id.clone(),
                message_id: retry,
            });
        }
    }
}
