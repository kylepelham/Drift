//! Drives orchestrator turns from their final `<orchestrator_status>` block, without a client watching.
//! Working sends another prompt; done, blocked, failure or [`MAX_ROUNDS`] nudges ends the turn.
//! The engine reads the status directly, rather than asking another model to judge the reply.

use serde::Deserialize;

use super::types::{MessageStatus, MessageWithParts, Part, Session};

pub const AGENT: &str = "orchestrator";
/// Nudges allowed per user prompt; each covers a whole dispatch and verify round.
pub const MAX_ROUNDS: usize = 30;

const PROCEED: &str = "Proceed toward the goal. Dispatch the next tasks now and verify results as they land. \
    Do not re-summarize completed work.";
const REMINDER: &str = "Your last reply did not end with a valid <orchestrator_status> block, so your state is unknown. \
    Proceed toward the goal, and end every reply with the mandatory status block.";

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum State {
    Working,
    Done,
    Blocked,
}

#[derive(Deserialize)]
struct Status {
    state: State,
}

/// The state in the reply's final status block; `None` when it is missing, malformed, or not last.
fn state(text: &str) -> Option<State> {
    let start = text.rfind("<orchestrator_status>")?;
    let body = &text[start + "<orchestrator_status>".len()..];
    let end = body.find("</orchestrator_status>")?;
    if !body[end + "</orchestrator_status>".len()..].trim().is_empty() {
        return None;
    }

    serde_json::from_str::<Status>(body[..end].trim())
        .ok()
        .map(|status| status.state)
}

/// The nudge to send after `reply`, the last reply of a top-level orchestrator turn, given the
/// nudges already sent for this prompt; `None` ends the turn.
pub(super) fn next(session: &Session, reply: &MessageWithParts, rounds: usize) -> Option<&'static str> {
    let info = &reply.info;
    let answered = info.status == MessageStatus::Done && info.error.is_none() && info.ending.is_none() && !info.summary;
    if session.agent != AGENT || session.parent_id.is_some() || !answered || rounds >= MAX_ROUNDS {
        return None;
    }

    let text: String = reply
        .parts
        .iter()
        .filter_map(|row| match &row.part {
            Part::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    match state(&text) {
        Some(State::Working) => Some(PROCEED),
        None => Some(REMINDER),
        Some(State::Done | State::Blocked) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_final_well_formed_block_counts() {
        assert_eq!(
            state("did a thing\n<orchestrator_status>{\"state\":\"working\",\"headline\":\"x\"}</orchestrator_status>"),
            Some(State::Working)
        );
        assert_eq!(
            state(
                "<orchestrator_status>{\"state\":\"done\"}</orchestrator_status>\n\
                 <orchestrator_status>{\"state\":\"blocked\"}</orchestrator_status>"
            ),
            Some(State::Blocked),
            "the last block wins"
        );
        assert_eq!(
            state("<orchestrator_status>{\"state\":\"done\"}</orchestrator_status> and then prose"),
            None,
            "the block must come last"
        );
        assert_eq!(
            state("<orchestrator_status>{\"state\":\"thinking\"}</orchestrator_status>"),
            None
        );
        assert_eq!(state("<orchestrator_status>not json</orchestrator_status>"), None);
        assert_eq!(state("no block"), None);
    }
}
