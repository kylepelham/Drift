use super::Store;
use super::parts::{insert_part, map_part};
use super::sessions::transaction;
use crate::id;
use crate::session::types::{Message, MessageStatus, MessageWithParts, ModelRef, Part, PartRow, Role, Usage};
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::collections::HashMap;

#[cfg(test)]
#[path = "tests/messages.rs"]
mod tests;

const MESSAGE_COLUMNS: &str = "id, session_id, role, status, model_provider, model_id, usage_json, cost, error, created_at, finished_at, summary, agent, ending, generation_ms";

pub(super) struct NewMessage<'a> {
    session_id: &'a str,
    role: Role,
    model: Option<&'a ModelRef>,
    agent: Option<&'a str>,
    summary: bool,
}

impl<'a> NewMessage<'a> {
    pub(super) fn new(session_id: &'a str, role: Role, model: Option<&'a ModelRef>) -> Self {
        Self {
            session_id,
            role,
            model,
            agent: None,
            summary: false,
        }
    }
}

impl Store {
    pub fn create_message(&self, session_id: &str, role: Role, model: Option<&ModelRef>) -> rusqlite::Result<Message> {
        insert_message(&self.lock(), NewMessage::new(session_id, role, model))
    }

    /// A turn's reply, marked with the agent that turn runs as, whatever the session has since switched to.
    pub fn create_reply(&self, session_id: &str, model: &ModelRef, agent: &str) -> rusqlite::Result<Message> {
        insert_message(
            &self.lock(),
            NewMessage {
                agent: Some(agent),
                ..NewMessage::new(session_id, Role::Assistant, Some(model))
            },
        )
    }

    /// The streaming assistant message a compaction writes its summary into.
    pub fn create_summary_message(&self, session_id: &str, model: &ModelRef) -> rusqlite::Result<Message> {
        insert_message(
            &self.lock(),
            NewMessage {
                summary: true,
                ..NewMessage::new(session_id, Role::Assistant, Some(model))
            },
        )
    }

    pub fn save_message(&self, message: &Message) -> rusqlite::Result<()> {
        save_message_in(&self.lock(), message)
    }

    /// Deletes a reply that holds nothing, such as a request the provider refused; true when it went.
    pub fn discard_empty_reply(&self, id: &str) -> rusqlite::Result<bool> {
        let deleted = self.lock()
            .prepare_cached("DELETE FROM message WHERE id = ?1 AND role = 'assistant' AND NOT EXISTS (SELECT 1 FROM part WHERE message_id = ?1)")?
            .execute([id])?;

        Ok(deleted > 0)
    }

    /// Stores a summary's text and its finished state as one write: a summary is never done without its text.
    pub fn complete_summary(&self, message: &Message, text: &str) -> rusqlite::Result<PartRow> {
        transaction(&self.lock(), |connection| {
            let row = insert_part(
                connection,
                &message.id,
                &message.session_id,
                Part::Text { text: text.into() },
            )?;
            save_message_in(connection, message)?;
            Ok(row)
        })
    }

    pub fn message(&self, id: &str) -> rusqlite::Result<Option<Message>> {
        self.lock()
            .prepare_cached(&format!("SELECT {MESSAGE_COLUMNS} FROM message WHERE id = ?1"))?
            .query_row([id], map_message)
            .optional()
    }

    /// Newest page last, so the caller can prepend older pages as the user scrolls up.
    pub fn messages(
        &self,
        session_id: &str,
        before: Option<&str>,
        limit: usize,
    ) -> rusqlite::Result<Vec<MessageWithParts>> {
        let connection = self.lock();
        let mut statement = connection.prepare_cached(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM (
                SELECT * FROM message WHERE session_id = ?1 AND (?2 IS NULL OR id < ?2)
                ORDER BY id DESC LIMIT ?3
             ) ORDER BY id ASC"
        ))?;
        let messages = statement
            .query_map(params![session_id, before, limit as i64], map_message)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        with_parts_in(self, &connection, session_id, messages)
    }

    /// A session's messages in order without their parts: for deciding about a long history without loading it.
    pub fn message_infos(&self, session_id: &str) -> rusqlite::Result<Vec<Message>> {
        self.lock()
            .prepare_cached(&format!(
                "SELECT {MESSAGE_COLUMNS} FROM message WHERE session_id = ?1 ORDER BY id"
            ))?
            .query_map([session_id], map_message)?
            .collect()
    }

    /// All messages of a session in order: what a turn sends back to the model.
    pub fn transcript(&self, session_id: &str) -> rusqlite::Result<Vec<MessageWithParts>> {
        self.messages(session_id, None, usize::MAX / 2)
    }

    /// The messages from `from` on, in order.
    pub fn messages_from(&self, session_id: &str, from: &str) -> rusqlite::Result<Vec<MessageWithParts>> {
        let connection = self.lock();
        let messages = connection
            .prepare_cached(&format!(
                "SELECT {MESSAGE_COLUMNS} FROM message WHERE session_id = ?1 AND id >= ?2 ORDER BY id"
            ))?
            .query_map(params![session_id, from], map_message)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        with_parts_in(self, &connection, session_id, messages)
    }

    /// Where the model's view of a compacted session starts: the kept tail of the latest finished
    /// summary, else its compaction boundary. `None` when nothing has been summarised.
    pub fn view_start(&self, session_id: &str) -> rusqlite::Result<Option<String>> {
        let connection = self.lock();
        let summary: Option<String> = connection
            .prepare_cached("SELECT id FROM message WHERE session_id = ?1 AND summary = 1 AND status = 'done' ORDER BY id DESC LIMIT 1")?
            .query_row([session_id], |row| row.get(0)).optional()?;
        let Some(summary) = summary else { return Ok(None) };

        let boundary: Option<(String, String)> = connection.prepare_cached(
            "SELECT m.id, p.json FROM message m JOIN part p ON p.message_id = m.id
             WHERE m.session_id = ?1 AND m.id < ?2 AND json_extract(p.json, '$.type') = 'compaction' ORDER BY m.id DESC LIMIT 1"
        )?.query_row(params![session_id, summary], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
        let Some((boundary, json)) = boundary else {
            return Ok(Some(summary));
        };

        let tail = serde_json::from_str::<serde_json::Value>(&json)
            .ok()
            .and_then(|part| part["tailFrom"].as_str().map(str::to_string));
        Ok(Some(tail.filter(|tail| *tail < boundary).unwrap_or(boundary)))
    }

    /// The id of the session's newest user message.
    pub fn newest_prompt(&self, session_id: &str) -> rusqlite::Result<Option<String>> {
        self.lock()
            .prepare_cached("SELECT id FROM message WHERE session_id = ?1 AND role = 'user' ORDER BY id DESC LIMIT 1")?
            .query_row([session_id], |row| row.get(0))
            .optional()
    }

    /// Nudges the engine sent since the user's newest prompt of their own (one with text or a file).
    pub fn nudges_since_prompt(&self, session_id: &str) -> rusqlite::Result<usize> {
        self.lock().prepare_cached(
            "SELECT COUNT(DISTINCT m.id) FROM message m JOIN part p ON p.message_id = m.id
             WHERE m.session_id = ?1 AND m.role = 'user' AND json_extract(p.json, '$.type') = 'nudge'
             AND m.id > COALESCE((SELECT MAX(u.id) FROM message u JOIN part q ON q.message_id = u.id
                 WHERE u.session_id = ?1 AND u.role = 'user' AND json_extract(q.json, '$.type') IN ('text', 'file')), '')"
        )?.query_row([session_id], |row| row.get(0))
    }

    /// The session's newest assistant message, without loading the rest of the conversation.
    pub fn last_reply(&self, session_id: &str) -> rusqlite::Result<Option<MessageWithParts>> {
        let connection = self.lock();
        let message = connection
            .prepare_cached(&format!("SELECT {MESSAGE_COLUMNS} FROM message WHERE session_id = ?1 AND role = 'assistant' ORDER BY id DESC LIMIT 1"))?
            .query_row([session_id], map_message).optional()?;

        Ok(with_parts_in(self, &connection, session_id, message.into_iter().collect())?.pop())
    }

    /// The user's prompt the turn holding `message_id` answers: the newest user message before it that is not a compaction boundary.
    pub fn prompt_before(&self, session_id: &str, message_id: &str) -> rusqlite::Result<Option<MessageWithParts>> {
        let connection = self.lock();
        let message = connection.prepare_cached(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM message m WHERE session_id = ?1 AND id < ?2 AND role = 'user' AND summary = 0
             AND NOT EXISTS (SELECT 1 FROM part p WHERE p.message_id = m.id AND json_extract(p.json, '$.type') = 'compaction') ORDER BY id DESC LIMIT 1"
        ))?.query_row(params![session_id, message_id], map_message).optional()?;

        Ok(with_parts_in(self, &connection, session_id, message.into_iter().collect())?.pop())
    }

    /// One message and its parts.
    pub fn with_parts(&self, message_id: &str) -> rusqlite::Result<Option<MessageWithParts>> {
        let connection = self.lock();
        let message = connection
            .prepare_cached(&format!("SELECT {MESSAGE_COLUMNS} FROM message WHERE id = ?1"))?
            .query_row([message_id], map_message)
            .optional()?;
        let Some(message) = message else { return Ok(None) };

        let session_id = message.session_id.clone();
        Ok(with_parts_in(self, &connection, &session_id, vec![message])?.pop())
    }

    /// Anything still streaming when the engine last stopped did not finish.
    pub fn abandon_streaming_messages(&self) -> rusqlite::Result<usize> {
        self.lock().execute(
            "UPDATE message SET status = 'aborted', finished_at = ?1 WHERE status = 'streaming'",
            [id::now_ms()],
        )
    }
}

fn map_message(row: &Row) -> rusqlite::Result<Message> {
    let provider: Option<String> = row.get(4)?;
    let model: Option<String> = row.get(5)?;
    let usage: String = row.get(6)?;

    Ok(Message {
        id: row.get(0)?,
        session_id: row.get(1)?,
        role: if row.get::<_, String>(2)? == "user" {
            Role::User
        } else {
            Role::Assistant
        },
        status: parse_status(&row.get::<_, String>(3)?),
        model: provider
            .zip(model)
            .map(|(provider, model)| ModelRef { provider, model }),
        agent: row.get(12)?,
        usage: serde_json::from_str(&usage).unwrap_or_default(),
        cost: row.get(7)?,
        error: row.get(8)?,
        created_at: row.get(9)?,
        finished_at: row.get(10)?,
        generation_ms: row.get(14)?,
        summary: row.get(11)?,
        ending: row
            .get::<_, Option<String>>(13)?
            .as_deref()
            .and_then(crate::session::types::Ending::parse),
    })
}

/// Attaches parts to messages (in id order) with one query over their id range, not one per message.
fn with_parts_in(
    store: &Store,
    connection: &Connection,
    session_id: &str,
    messages: Vec<Message>,
) -> rusqlite::Result<Vec<MessageWithParts>> {
    let (Some(first), Some(last)) = (messages.first(), messages.last()) else {
        return Ok(Vec::new());
    };
    let mut by_message: HashMap<String, Vec<PartRow>> = HashMap::new();
    let mut statement = connection.prepare_cached(
        "SELECT p.id, p.session_id, p.json, p.message_id, p.provider_signature FROM message m JOIN part p ON p.message_id = m.id
         WHERE m.session_id = ?1 AND m.id >= ?2 AND m.id <= ?3 ORDER BY p.message_id, p.id"
    )?;
    let rows = statement.query_map(params![session_id, first.id, last.id], |row| {
        let message_id: String = row.get(3)?;
        Ok((message_id.clone(), map_part(row, &message_id)?))
    })?;

    let streaming = store.streaming.lock().unwrap();
    for row in rows {
        let (message_id, part) = row?;
        let part = streaming.get(&part.id).cloned().unwrap_or(part);
        by_message.entry(message_id).or_default().push(part);
    }

    Ok(messages
        .into_iter()
        .map(|info| MessageWithParts {
            parts: by_message.remove(&info.id).unwrap_or_default(),
            info,
        })
        .collect())
}

pub(super) fn role_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

pub(super) fn status_str(status: MessageStatus) -> &'static str {
    match status {
        MessageStatus::Streaming => "streaming",
        MessageStatus::Done => "done",
        MessageStatus::Aborted => "aborted",
        MessageStatus::Error => "error",
        MessageStatus::Paused => "paused",
    }
}

fn parse_status(status: &str) -> MessageStatus {
    match status {
        "streaming" => MessageStatus::Streaming,
        "aborted" => MessageStatus::Aborted,
        "error" => MessageStatus::Error,
        "paused" => MessageStatus::Paused,
        _ => MessageStatus::Done,
    }
}

fn save_message_in(connection: &Connection, message: &Message) -> rusqlite::Result<()> {
    connection.prepare_cached("UPDATE message SET status = ?2, usage_json = ?3, cost = ?4, error = ?5, finished_at = ?6, ending = ?7, generation_ms = ?8 WHERE id = ?1")?
        .execute(params![message.id, status_str(message.status), serde_json::to_string(&message.usage).unwrap(),
            message.cost, message.error, message.finished_at, message.ending.map(crate::session::types::Ending::as_str), message.generation_ms])?;
    Ok(())
}

/// `agent` defaults to the one the session runs as now.
pub(super) fn insert_message(connection: &Connection, new: NewMessage<'_>) -> rusqlite::Result<Message> {
    let NewMessage {
        session_id,
        role,
        model,
        agent,
        summary,
    } = new;
    let agent = match agent {
        Some(agent) => Some(agent.to_string()),
        None => connection
            .prepare_cached("SELECT agent FROM session WHERE id = ?1")?
            .query_row([session_id], |row| row.get(0))
            .optional()?,
    };
    let message = Message {
        id: id::new("msg"),
        session_id: session_id.into(),
        role,
        status: if role == Role::User {
            MessageStatus::Done
        } else {
            MessageStatus::Streaming
        },
        model: model.cloned(),
        agent,
        usage: Usage::default(),
        cost: 0.0,
        error: None,
        created_at: id::now_ms(),
        finished_at: None,
        generation_ms: None,
        summary,
        ending: None,
    };

    connection.prepare_cached(
        "INSERT INTO message(id, session_id, role, status, model_provider, model_id, usage_json, cost, created_at, summary, agent)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9, ?10)"
    )?.execute(params![message.id, message.session_id, role_str(role), status_str(message.status),
        model.map(|model| &model.provider), model.map(|model| &model.model), serde_json::to_string(&message.usage).unwrap(),
        message.created_at, summary, message.agent])?;

    Ok(message)
}
