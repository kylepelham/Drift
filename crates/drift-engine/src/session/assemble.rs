//! Turns a chunk stream into stored parts, publishing deltas as they arrive.

use serde_json::Value;

use crate::event::{Event, Hub};
use crate::llm::{Chunk, StopReason};
use crate::session::types::{Message, Part, PartRow, ToolStatus, Usage};
use crate::store::Store;

pub struct Assembler<'a> {
    store: &'a Store,
    hub: &'a Hub,
    message: &'a Message,
    open: Option<Open>,
    pub usage: Usage,
    pub stop: Option<StopReason>,
    pub calls: Vec<PartRow>,
}

/// The block currently streaming; tool input arrives as JSON text and is parsed at stop.
struct Open {
    row: PartRow,
    tool_json: String,
}

impl<'a> Assembler<'a> {
    pub fn new(store: &'a Store, hub: &'a Hub, message: &'a Message) -> Self {
        Self { store, hub, message, open: None, usage: Usage::default(), stop: None, calls: Vec::new() }
    }

    pub fn apply(&mut self, chunk: Chunk) -> rusqlite::Result<()> {
        match chunk {
            Chunk::TextStart => self.start(Part::Text { text: String::new() }),
            Chunk::ReasoningStart => self.start(Part::Reasoning { text: String::new(), signature: None, redacted: None }),
            Chunk::ReasoningRedacted(data) => {
                self.start(Part::Reasoning { text: String::new(), signature: None, redacted: Some(data) })?;
                self.stop_block()
            }
            Chunk::ToolUseStart { id, name } => self.start(Part::ToolCall {
                call_id: id,
                name,
                input: Value::Null,
                status: ToolStatus::Pending,
                title: None,
                output: None,
                metadata: None,
                started_at: None,
                finished_at: None,
            }),
            Chunk::TextDelta(delta) | Chunk::ReasoningDelta(delta) => self.delta(&delta),
            Chunk::ReasoningSignature(signature) => self.edit(|part| {
                if let Part::Reasoning { signature: slot, .. } = part {
                    *slot = Some(signature);
                }
            }),
            Chunk::ToolInputDelta(json) => {
                if let Some(open) = &mut self.open {
                    open.tool_json.push_str(&json);
                }
                Ok(())
            }
            Chunk::BlockStop => self.stop_block(),
            Chunk::Usage(usage) => {
                self.usage.add(usage);
                Ok(())
            }
            Chunk::Stop(reason) => {
                self.stop = Some(reason);
                Ok(())
            }
        }
    }

    fn start(&mut self, part: Part) -> rusqlite::Result<()> {
        self.stop_block()?;
        let row = self.store.add_part(&self.message.id, &self.message.session_id, part)?;
        self.hub.publish(Event::PartCreated { part: row.clone() });
        self.open = Some(Open { row, tool_json: String::new() });
        Ok(())
    }

    fn delta(&mut self, delta: &str) -> rusqlite::Result<()> {
        let Some(open) = &mut self.open else { return Ok(()) };
        match &mut open.row.part {
            Part::Text { text } | Part::Reasoning { text, .. } => text.push_str(delta),
            _ => return Ok(()),
        }
        self.hub.publish(Event::PartDelta {
            session_id: self.message.session_id.clone(),
            message_id: self.message.id.clone(),
            part_id: open.row.id.clone(),
            delta: delta.into(),
        });
        Ok(())
    }

    fn edit(&mut self, change: impl FnOnce(&mut Part)) -> rusqlite::Result<()> {
        if let Some(open) = &mut self.open {
            change(&mut open.row.part);
        }
        Ok(())
    }

    /// Persists the open block; tool calls are queued for execution once the message ends.
    pub fn stop_block(&mut self) -> rusqlite::Result<()> {
        let Some(mut open) = self.open.take() else { return Ok(()) };
        if let Part::ToolCall { input, .. } = &mut open.row.part {
            *input = parse_input(&open.tool_json);
        }
        self.store.save_part(&open.row)?;
        self.hub.publish(Event::PartUpdated { part: open.row.clone() });
        if matches!(open.row.part, Part::ToolCall { .. }) {
            self.calls.push(open.row);
        }
        Ok(())
    }
}

fn parse_input(json: &str) -> Value {
    if json.trim().is_empty() {
        return Value::Object(Default::default());
    }
    serde_json::from_str(json).unwrap_or_else(|_| Value::String(json.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::types::{Role, Visibility};
    use crate::store::{tests::store, NewSession};

    #[test]
    fn assembles_text_reasoning_and_tool_calls() {
        let store = store();
        let hub = Hub::new(64);
        let session = store
            .create_session(NewSession { workspace_id: "w", parent_id: None, visibility: Visibility::Sibling, title: "", agent: "build", model: None })
            .unwrap();
        let message = store.create_message(&session.id, Role::Assistant, None).unwrap();
        let mut rx = hub.attach(None).rx;
        let mut assembler = Assembler::new(&store, &hub, &message);
        for chunk in [
            Chunk::Usage(Usage { input: 5, ..Usage::default() }),
            Chunk::ReasoningStart,
            Chunk::ReasoningDelta("hm".into()),
            Chunk::ReasoningSignature("sig".into()),
            Chunk::BlockStop,
            Chunk::TextStart,
            Chunk::TextDelta("Hel".into()),
            Chunk::TextDelta("lo".into()),
            Chunk::BlockStop,
            Chunk::ToolUseStart { id: "t1".into(), name: "read".into() },
            Chunk::ToolInputDelta("{\"path\":".into()),
            Chunk::ToolInputDelta(" \"a\"}".into()),
            Chunk::BlockStop,
            Chunk::Usage(Usage { output: 9, ..Usage::default() }),
            Chunk::Stop(StopReason::ToolUse),
        ] {
            assembler.apply(chunk).unwrap();
        }
        assert_eq!(assembler.usage, Usage { input: 5, output: 9, cache_read: 0, cache_write: 0 });
        assert_eq!(assembler.stop, Some(StopReason::ToolUse));
        assert_eq!(assembler.calls.len(), 1);
        let parts = store.transcript(&session.id).unwrap().remove(0).parts;
        assert_eq!(parts[0].part, Part::Reasoning { text: "hm".into(), signature: Some("sig".into()), redacted: None });
        assert_eq!(parts[1].part, Part::Text { text: "Hello".into() });
        let Part::ToolCall { input, status, .. } = &parts[2].part else { panic!() };
        assert_eq!(input, &serde_json::json!({ "path": "a" }));
        assert_eq!(*status, ToolStatus::Pending);

        let mut kinds = Vec::new();
        while let Ok(envelope) = rx.try_recv() {
            kinds.push(match envelope.event {
                Event::PartCreated { .. } => "created",
                Event::PartDelta { .. } => "delta",
                Event::PartUpdated { .. } => "updated",
                _ => "other",
            });
        }
        assert_eq!(kinds, ["created", "delta", "updated", "created", "delta", "delta", "updated", "created", "updated"]);
    }

    #[test]
    fn empty_or_broken_tool_input_is_tolerated() {
        assert_eq!(parse_input(""), serde_json::json!({}));
        assert_eq!(parse_input("{\"a\": 1}"), serde_json::json!({ "a": 1 }));
        assert_eq!(parse_input("{oops"), Value::String("{oops".into()));
    }
}
