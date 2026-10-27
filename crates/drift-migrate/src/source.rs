//! Reads opencode's database read-only, from a consistent snapshot and in bounded pages.
//! No conversation is held whole in memory, regardless of its length.

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Value, json};
use std::path::Path;

/// Tool parts beyond this byte limit are streamed and reduced to their name, input and output prefix.
/// Other part kinds are read whole; oversized tool display copies are skipped. A v2 row past it is read whole
/// and its calls reduced the same way.
const OVERSIZED_PART_BYTES: i64 = 8_000_000;
/// Maximum bytes retained from an oversized call's input or output.
pub(crate) const KEPT_BYTES: i64 = 64 * 1024;

pub(crate) struct Source {
    conn: Connection,
    tables: std::collections::HashSet<String>,
}

pub(crate) struct OcSession {
    pub id: String,
    pub parent_id: Option<String>,
    pub directory: String,
    /// The repository root opencode filed it under; the conversation may have run in a directory inside it.
    pub worktree: Option<String>,
    pub title: String,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub created: i64,
    pub updated: i64,
    pub archived: bool,
    /// Its messages are opencode v2's `session_message` rows rather than v1's messages and parts.
    pub v2: bool,
}

pub(crate) struct OcMessage {
    pub id: String,
    pub created: i64,
    /// A v2 row's place in its conversation; v1 messages are ordered by creation time and id instead.
    pub seq: i64,
    pub data: String,
}

/// opencode v1 keeps conversations in `session`, v2 in `session_v2`; a v1 conversation v2 copied keeps its id in both.
const V1_SESSIONS: &str = "SELECT s.id, s.parent_id, s.directory, coalesce(s.title, ''), s.agent, s.model,
        s.time_created, s.time_updated, s.time_archived IS NOT NULL, p.worktree, {continued}
    FROM session s LEFT JOIN project p ON p.id = s.project_id";
const V2_SESSIONS: &str = "SELECT s.id, s.parent_id, s.directory, coalesce(s.title, ''), s.agent, s.model,
        s.time_created, s.time_updated, s.time_archived IS NOT NULL, p.worktree, 1
    FROM session_v2 s LEFT JOIN project p ON p.id = s.project_id";
/// A v1 conversation continued in v2 has v2 rows newer than its last v1 message; v2's copy of the history keeps its times.
const CONTINUED: &str = "EXISTS (SELECT 1 FROM session_message m WHERE m.session_id = s.id
    AND m.time_created > coalesce((SELECT max(time_created) FROM message WHERE session_id = s.id), -1))";

fn session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OcSession> {
    Ok(OcSession {
        id: row.get(0)?,
        parent_id: row.get(1)?,
        directory: row.get(2)?,
        title: row.get(3)?,
        agent: row.get(4)?,
        model: row.get(5)?,
        created: row.get(6)?,
        updated: row.get(7)?,
        archived: row.get(8)?,
        worktree: row.get(9)?,
        v2: row.get(10)?,
    })
}

pub(crate) struct OcPart {
    pub id: String,
    pub data: String,
}

pub(crate) struct OcTodo {
    pub content: String,
    pub status: String,
    pub priority: String,
}

/// Fields retained from an oversized tool call.
/// The streaming reader skips all other fields, including display copies, without storing them.
#[derive(serde::Deserialize)]
struct BigCall {
    #[serde(rename = "type")]
    kind: Option<String>,
    tool: Option<String>,
    #[serde(rename = "callID")]
    call_id: Option<String>,
    #[serde(default)]
    state: BigState,
}

#[derive(Default, serde::Deserialize)]
struct BigState {
    status: Option<String>,
    title: Option<String>,
    input: Option<Value>,
    output: Option<String>,
    error: Option<String>,
    time: Option<Value>,
}

impl BigCall {
    fn kept(self) -> Value {
        let input = self
            .state
            .input
            .filter(|input| input.to_string().len() <= KEPT_BYTES as usize);

        json!({ "type": "tool", "tool": self.tool, "callID": self.call_id, "state": {
            "status": self.state.status, "title": self.state.title, "time": self.state.time,
            "input": input, "output": truncate(self.state.output), "error": truncate(self.state.error),
        } })
    }
}

pub(crate) fn truncate(text: Option<String>) -> Option<String> {
    text.map(|mut text| {
        let mut boundary = (KEPT_BYTES as usize).min(text.len());
        while !text.is_char_boundary(boundary) {
            boundary -= 1;
        }

        text.truncate(boundary);
        text
    })
}

fn message_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OcMessage> {
    Ok(OcMessage {
        id: row.get(0)?,
        created: row.get(1)?,
        seq: row.get(2)?,
        data: row.get(3)?,
    })
}

impl Source {
    /// Opens read-only and holds one read transaction, keeping all rows consistent while opencode writes.
    pub(crate) fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn =
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;

        conn.execute_batch("BEGIN")?;
        let tables = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;

        Ok(Self { conn, tables })
    }

    fn has(&self, table: &str) -> bool {
        self.tables.contains(table)
    }

    /// Returns all conversations, most recently used first so the sidebar fills from the top.
    /// Subagents follow the conversations that could have started them.
    /// A conversation in both shapes is read from v1, which keeps more, unless it went on in v2.
    pub(crate) fn sessions(&self) -> rusqlite::Result<Vec<OcSession>> {
        let mut found: std::collections::HashMap<String, OcSession> = std::collections::HashMap::new();
        if self.has("session_v2") {
            for session in self.query_sessions(V2_SESSIONS)? {
                found.insert(session.id.clone(), session);
            }
        }
        if self.has("session") {
            let continued = if self.has("session_message") { CONTINUED } else { "0" };
            for session in self.query_sessions(&V1_SESSIONS.replace("{continued}", continued))? {
                if !(session.v2 && found.contains_key(&session.id)) {
                    found.insert(session.id.clone(), session);
                }
            }
        }

        let mut sessions: Vec<OcSession> = found.into_values().collect();
        sessions.sort_by(|left, right| {
            (left.parent_id.is_some(), std::cmp::Reverse(left.updated), &left.id).cmp(&(
                right.parent_id.is_some(),
                std::cmp::Reverse(right.updated),
                &right.id,
            ))
        });

        Ok(sessions)
    }

    fn query_sessions(&self, sql: &str) -> rusqlite::Result<Vec<OcSession>> {
        self.conn.prepare(sql)?.query_map([], session_row)?.collect()
    }

    /// Returns up to `limit` messages after `after`, in written order: v1 by creation time and id, v2 by sequence.
    /// Per-file diff summaries on v1 user messages are left in the database rather than read; v2 rows come without their data.
    pub(crate) fn messages_after(
        &self,
        session: &OcSession,
        after: Option<&OcMessage>,
        limit: usize,
    ) -> rusqlite::Result<Vec<OcMessage>> {
        if session.v2 {
            let seq = after.map_or(i64::MIN, |message| message.seq);
            let mut statement = self.conn.prepare_cached(
                "SELECT id, time_created, seq, '' FROM session_message
                 WHERE session_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
            )?;
            return statement
                .query_map(params![session.id, seq, limit as i64], message_row)?
                .collect();
        }

        let (created, id) = after.map_or((i64::MIN, ""), |message| (message.created, message.id.as_str()));
        let mut statement = self.conn.prepare_cached(
            "SELECT id, time_created, 0, json_remove(data, '$.summary.diffs') FROM message
             WHERE session_id = ?1 AND (time_created > ?2 OR (time_created = ?2 AND id > ?3))
             ORDER BY time_created, id LIMIT ?4",
        )?;
        let rows = statement.query_map(params![session.id, created, id, limit as i64], message_row)?;

        rows.collect()
    }

    /// A v2 row's data with its type, and whether it is past [`OVERSIZED_PART_BYTES`]; rows are read one at a time.
    pub(crate) fn v2_row(&self, id: &str) -> rusqlite::Result<(String, bool)> {
        self.conn
            .prepare_cached(
                "SELECT json_set(data, '$.type', type), octet_length(data) > ?2 FROM session_message WHERE id = ?1",
            )?
            .query_row(params![id, OVERSIZED_PART_BYTES], |row| Ok((row.get(0)?, row.get(1)?)))
    }

    /// Returns the conversation's newest `limit` messages, newest first.
    pub(crate) fn newest_messages(&self, session_id: &str, limit: usize) -> rusqlite::Result<Vec<OcMessage>> {
        let mut statement = self.conn.prepare_cached(
            "SELECT id, time_created, 0, '' FROM message WHERE session_id = ?1
             ORDER BY time_created DESC, id DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![session_id, limit as i64], message_row)?;

        rows.collect()
    }

    /// Returns a message's parts in order, reducing tool calls larger than `OVERSIZED_PART_BYTES`.
    /// Oversized tool calls are streamed from disk rather than held whole in memory.
    pub(crate) fn parts(&self, message_id: &str) -> rusqlite::Result<Vec<OcPart>> {
        let mut statement = self.conn.prepare_cached(
            "SELECT id, rowid, CASE WHEN octet_length(data) <= ?2 THEN data END FROM part
             WHERE message_id = ?1 ORDER BY id",
        )?;
        let rows: Vec<(String, i64, Option<String>)> = statement
            .query_map(params![message_id, OVERSIZED_PART_BYTES], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?;

        rows.into_iter()
            .map(|(id, rowid, data)| {
                let data = match data {
                    Some(data) => data,
                    None => self.oversized(rowid)?,
                };
                Ok(OcPart { id, data })
            })
            .collect()
    }

    /// Reduces a tool call to its retained fields; any other oversized part is read whole.
    fn oversized(&self, rowid: i64) -> rusqlite::Result<String> {
        let stream = self.conn.blob_open("main", "part", "data", rowid, true)?;

        match serde_json::from_reader::<_, BigCall>(std::io::BufReader::new(stream)) {
            Ok(call) if call.kind.as_deref() == Some("tool") => Ok(call.kept().to_string()),
            _ => self
                .conn
                .prepare_cached("SELECT data FROM part WHERE rowid = ?1")?
                .query_row([rowid], |row| row.get(0)),
        }
    }

    /// A part's stored text when it is within [`OVERSIZED_PART_BYTES`].
    pub(crate) fn small_part(&self, id: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .prepare_cached("SELECT data FROM part WHERE id = ?1 AND octet_length(data) <= ?2")?
            .query_row(params![id, OVERSIZED_PART_BYTES], |row| row.get(0))
            .optional()
    }

    /// Returns a message's part IDs without reading their text.
    pub(crate) fn part_ids(&self, message_id: &str) -> rusqlite::Result<Vec<String>> {
        self.conn
            .prepare_cached("SELECT id FROM part WHERE message_id = ?1 ORDER BY id")?
            .query_map([message_id], |row| row.get(0))?
            .collect()
    }

    /// Returns conversations containing queued prompts that opencode never ran: v1's `session_input`, v2's `session_inbox`.
    /// Databases without either table have no pending prompts.
    pub(crate) fn pending_inputs(&self) -> rusqlite::Result<std::collections::HashSet<String>> {
        let queued = |table: &str, sql: &str| -> rusqlite::Result<std::collections::HashSet<String>> {
            if !self.has(table) {
                return Ok(Default::default());
            }
            self.conn.prepare(sql)?.query_map([], |row| row.get(0))?.collect()
        };

        let mut ids = queued(
            "session_input",
            "SELECT DISTINCT session_id FROM session_input WHERE promoted_seq IS NULL",
        )?;
        ids.extend(queued(
            "session_inbox",
            "SELECT DISTINCT session_id FROM session_inbox WHERE type = 'user'",
        )?);
        Ok(ids)
    }

    pub(crate) fn todos(&self, session_id: &str) -> rusqlite::Result<Vec<OcTodo>> {
        self.conn
            .prepare_cached("SELECT content, status, priority FROM todo WHERE session_id = ?1 ORDER BY position")?
            .query_map([session_id], |row| {
                Ok(OcTodo {
                    content: row.get(0)?,
                    status: row.get(1)?,
                    priority: row.get(2)?,
                })
            })?
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_text_is_cut_at_a_utf8_boundary_and_missing_text_stays_missing() {
        let prefix = "x".repeat(KEPT_BYTES as usize - 1);
        let text = format!("{prefix}é");

        assert_eq!(truncate(Some(text)), Some(prefix));
        assert_eq!(truncate(None), None);
        assert_eq!(truncate(Some("short".into())), Some("short".into()));
    }
}
