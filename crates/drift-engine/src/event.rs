//! Engine events: a monotonic sequence with bounded replay so clients can resume after a drop.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use utoipa::ToSchema;

use crate::permission::{Decision, Request as PermissionRequest};
use crate::question::Request as QuestionRequest;
use crate::session::types::{Message, PartRow, Session, Todo};
use crate::store::Workspace;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Idle,
    Running,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    #[serde(rename = "workspace.created")]
    WorkspaceCreated { workspace: Workspace },
    #[serde(rename = "session.created")]
    SessionCreated { session: Session },
    #[serde(rename = "session.updated")]
    SessionUpdated { session: Session },
    #[serde(rename = "session.status", rename_all = "camelCase")]
    SessionStatusChanged { session_id: String, status: SessionStatus },
    #[serde(rename = "message.created")]
    MessageCreated { message: Message },
    #[serde(rename = "message.updated")]
    MessageUpdated { message: Message },
    #[serde(rename = "part.created")]
    PartCreated { part: PartRow },
    #[serde(rename = "part.updated")]
    PartUpdated { part: PartRow },
    /// Streamed text appended to a `text` or `reasoning` part; the part itself is saved later.
    #[serde(rename = "part.delta", rename_all = "camelCase")]
    PartDelta { session_id: String, message_id: String, part_id: String, delta: String },
    #[serde(rename = "todo.updated", rename_all = "camelCase")]
    TodoUpdated { session_id: String, todos: Vec<Todo> },
    #[serde(rename = "permission.asked")]
    PermissionAsked { request: PermissionRequest },
    #[serde(rename = "permission.replied", rename_all = "camelCase")]
    PermissionReplied { request_id: String, session_id: String, decision: Decision },
    #[serde(rename = "question.asked")]
    QuestionAsked { request: QuestionRequest },
    #[serde(rename = "question.replied", rename_all = "camelCase")]
    QuestionReplied { request_id: String, session_id: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Envelope {
    pub seq: u64,
    #[serde(flatten)]
    pub event: Event,
}

pub struct Hub {
    /// Random per process; sequence numbers only mean something within one instance.
    pub instance: String,
    ring: Mutex<Ring>,
    tx: broadcast::Sender<Envelope>,
}

struct Ring {
    next_seq: u64,
    capacity: usize,
    events: VecDeque<Envelope>,
}

/// What a client gets when it attaches: the events it missed, or word that it missed too many.
pub struct Attached {
    /// Head of the sequence at the moment of attaching; replay ends here.
    pub seq: u64,
    pub replay: Replay,
    pub rx: broadcast::Receiver<Envelope>,
}

#[derive(Debug, PartialEq)]
pub enum Replay {
    Events(Vec<Envelope>),
    Stale,
}

impl Hub {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self {
            instance: crate::random_hex(8),
            ring: Mutex::new(Ring {
                next_seq: 1,
                capacity,
                events: VecDeque::with_capacity(capacity),
            }),
            tx,
        }
    }

    pub fn publish(&self, event: Event) -> u64 {
        let mut ring = self.ring.lock().unwrap();
        let seq = ring.next_seq;
        ring.next_seq += 1;
        let envelope = Envelope { seq, event };
        if ring.events.len() == ring.capacity {
            ring.events.pop_front();
        }
        ring.events.push_back(envelope.clone());
        let _ = self.tx.send(envelope);
        seq
    }

    /// The last published sequence number, or 0 when nothing has been published.
    pub fn seq(&self) -> u64 {
        self.ring.lock().unwrap().next_seq - 1
    }

    /// Subscribes and replays everything after `cursor` in one step so no event falls between.
    pub fn attach(&self, cursor: Option<u64>) -> Attached {
        let ring = self.ring.lock().unwrap();
        let rx = self.tx.subscribe();
        let replay = match cursor {
            None => Replay::Events(Vec::new()),
            Some(cursor) => ring.since(cursor),
        };
        Attached {
            seq: ring.next_seq - 1,
            replay,
            rx,
        }
    }
}

impl Ring {
    /// A cursor past the head belongs to another process lifetime, so it is stale too.
    fn since(&self, cursor: u64) -> Replay {
        let last = self.next_seq - 1;
        if cursor == last {
            return Replay::Events(Vec::new());
        }
        if cursor > last {
            return Replay::Stale;
        }
        let oldest = self.events.front().map(|e| e.seq).unwrap_or(self.next_seq);
        if cursor + 1 < oldest {
            return Replay::Stale;
        }
        Replay::Events(self.events.iter().filter(|e| e.seq > cursor).cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(name: &str) -> Event {
        Event::WorkspaceCreated {
            workspace: Workspace {
                id: name.into(),
                path: format!("C:/{name}"),
                name: name.into(),
                icon: String::new(),
                last_used: 0,
            },
        }
    }

    #[test]
    fn sequence_is_monotonic_from_one() {
        let hub = Hub::new(8);
        assert_eq!(hub.seq(), 0);
        assert_eq!(hub.publish(workspace("a")), 1);
        assert_eq!(hub.publish(workspace("b")), 2);
        assert_eq!(hub.seq(), 2);
    }

    #[test]
    fn attach_without_cursor_replays_nothing() {
        let hub = Hub::new(8);
        hub.publish(workspace("a"));
        assert_eq!(hub.attach(None).replay, Replay::Events(Vec::new()));
    }

    #[test]
    fn attach_replays_events_after_cursor() {
        let hub = Hub::new(8);
        for name in ["a", "b", "c"] {
            hub.publish(workspace(name));
        }
        let Replay::Events(events) = hub.attach(Some(1)).replay else {
            panic!("expected replay");
        };
        assert_eq!(events.iter().map(|e| e.seq).collect::<Vec<_>>(), [2, 3]);
    }

    #[test]
    fn attach_at_head_replays_nothing() {
        let hub = Hub::new(8);
        hub.publish(workspace("a"));
        assert_eq!(hub.attach(Some(1)).replay, Replay::Events(Vec::new()));
    }

    #[test]
    fn cursor_beyond_head_is_stale() {
        let hub = Hub::new(8);
        hub.publish(workspace("a"));
        assert_eq!(hub.attach(Some(9)).replay, Replay::Stale);
    }

    #[test]
    fn cursor_older_than_ring_is_stale() {
        let hub = Hub::new(2);
        for name in ["a", "b", "c"] {
            hub.publish(workspace(name));
        }
        assert_eq!(hub.attach(Some(0)).replay, Replay::Stale);
        let Replay::Events(events) = hub.attach(Some(1)).replay else {
            panic!("cursor at the ring's edge must still replay");
        };
        assert_eq!(events.iter().map(|e| e.seq).collect::<Vec<_>>(), [2, 3]);
    }

    #[test]
    fn empty_ring_with_zero_cursor_is_not_stale() {
        let hub = Hub::new(2);
        assert_eq!(hub.attach(Some(0)).replay, Replay::Events(Vec::new()));
    }

    #[tokio::test]
    async fn attached_receiver_sees_later_events() {
        let hub = Hub::new(8);
        let mut attached = hub.attach(None);
        hub.publish(workspace("a"));
        let received = attached.rx.recv().await.unwrap();
        assert_eq!(received.seq, 1);
    }

    #[test]
    fn envelope_serialises_flat_with_type_and_seq() {
        let envelope = Envelope { seq: 7, event: workspace("a") };
        let json = serde_json::to_value(&envelope).unwrap();
        assert_eq!(json["seq"], 7);
        assert_eq!(json["type"], "workspace.created");
        assert_eq!(json["workspace"]["name"], "a");
    }
}
