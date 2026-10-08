//! Turns a chunk stream into stored parts, publishing deltas as they arrive.

use serde_json::Value;

use crate::event::{Event, Hub};
use crate::llm::{Chunk, StopReason};
use crate::session::types::{Message, Part, PartRow, ToolStatus, Usage};
use crate::store::Store;

pub(crate) struct Assembler<'a> {
    store: &'a Store,
    hub: &'a Hub,
    message: &'a Message,
    open: Option<Open>,
    pub usage: Usage,
    pub stop: Option<StopReason>,
    pub calls: Vec<PartRow>,
}

/// How often a streaming part is written to disk as it stands, so a crash loses little of it.
const CHECKPOINT: std::time::Duration = std::time::Duration::from_secs(2);

/// The block currently streaming; tool input arrives as JSON text and is parsed at stop.
struct Open {
    row: PartRow,
    tool_json: String,
    /// Its text's length in UTF-16 units, which each delta names as its offset.
    length: usize,
    saved: std::time::Instant,
}

impl<'a> Assembler<'a> {
    pub(crate) fn new(store: &'a Store, hub: &'a Hub, message: &'a Message) -> Self {
        Self {
            store,
            hub,
            message,
            open: None,
            usage: Usage::default(),
            stop: None,
            calls: Vec::new(),
        }
    }

    pub(crate) fn apply(&mut self, chunk: Chunk) -> rusqlite::Result<()> {
        match chunk {
            Chunk::TextStart => self.start(Part::Text { text: String::new() }),
            Chunk::ReasoningStart => self.start(Part::Reasoning {
                text: String::new(),
                signature: None,
                redacted: None,
            }),
            Chunk::ReasoningRedacted(data) => {
                self.start(Part::Reasoning {
                    text: String::new(),
                    signature: None,
                    redacted: Some(data),
                })?;
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
            Chunk::PartSignature(signature) => {
                if let Some(open) = &mut self.open {
                    open.row.provider_signature = Some(signature);
                }
                Ok(())
            }
            Chunk::ToolInputDelta(json) => {
                if let Some(open) = &mut self.open {
                    open.tool_json.push_str(&json);
                }
                Ok(())
            }
            Chunk::BlockStop => self.stop_block(),
            Chunk::Usage(usage) => {
                self.usage.merge(usage);
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
        self.open = Some(Open {
            row,
            tool_json: String::new(),
            length: 0,
            saved: std::time::Instant::now(),
        });

        Ok(())
    }

    /// Updates the readable prefix before publishing its delta.
    fn delta(&mut self, delta: &str) -> rusqlite::Result<()> {
        let Some(open) = &mut self.open else { return Ok(()) };
        match &mut open.row.part {
            Part::Text { text } | Part::Reasoning { text, .. } => text.push_str(delta),
            _ => return Ok(()),
        }

        self.store.stream_part(&open.row);
        let offset = open.length;
        open.length += delta.encode_utf16().count();
        self.hub.publish(Event::PartDelta {
            session_id: self.message.session_id.clone(),
            message_id: self.message.id.clone(),
            part_id: open.row.id.clone(),
            delta: delta.into(),
            offset,
        });

        if open.saved.elapsed() >= CHECKPOINT {
            open.saved = std::time::Instant::now();
            self.store.checkpoint_part(&open.row)?;
        }

        Ok(())
    }

    fn edit(&mut self, change: impl FnOnce(&mut Part)) -> rusqlite::Result<()> {
        if let Some(open) = &mut self.open {
            change(&mut open.row.part);
        }

        Ok(())
    }

    /// Persists the open block; tool calls are queued for execution once the message ends.
    pub(crate) fn stop_block(&mut self) -> rusqlite::Result<()> {
        let Some(mut open) = self.open.take() else {
            return Ok(());
        };

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
    use crate::store::{NewSession, tests::store};

    fn session(store: &Store, workspace_id: &str) -> crate::session::types::Session {
        store
            .create_session(NewSession {
                workspace_id,
                parent_id: None,
                visibility: Visibility::Sibling,
                title: "",
                agent: "build",
                model: None,
            })
            .unwrap()
    }

    #[test]
    fn assembles_text_reasoning_and_tool_calls() {
        let store = store();
        let hub = Hub::new(64);
        let session = session(&store, "w");
        let message = store.create_message(&session.id, Role::Assistant, None).unwrap();
        let mut rx = hub.attach(None).rx;
        let mut assembler = Assembler::new(&store, &hub, &message);

        for chunk in [
            Chunk::Usage(Usage {
                input: 5,
                ..Usage::default()
            }),
            Chunk::ReasoningStart,
            Chunk::ReasoningDelta("hm".into()),
            Chunk::ReasoningSignature("sig".into()),
            Chunk::BlockStop,
            Chunk::TextStart,
            Chunk::TextDelta("Hel".into()),
            Chunk::TextDelta("lo".into()),
            Chunk::BlockStop,
            Chunk::ToolUseStart {
                id: "t1".into(),
                name: "read".into(),
            },
            Chunk::ToolInputDelta("{\"path\":".into()),
            Chunk::ToolInputDelta(" \"a\"}".into()),
            Chunk::BlockStop,
            Chunk::Usage(Usage {
                output: 9,
                ..Usage::default()
            }),
            Chunk::Stop(StopReason::ToolUse),
        ] {
            assembler.apply(chunk).unwrap();
        }

        assert_eq!(
            assembler.usage,
            Usage {
                input: 5,
                output: 9,
                cache_read: 0,
                cache_write: 0
            }
        );
        assert_eq!(assembler.stop, Some(StopReason::ToolUse));
        assert_eq!(assembler.calls.len(), 1);

        let parts = store.transcript(&session.id).unwrap().remove(0).parts;
        assert_eq!(
            parts[0].part,
            Part::Reasoning {
                text: "hm".into(),
                signature: Some("sig".into()),
                redacted: None
            }
        );
        assert_eq!(parts[1].part, Part::Text { text: "Hello".into() });
        let Part::ToolCall { input, status, .. } = &parts[2].part else {
            panic!()
        };
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

        assert_eq!(
            kinds,
            [
                "created", "delta", "updated", "created", "delta", "delta", "updated", "created", "updated"
            ]
        );
    }

    #[test]
    fn a_snapshot_mid_stream_holds_the_text_so_far_and_deltas_say_where_they_start() {
        let store = store();
        let hub = Hub::new(64);
        let session = session(&store, "w");
        let message = store.create_message(&session.id, Role::Assistant, None).unwrap();
        let mut rx = hub.attach(None).rx;
        let mut assembler = Assembler::new(&store, &hub, &message);

        for chunk in [
            Chunk::TextStart,
            Chunk::TextDelta("héllo ".into()),
            Chunk::TextDelta("wörld".into()),
        ] {
            assembler.apply(chunk).unwrap();
        }

        let parts = store.transcript(&session.id).unwrap().remove(0).parts;
        assert_eq!(
            parts[0].part,
            Part::Text {
                text: "héllo wörld".into()
            },
            "a reader mid-stream sees every published delta"
        );

        let offsets: Vec<usize> = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|envelope| match envelope.event {
                Event::PartDelta { offset, .. } => Some(offset),
                _ => None,
            })
            .collect();
        assert_eq!(offsets, [0, 6], "in UTF-16 units, as a browser counts");

        assembler.apply(Chunk::BlockStop).unwrap();
        store
            .lock()
            .execute("UPDATE part SET json = '{\"type\":\"text\",\"text\":\"on disk\"}'", [])
            .unwrap();
        let parts = store.transcript(&session.id).unwrap().remove(0).parts;

        assert_eq!(
            parts[0].part,
            Part::Text { text: "on disk".into() },
            "a closed part is read from disk again"
        );
    }

    #[test]
    fn call_signatures_survive_storage_reopen_fork_and_model_replay() {
        let dir = std::env::temp_dir().join(format!("drift-signature-{}", crate::random_hex(4)));
        let store = crate::store::open(&dir).unwrap();
        let workspace = store.add_workspace("ws", "ws", "").unwrap();
        let session = session(&store, &workspace.id);
        let model = crate::session::types::ModelRef {
            provider: "google".into(),
            model: "gemini-3-pro".into(),
        };
        let mut message = store
            .create_message(&session.id, Role::Assistant, Some(&model))
            .unwrap();
        let hub = Hub::new(64);

        {
            let mut assembler = Assembler::new(&store, &hub, &message);
            for chunk in [
                Chunk::ToolUseStart {
                    id: "c".into(),
                    name: "read".into(),
                },
                Chunk::ToolInputDelta(r#"{"path":"a"}"#.into()),
                Chunk::PartSignature("opaque-signature".into()),
                Chunk::BlockStop,
            ] {
                assembler.apply(chunk).unwrap();
            }
        }

        message.status = crate::session::types::MessageStatus::Done;
        store.save_message(&message).unwrap();
        drop(store);

        let store = crate::store::open(&dir).unwrap();
        let transcript = store.transcript(&session.id).unwrap();
        assert_eq!(
            transcript[0].parts[0].provider_signature.as_deref(),
            Some("opaque-signature")
        );

        let mut sent = Vec::new();
        super::super::convert::append(&mut sent, &transcript, &model);

        let crate::llm::Block::Signed { part, signature } = &sent[0].blocks[0] else {
            panic!("a signed call replays signed to the same model");
        };
        assert_eq!(signature, "opaque-signature");
        assert!(matches!(part.as_ref(), crate::llm::Block::ToolUse { .. }));

        let fork = store
            .fork_session(
                &session.id,
                NewSession {
                    workspace_id: &workspace.id,
                    parent_id: None,
                    visibility: Visibility::Sibling,
                    title: "fork",
                    agent: "build",
                    model: None,
                },
                &message.id,
                None,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            store.transcript(&fork.id).unwrap()[0].parts[0]
                .provider_signature
                .as_deref(),
            Some("opaque-signature")
        );

        let mut switched = Vec::new();
        super::super::convert::append(
            &mut switched,
            &transcript,
            &crate::session::types::ModelRef {
                provider: "openai".into(),
                model: "gpt".into(),
            },
        );

        assert!(matches!(switched[0].blocks[0], crate::llm::Block::ToolUse { .. }));

        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn empty_or_broken_tool_input_is_tolerated() {
        assert_eq!(parse_input(""), serde_json::json!({}));
        assert_eq!(parse_input("{\"a\": 1}"), serde_json::json!({ "a": 1 }));
        assert_eq!(parse_input("{oops"), Value::String("{oops".into()));
    }
}
