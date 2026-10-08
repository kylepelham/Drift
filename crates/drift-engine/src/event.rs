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
    #[serde(rename = "session.deleted", rename_all = "camelCase")]
    SessionDeleted { session_id: String },
    #[serde(rename = "session.status", rename_all = "camelCase")]
    SessionStatusChanged { session_id: String, status: SessionStatus },
    /// A turn is waiting to retry a failed request; `running` follows when it tries again.
    #[serde(rename = "session.retry", rename_all = "camelCase")]
    SessionRetry {
        session_id: String,
        attempt: u32,
        message: String,
        next_at: i64,
    },
    #[serde(rename = "message.created")]
    MessageCreated { message: Message },
    #[serde(rename = "message.updated")]
    MessageUpdated { message: Message },
    /// Gone for good, as when a new prompt commits an undo.
    #[serde(rename = "message.removed", rename_all = "camelCase")]
    MessageRemoved { session_id: String, message_id: String },
    #[serde(rename = "part.created")]
    PartCreated { part: PartRow },
    #[serde(rename = "part.updated")]
    PartUpdated { part: PartRow },
    /// Text appended at `offset` UTF-16 units, allowing clients to skip deltas already in their snapshot.
    #[serde(rename = "part.delta", rename_all = "camelCase")]
    PartDelta {
        session_id: String,
        message_id: String,
        part_id: String,
        delta: String,
        offset: usize,
    },
    #[serde(rename = "todo.updated", rename_all = "camelCase")]
    TodoUpdated { session_id: String, todos: Vec<Todo> },
    #[serde(rename = "permission.asked")]
    PermissionAsked { request: PermissionRequest },
    #[serde(rename = "permission.replied", rename_all = "camelCase")]
    PermissionReplied {
        request_id: String,
        session_id: String,
        decision: Decision,
    },
    /// The model catalog was refreshed; clients reload `/providers`.
    #[serde(rename = "catalog.updated")]
    CatalogUpdated {},
    #[serde(rename = "mcp.updated")]
    McpUpdated { server: crate::mcp::ServerStatus },
    #[serde(rename = "mcp.removed")]
    McpRemoved { name: String },
    #[serde(rename = "question.asked")]
    QuestionAsked { request: QuestionRequest },
    #[serde(rename = "question.replied", rename_all = "camelCase")]
    QuestionReplied { request_id: String, session_id: String },
    /// A worker was launched, started, ended, or its result reached its parent.
    #[serde(rename = "task.updated")]
    TaskUpdated { task: crate::session::tasks::TaskRecord },
    /// A plugin has something to tell the user; `tone` is info, success, warning or error.
    #[serde(rename = "plugin.notice")]
    PluginNotice {
        plugin: String,
        title: String,
        body: String,
        tone: String,
    },
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
    sender: broadcast::Sender<Envelope>,
}

struct Ring {
    next_seq: u64,
    capacity: usize,
    events: VecDeque<Envelope>,
    /// The newest sequence number pushed out of the window; a client behind it missed something.
    evicted: u64,
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
        let (sender, _) = broadcast::channel(capacity);

        Self {
            instance: crate::random_hex(8),
            ring: Mutex::new(Ring {
                next_seq: 1,
                capacity,
                events: VecDeque::with_capacity(capacity),
                evicted: 0,
            }),
            sender,
        }
    }

    pub fn publish(&self, event: Event) -> u64 {
        let mut ring = self.ring.lock().unwrap();
        let seq = ring.next_seq;
        ring.next_seq += 1;
        let envelope = Envelope { seq, event };

        if ring.events.len() == ring.capacity
            && let Some(dropped) = ring.events.pop_front()
        {
            ring.evicted = dropped.seq;
        }
        ring.events.push_back(envelope.clone());
        let _ = self.sender.send(envelope);

        seq
    }

    /// An event shown live and never replayed (a running command's output so far): it takes a
    /// sequence number, so clients keep their order, but stays out of the replay window.
    pub fn publish_transient(&self, event: Event) -> u64 {
        let mut ring = self.ring.lock().unwrap();
        let seq = ring.next_seq;
        ring.next_seq += 1;
        let _ = self.sender.send(Envelope { seq, event });

        seq
    }

    /// The last published sequence number, or 0 when nothing has been published.
    pub fn seq(&self) -> u64 {
        self.ring.lock().unwrap().next_seq - 1
    }

    /// Subscribes and replays everything after `cursor` in one step so no event falls between.
    pub fn attach(&self, cursor: Option<u64>) -> Attached {
        let ring = self.ring.lock().unwrap();
        let rx = self.sender.subscribe();
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
        // Transient events leave gaps in the window, so only an evicted event makes a cursor stale.
        if cursor < self.evicted {
            return Replay::Stale;
        }
        Replay::Events(self.events.iter().filter(|event| event.seq > cursor).cloned().collect())
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

    /// What a socket does when its receiver lags (`api::events`): attach again at the last `seq` it sent.
    #[tokio::test]
    async fn a_socket_that_lags_catches_up_from_the_window_or_resyncs() {
        let hub = Hub::new(4);
        let mut slow = hub.attach(None).rx;
        hub.publish(workspace("kept"));
        for _ in 0..6 {
            hub.publish_transient(workspace("live output"));
        }
        assert!(
            matches!(slow.recv().await, Err(broadcast::error::RecvError::Lagged(_))),
            "more than the receiver holds went unread"
        );
        let Replay::Events(missed) = hub.attach(Some(0)).replay else {
            panic!("live output crowded the receiver, not the window")
        };
        assert_eq!(
            missed.iter().map(|e| e.seq).collect::<Vec<_>>(),
            [1],
            "the durable event is replayed; live output is not, by design"
        );

        let mut stalled = hub.attach(None).rx;
        let last = hub.seq();
        for name in ["a", "b", "c", "d", "e", "f"] {
            hub.publish(workspace(name));
        }
        assert!(matches!(
            stalled.recv().await,
            Err(broadcast::error::RecvError::Lagged(_))
        ));
        assert_eq!(
            hub.attach(Some(last)).replay,
            Replay::Stale,
            "the window moved past it: the client resyncs and hydrates"
        );
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

    #[tokio::test]
    async fn transient_events_are_seen_live_but_never_replayed_or_crowd_the_window() {
        let hub = Hub::new(16);
        let mut live = hub.attach(None);
        hub.publish(workspace("a"));
        for _ in 0..10 {
            hub.publish_transient(workspace("progress"));
        }
        hub.publish(workspace("b"));
        let seqs: Vec<u64> = (0..12).map(|_| live.rx.try_recv().unwrap().seq).collect();
        assert_eq!(seqs, (1..=12).collect::<Vec<_>>(), "live order is kept");
        let small = Hub::new(2);
        small.publish(workspace("a"));
        for _ in 0..10 {
            small.publish_transient(workspace("progress"));
        }
        assert!(
            matches!(small.attach(Some(0)).replay, Replay::Events(ref kept) if kept.len() == 1),
            "progress never pushes a real event out"
        );
        let Replay::Events(events) = hub.attach(Some(0)).replay else {
            panic!("nothing real was evicted, so not stale")
        };
        assert_eq!(
            events.iter().map(|e| e.seq).collect::<Vec<_>>(),
            [1, 12],
            "only real events replay"
        );
        let Replay::Events(after) = hub.attach(Some(5)).replay else {
            panic!("a cursor on a transient event resumes")
        };
        assert_eq!(after.iter().map(|e| e.seq).collect::<Vec<_>>(), [12]);
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
        let envelope = Envelope {
            seq: 7,
            event: workspace("a"),
        };
        let json = serde_json::to_value(&envelope).unwrap();
        assert_eq!(json["seq"], 7);
        assert_eq!(json["type"], "workspace.created");
        assert_eq!(json["workspace"]["name"], "a");
    }
}
