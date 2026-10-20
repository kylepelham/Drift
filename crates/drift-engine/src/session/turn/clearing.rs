//! Clearing old tool results: a request that misses the prompt cache anyway is the one time its start can
//! change for free, so it sends the bulk of old results as a short note and every later step reads the smaller prompt.

use crate::Engine;
use crate::session::types::{MessageWithParts, Part, PartRow, ToolStatus};

/// The newest results are always sent whole.
const KEEP_RESULTS: usize = 10;
/// A result this short costs about as little as the note that would replace it.
const KEEP_CHARS: usize = 2_000;
/// Clearing writes a new prompt start, so it waits until it removes at least this much.
const CLEAR_AT_LEAST_CHARS: usize = 80_000;
/// Results the conversation's own state rests on: loaded instructions, subagent reports, answers, the plan.
const KEPT_TOOLS: [&str; 6] = ["skill", "task", "task_output", "question", "todowrite", "read_thread"];

impl Engine {
    /// Marks old, large tool results in `transcript` cleared and stores them; the UI keeps showing their output.
    pub(super) fn clear_old_results(&self, transcript: &mut [MessageWithParts]) {
        let mut rows: Vec<&mut PartRow> = transcript
            .iter_mut()
            .flat_map(|message| message.parts.iter_mut())
            .filter(|row| matches!(row.part, Part::ToolCall { .. }))
            .collect();
        let older = rows.len().saturating_sub(KEEP_RESULTS);
        let mut clearable: Vec<&mut PartRow> = rows.drain(..older).filter(|row| clearable(&row.part)).collect();

        let chars: usize = clearable.iter().map(|row| output_len(&row.part)).sum();
        if chars < CLEAR_AT_LEAST_CHARS {
            return;
        }

        for row in &mut clearable {
            if let Part::ToolCall { metadata, .. } = &mut row.part {
                metadata.get_or_insert_with(Box::default).cleared = Some(true);
            }
            let _ = self.store.save_part(row);
        }
    }
}

fn clearable(part: &Part) -> bool {
    let Part::ToolCall {
        name, status, metadata, ..
    } = part
    else {
        return false;
    };

    let finished = matches!(status, ToolStatus::Done | ToolStatus::Error);
    let plain = metadata.as_ref().is_none_or(|metadata| {
        metadata.legacy.is_none() && metadata.engine_command.is_none() && metadata.cleared.is_none()
    });

    finished && plain && !KEPT_TOOLS.contains(&name.as_str()) && output_len(part) > KEEP_CHARS
}

fn output_len(part: &Part) -> usize {
    match part {
        Part::ToolCall { output, .. } => output.as_ref().map_or(0, String::len),
        _ => 0,
    }
}
