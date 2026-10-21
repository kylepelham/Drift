//! Reasoning level changes that keep the prompt cache. On a model that takes a per-message effort
//! (`Model::effort_updates`) the request's top-level effort stays at the level its instruction
//! baseline began with, and each switch since is saved as an `effort_update` part and sent where it
//! happened. Any other model gets the current level at the top, as before, and no marks.

use super::Plan;
use super::instructions::{EFFORT_KEY, carrier};
use crate::Engine;
use crate::event::Event;
use crate::llm::catalog::Reasoning;
use crate::llm::{Block, ChatMessage, EffortMark};
use crate::session::types::{MessageWithParts, Part};

fn strip(transcript: &mut [MessageWithParts]) {
    for message in transcript {
        message
            .parts
            .retain(|row| !matches!(row.part, Part::EffortUpdate { .. }));
    }
}

impl Engine {
    /// The effort the request sends at its top level, saving a switch to `reasoning` as a mark first.
    pub(super) fn top_level_effort(
        &self,
        plan: &Plan,
        reasoning: Option<Reasoning>,
        transcript: &mut [MessageWithParts],
    ) -> Option<Reasoning> {
        let Some(Reasoning::Effort { level }) = reasoning.clone().filter(|_| plan.model.effort_updates) else {
            strip(transcript);
            return reasoning;
        };
        let session = &plan.session.id;
        let known = self.store.instructions(session).unwrap_or_default();
        let Some(generation) = known.first().map(|known| known.generation.clone()) else {
            strip(transcript);
            return reasoning;
        };
        let begun = known
            .iter()
            .find(|known| known.key == EFFORT_KEY)
            .and_then(|known| known.baseline.as_deref())
            .and_then(|hash| self.store.instruction_text(hash).ok().flatten());
        let Some(begun) = begun else {
            let _ = self.store.add_to_instruction_baseline(session, EFFORT_KEY, &level);
            strip(transcript);
            return reasoning;
        };

        let marked = transcript
            .iter()
            .flat_map(|message| &message.parts)
            .filter_map(|row| match &row.part {
                Part::EffortUpdate { effort, .. } => Some(effort.as_str()),
                _ => None,
            })
            .next_back();
        if marked.unwrap_or(&begun) != level
            && let Some(carrier) = carrier(transcript)
        {
            let part = Part::EffortUpdate {
                generation,
                effort: level.clone(),
            };
            if let Ok(row) = self.store.add_part(&carrier.info.id, session, part) {
                self.hub.publish(Event::PartCreated { part: row.clone() });
                carrier.parts.push(row);
            }
        }

        Some(Reasoning::Effort { level: begun })
    }
}

/// Takes the effort blocks conversion placed out of the messages, each as a mark after its message.
pub(super) fn lift_marks(messages: &mut Vec<ChatMessage>) -> Vec<EffortMark> {
    let mut marks = Vec::new();
    let mut kept: Vec<ChatMessage> = Vec::with_capacity(messages.len());
    for mut message in messages.drain(..) {
        let efforts: Vec<String> = message
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Effort(effort) => Some(effort.clone()),
                _ => None,
            })
            .collect();
        message.blocks.retain(|block| !matches!(block, Block::Effort(_)));
        if !message.blocks.is_empty() {
            kept.push(message);
        }
        // A message left empty is dropped; its mark follows whatever came before it.
        if let Some(after) = kept.len().checked_sub(1) {
            marks.extend(efforts.into_iter().map(|effort| EffortMark { after, effort }));
        }
    }
    *messages = kept;

    marks
}
