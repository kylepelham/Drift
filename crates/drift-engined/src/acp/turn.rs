//! One prompt turn: the engine's events for the session become `session/update` notifications.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use drift_engine::Engine;
use drift_engine::event::{Envelope, Event, SessionStatus};
use drift_engine::permission::{self, Reply, ReplyBody};
use drift_engine::session::turn::{Prompt, TurnError};
use drift_engine::session::types::{Ending, Message, MessageStatus, Part, PartRow, Role};
use serde_json::{Value, json};
use tokio::sync::broadcast::{Receiver, error::RecvError};

use super::rpc::{Peer, RpcError};
use super::{chunk, content, thought, tool};

/// How long the turn goes without an event before it asks the engine whether it is still running.
const IDLE_CHECK: Duration = Duration::from_secs(1);
/// How far up a chain of subagents an ask is traced to the session the editor drives.
const MAX_DEPTH: usize = 4;

fn refused(error: &TurnError) -> RpcError {
    match error {
        TurnError::NoModel => RpcError::invalid_request(
            "no model selected: start drift-engined with --model provider/model, or set model in drift.json",
        ),
        other => RpcError::internal(other),
    }
}

/// Which kind of streamed text a part holds.
#[derive(Clone, Copy)]
enum Text {
    Reply,
    Reasoning,
}

pub(super) struct Turn {
    engine: Arc<Engine>,
    peer: Peer,
    root: String,
    cwd: PathBuf,
    /// Subagent sessions under this one, whose asks the editor answers too.
    children: HashSet<String>,
    /// Whether each message seen is the assistant's.
    roles: HashMap<String, bool>,
    texts: HashMap<String, Text>,
    /// Each call announced to the client, by the model's call id, as the part that holds it.
    calls: HashMap<String, String>,
    /// The session's latest reply, which says how the turn ended.
    last: Option<Message>,
}

impl Turn {
    pub(super) fn new(engine: Arc<Engine>, peer: Peer, session_id: &str, cwd: PathBuf) -> Self {
        Self {
            engine,
            peer,
            root: session_id.into(),
            cwd,
            children: HashSet::new(),
            roles: HashMap::new(),
            texts: HashMap::new(),
            calls: HashMap::new(),
            last: None,
        }
    }

    pub(super) async fn run(mut self, blocks: &[Value], cancelled: &AtomicBool) -> Result<Value, RpcError> {
        let prompt = Prompt {
            parts: content::parts(blocks)?,
            ..Prompt::default()
        };
        // Subscribed before the prompt goes in, so nothing the turn does is missed.
        let mut events = self.engine.hub.attach(None).rx;
        match self.engine.submit(&self.root, prompt).await {
            Ok(_) => {}
            Err(TurnError::Stopped) => return Ok(json!({ "stopReason": "cancelled" })),
            Err(error) => return Err(refused(&error)),
        }
        if cancelled.load(Ordering::SeqCst) {
            self.engine.abort(&self.root);
        }

        self.follow(&mut events).await;
        self.finish(cancelled.load(Ordering::SeqCst))
    }

    /// Translates events until the session goes idle; a lagging or silent feed asks the engine instead.
    async fn follow(&mut self, events: &mut Receiver<Envelope>) {
        loop {
            match tokio::time::timeout(IDLE_CHECK, events.recv()).await {
                Ok(Ok(envelope)) => {
                    if self.on(envelope.event) && !self.running() {
                        return;
                    }
                }
                Ok(Err(RecvError::Lagged(missed))) => eprintln!("drift: acp missed {missed} engine events"),
                Ok(Err(RecvError::Closed)) => return,
                Err(_) if !self.running() => return,
                Err(_) => {}
            }
        }
    }

    fn running(&self) -> bool {
        self.engine.turns.is_running(&self.root)
    }

    /// True when the session went idle.
    fn on(&mut self, event: Event) -> bool {
        match event {
            Event::MessageCreated { message } | Event::MessageUpdated { message } => self.message(message),
            Event::PartCreated { part } => self.part(&part, true),
            Event::PartUpdated { part } => self.part(&part, false),
            Event::PartDelta {
                session_id,
                message_id,
                part_id,
                delta,
                ..
            } if session_id == self.root => self.delta(&message_id, &part_id, &delta),
            Event::TodoUpdated { session_id, todos } if session_id == self.root => self.send(tool::plan(&todos)),
            Event::PermissionAsked { request } if self.owns(&request.session_id) => self.ask(request),
            Event::QuestionAsked { request } if !request.is_async && self.owns(&request.session_id) => {
                // ACP has no question card; declining lets the model go on with what it has.
                let _ = self.engine.questions.reply(&self.engine.hub, &request.id, None);
            }
            Event::SessionStatusChanged {
                session_id,
                status: SessionStatus::Idle,
            } => return session_id == self.root,
            _ => {}
        }

        false
    }

    /// The session itself, or a subagent under it, found through the stored parents.
    fn owns(&mut self, session_id: &str) -> bool {
        let mut lineage = Vec::new();
        let mut current = session_id.to_string();
        while lineage.len() < MAX_DEPTH {
            if current == self.root || self.children.contains(&current) {
                self.children.extend(lineage);
                return true;
            }
            let parent = self
                .engine
                .store
                .session(&current)
                .ok()
                .flatten()
                .and_then(|session| session.parent_id);
            let Some(parent) = parent else { return false };
            lineage.push(std::mem::replace(&mut current, parent));
        }

        false
    }

    fn message(&mut self, message: Message) {
        if message.session_id != self.root {
            return;
        }

        let assistant = message.role == Role::Assistant;
        self.roles.insert(message.id.clone(), assistant);
        if assistant {
            self.last = Some(message);
        }
    }

    /// A message from before the turn began is looked up once.
    fn assistant(&mut self, message_id: &str) -> bool {
        if let Some(known) = self.roles.get(message_id) {
            return *known;
        }

        let found = self
            .engine
            .store
            .message(message_id)
            .ok()
            .flatten()
            .is_some_and(|message| message.role == Role::Assistant);
        self.roles.insert(message_id.into(), found);
        found
    }

    fn part(&mut self, row: &PartRow, created: bool) {
        if row.session_id != self.root {
            return;
        }

        match &row.part {
            Part::Text { text } => self.text(row, Text::Reply, text, created),
            Part::Reasoning { text, .. } => self.text(row, Text::Reasoning, text, created),
            Part::ToolCall { call_id, .. } => {
                let first = self.calls.insert(call_id.clone(), row.id.clone()).as_ref() != Some(&row.id);
                if let Some(update) = tool::update(row, &self.cwd, first) {
                    self.send(update);
                }
            }
            _ => {}
        }
    }

    /// Streamed parts start empty and arrive as deltas; one written whole arrives with its text.
    fn text(&mut self, row: &PartRow, kind: Text, text: &str, created: bool) {
        self.texts.insert(row.id.clone(), kind);
        if created && !text.is_empty() {
            self.delta(&row.message_id, &row.id, text);
        }
    }

    fn delta(&mut self, message_id: &str, part_id: &str, delta: &str) {
        let Some(kind) = self.texts.get(part_id).copied() else {
            return;
        };
        if !self.assistant(message_id) {
            return;
        }

        self.send(match kind {
            Text::Reply => chunk("agent_message_chunk", message_id, delta),
            Text::Reasoning => thought(message_id, delta),
        });
    }

    fn send(&self, update: Value) {
        self.peer
            .notify("session/update", json!({ "sessionId": self.root, "update": update }));
    }

    /// Puts the ask to the editor without holding up the stream; a turn that ends first leaves the answer unused.
    fn ask(&self, request: permission::Request) {
        let tool_call_id = Some(&request.call_id)
            .filter(|_| request.session_id == self.root)
            .and_then(|call_id| self.calls.get(call_id))
            .cloned()
            .unwrap_or_else(|| request.id.clone());
        let params = json!({
            "sessionId": self.root,
            "toolCall": tool::asked(&request, &tool_call_id),
            "options": tool::options(&request),
        });
        let (engine, peer) = (self.engine.clone(), self.peer.clone());

        tokio::spawn(async move {
            let answer = peer.request("session/request_permission", params).await;
            let body = ReplyBody {
                reply: reply(answer.as_ref()),
                pattern: None,
                message: None,
            };
            let _ = engine.permissions.reply(&engine.hub, &request.id, body);
        });
    }

    fn finish(&self, cancelled: bool) -> Result<Value, RpcError> {
        let reason = if cancelled { "cancelled" } else { self.stop_reason()? };

        Ok(json!({ "stopReason": reason }))
    }

    fn stop_reason(&self) -> Result<&'static str, RpcError> {
        let Some(last) = &self.last else {
            return Ok("end_turn");
        };

        match (last.status, last.ending) {
            (MessageStatus::Aborted, _) => Ok("cancelled"),
            (MessageStatus::Error, _) => Err(RpcError::internal(last.error.as_deref().unwrap_or("the turn failed"))),
            (MessageStatus::Paused, _) => {
                // A paused turn has no reply of its own, so the reason is said where the user reads.
                let reason = last.error.as_deref().unwrap_or("The turn paused.");
                self.send(chunk("agent_message_chunk", &last.id, reason));
                Ok("max_turn_requests")
            }
            (_, Some(Ending::Length)) => Ok("max_tokens"),
            (_, Some(Ending::Refused)) => Ok("refusal"),
            _ => Ok("end_turn"),
        }
    }
}

/// The editor's choice as Drift's reply; a cancelled or failed ask refuses the call.
fn reply(answer: Option<&Value>) -> Reply {
    let outcome = answer.map(|answer| &answer["outcome"]);
    let selected = outcome
        .filter(|outcome| outcome["outcome"] == "selected")
        .and_then(|outcome| outcome["optionId"].as_str());

    match selected {
        Some(tool::ALLOW_ONCE) => Reply::Once,
        Some(tool::ALLOW_ALWAYS) => Reply::Always,
        _ => Reply::Deny,
    }
}
