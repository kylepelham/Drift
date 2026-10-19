//! A session's frozen system prompt. Its first request freezes every source as a baseline; later
//! requests send the same system prompt, and a source that changed, arrived or went away reaches the
//! model as one `instruction_update` part saved in the transcript, so every later request replays the
//! same bytes and the cached prefix holds.

use std::collections::HashMap;

use sha2::Digest;

use super::Plan;
use crate::Engine;
use crate::event::Event;
use crate::session::prompt::{self, Source};
use crate::session::types::{InstructionChange, MessageStatus, MessageWithParts, Part, Role};
use crate::store::{Instruction, InstructionValue};

/// The row beside the prompt's sources that holds the reasoning level the baseline began with.
pub(super) const EFFORT_KEY: &str = "effort";

fn hash(text: &str) -> String {
    crate::hex_bytes(&sha2::Sha256::digest(text.as_bytes()))
}

/// How a key is named to the user and the model, for a source no longer in the offer.
fn title(key: &str) -> String {
    match key.split_once(':') {
        Some(("mcp", server)) => format!("the {server} MCP server"),
        Some(("file", name)) => name.to_string(),
        Some(("base", _)) => "the base prompt".into(),
        _ if key == "agent" => "the agent's prompt".into(),
        _ if matches!(key, "skills" | "subagents") => format!("the {key} list"),
        _ => format!("the {key}"),
    }
}

/// Leaves out updates made against another baseline: after a compaction or undo starts a new one, an
/// older update still in the history would contradict it.
fn drop_stale(transcript: &mut [MessageWithParts], generation: &str) {
    for message in transcript {
        message.parts.retain(|row| match &row.part {
            Part::InstructionUpdate { generation: made, .. } | Part::EffortUpdate { generation: made, .. } => {
                made == generation
            }
            _ => true,
        });
    }
}

/// The last message the request replays, which carries anything the engine adds before it.
pub(super) fn carrier(transcript: &mut [MessageWithParts]) -> Option<&mut MessageWithParts> {
    let replayed = |message: &MessageWithParts| {
        message.info.role == Role::User || matches!(message.info.status, MessageStatus::Done | MessageStatus::Aborted)
    };

    transcript.iter_mut().rev().find(|message| replayed(message))
}

/// What changed since the model last heard each source.
struct Diff<'a> {
    changed: Vec<&'a Source>,
    gone: Vec<String>,
}

impl Diff<'_> {
    fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.gone.is_empty()
    }
}

/// A source the offer left out keeps its last value when it is still configured: an MCP server
/// reconnecting is not removed, so reconnects and restarts announce nothing.
fn diff<'a>(known: &[Instruction], sources: &'a [Source], keeps: &dyn Fn(&str) -> bool) -> Diff<'a> {
    let current: HashMap<&str, Option<&str>> = known
        .iter()
        .map(|known| (known.key.as_str(), known.current.as_deref()))
        .collect();
    let changed = sources
        .iter()
        .filter(|source| current.get(source.key.as_str()).copied().flatten() != Some(&hash(&source.text)))
        .collect();
    let offered: Vec<&str> = sources.iter().map(|source| source.key.as_str()).collect();
    let gone = known
        .iter()
        .filter(|known| known.key != EFFORT_KEY && known.current.is_some())
        .filter(|known| !offered.contains(&known.key.as_str()) && !keeps(&known.key))
        .map(|known| known.key.clone())
        .collect();

    Diff { changed, gone }
}

impl Engine {
    /// The system prompt this request sends: the frozen baseline, or a new one when the session has none or
    /// switched to another prompt family. Changes since the model last heard each source are saved first, as
    /// an update on the transcript's last replayed message, and added to `transcript` too.
    pub(super) fn instruction_system(&self, plan: &Plan, transcript: &mut [MessageWithParts]) -> String {
        let sources = &plan.offer.sources;
        let session = &plan.session.id;
        let known = self.store.instructions(session).unwrap_or_default();
        let base = sources.first().map(|source| source.key.as_str());
        let same_family = known
            .iter()
            .any(|known| Some(known.key.as_str()) == base && known.baseline.is_some());
        if known.is_empty() || !same_family {
            let values: Vec<InstructionValue> = sources
                .iter()
                .map(|source| InstructionValue {
                    key: &source.key,
                    text: &source.text,
                })
                .collect();
            let generation = self
                .store
                .set_instruction_baseline(session, &values)
                .unwrap_or_default();
            drop_stale(transcript, &generation);
            return prompt::render(sources);
        }
        let generation = known[0].generation.clone();
        drop_stale(transcript, &generation);

        let keeps = |key: &str| {
            key.strip_prefix("mcp:")
                .is_some_and(|server| self.mcp.enabled(server, Some(&plan.workspace)))
        };
        let diff = diff(&known, sources, &keeps);
        if !diff.is_empty() && self.save_instruction_update(session, &generation, &diff, transcript) {
            let values: Vec<InstructionValue> = diff
                .changed
                .iter()
                .map(|source| InstructionValue {
                    key: &source.key,
                    text: &source.text,
                })
                .collect();
            let gone: Vec<&str> = diff.gone.iter().map(String::as_str).collect();
            let _ = self.store.update_instructions(session, &values, &gone);
        }

        self.frozen(&known).unwrap_or_else(|| prompt::render(sources))
    }

    /// Saves the update on the last message the request replays; false when there is none to carry it.
    fn save_instruction_update(
        &self,
        session: &str,
        generation: &str,
        diff: &Diff,
        transcript: &mut [MessageWithParts],
    ) -> bool {
        let Some(carrier) = carrier(transcript) else {
            return false;
        };

        let mut changes: Vec<InstructionChange> = diff
            .changed
            .iter()
            .map(|source| InstructionChange {
                key: source.key.clone(),
                title: source.title.clone(),
                text: Some(source.text.clone()),
            })
            .collect();
        changes.extend(diff.gone.iter().map(|key| InstructionChange {
            key: key.clone(),
            title: title(key),
            text: None,
        }));
        let Ok(row) = self.store.add_part(
            &carrier.info.id,
            session,
            Part::InstructionUpdate {
                generation: generation.to_string(),
                changes,
            },
        ) else {
            return false;
        };
        self.hub.publish(Event::PartCreated { part: row.clone() });
        carrier.parts.push(row);

        true
    }

    /// The baseline's texts in its order; `None` when one cannot be read, so the caller renders afresh.
    fn frozen(&self, known: &[Instruction]) -> Option<String> {
        let texts: Option<Vec<String>> = known
            .iter()
            .filter(|known| known.key != EFFORT_KEY)
            .filter_map(|known| known.baseline.as_deref())
            .map(|hash| self.store.instruction_text(hash).ok().flatten())
            .collect();
        let sources: Vec<Source> = texts?
            .into_iter()
            .map(|text| Source {
                key: String::new(),
                title: String::new(),
                text,
            })
            .collect();

        Some(prompt::render(&sources))
    }
}
