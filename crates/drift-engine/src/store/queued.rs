//! Prompts waiting for a running turn to hand over: one row per submission, oldest first.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;

use super::sessions::transaction;
use super::Store;
use crate::id;
use crate::session::types::{Part, Queued, Session};

/// One waiting submission, its prompt as the client sent it.
#[derive(Clone, Debug, PartialEq)]
pub struct QueuedRow {
    pub submission_id: String,
    pub payload_hash: String,
    pub prompt_json: String,
    pub error: Option<String>,
    pub created_at: i64,
}

/// The parts of a stored prompt the view reads.
#[derive(Deserialize)]
struct Stored {
    #[serde(default)]
    parts: Vec<Part>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default, deserialize_with = "crate::session::turn::present")]
    variant: Option<Option<String>>,
}

impl Store {
    pub fn queued(&self, session_id: &str) -> rusqlite::Result<Vec<QueuedRow>> {
        rows_in(&self.lock(), session_id)
    }

    /// The session and payload a submission waits under, if it is waiting.
    pub fn queued_submission(&self, id: &str) -> rusqlite::Result<Option<(String, String)>> {
        self.lock()
            .prepare_cached("SELECT session_id, payload_hash FROM queued_prompt WHERE submission_id = ?1")?
            .query_row([id], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()
    }

    /// Adds `row` behind what waits, or in place of it with `replace`; returns what it replaced. Either way it is tried afresh.
    pub fn queue(&self, session_id: &str, row: &QueuedRow, replace: bool) -> rusqlite::Result<Vec<QueuedRow>> {
        transaction(&self.lock(), |conn| {
            let replaced = if replace { take_in(conn, session_id)? } else { Vec::new() };
            conn.prepare_cached("UPDATE queued_prompt SET error = NULL WHERE session_id = ?1")?.execute([session_id])?;
            conn.prepare_cached("INSERT INTO queued_prompt(submission_id, session_id, payload_hash, prompt_json, created_at) VALUES(?1, ?2, ?3, ?4, ?5)")?
                .execute(params![row.submission_id, session_id, row.payload_hash, row.prompt_json, id::now_ms()])?;
            Ok(replaced)
        })
    }

    /// Removes and returns everything waiting in the session.
    pub fn take_queued(&self, session_id: &str) -> rusqlite::Result<Vec<QueuedRow>> {
        transaction(&self.lock(), |conn| take_in(conn, session_id))
    }

    /// Marks what waits as unable to start, so nothing tries it again until the user adds to it.
    pub fn fail_queued(&self, session_id: &str, error: &str) -> rusqlite::Result<()> {
        self.lock().prepare_cached("UPDATE queued_prompt SET error = ?2 WHERE session_id = ?1")?.execute(params![session_id, error])?;
        Ok(())
    }

    /// Whether something waits that can still start; a running turn hands over at its next step while it does.
    pub fn is_waiting(&self, session_id: &str) -> rusqlite::Result<bool> {
        self.lock().prepare_cached("SELECT 1 FROM queued_prompt WHERE session_id = ?1 AND error IS NULL")?.exists([session_id])
    }

    /// Sessions with something waiting that can still start, as after a restart.
    pub fn waiting_sessions(&self) -> rusqlite::Result<Vec<String>> {
        let conn = self.lock();
        let mut stmt = conn.prepare_cached("SELECT DISTINCT session_id FROM queued_prompt WHERE error IS NULL")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    }
}

fn rows_in(conn: &Connection, session_id: &str) -> rusqlite::Result<Vec<QueuedRow>> {
    let mut stmt = conn.prepare_cached("SELECT submission_id, payload_hash, prompt_json, error, created_at FROM queued_prompt WHERE session_id = ?1 ORDER BY rowid")?;
    let rows = stmt.query_map([session_id], |row| {
        Ok(QueuedRow { submission_id: row.get(0)?, payload_hash: row.get(1)?, prompt_json: row.get(2)?, error: row.get(3)?, created_at: row.get(4)? })
    })?;
    rows.collect()
}

fn take_in(conn: &Connection, session_id: &str) -> rusqlite::Result<Vec<QueuedRow>> {
    let rows = rows_in(conn, session_id)?;
    conn.prepare_cached("DELETE FROM queued_prompt WHERE session_id = ?1")?.execute([session_id])?;
    Ok(rows)
}

/// A submission admitted as a message no longer waits; called inside its admission's write.
pub(super) fn admitted(conn: &Connection, submission_id: &str) -> rusqlite::Result<()> {
    conn.prepare_cached("DELETE FROM queued_prompt WHERE submission_id = ?1")?.execute([submission_id])?;
    Ok(())
}

/// The session with what waits in it, as every read of a session shows it.
pub(super) fn with_queue(conn: &Connection, mut session: Session) -> rusqlite::Result<Session> {
    let rows = rows_in(conn, &session.id)?;
    session.queued = view(&session, &rows);
    Ok(session)
}

fn view(session: &Session, rows: &[QueuedRow]) -> Option<Queued> {
    let stored: Vec<Stored> = rows.iter().filter_map(|row| serde_json::from_str(&row.prompt_json).ok()).collect();
    let first = stored.first()?;
    let parts = || stored.iter().flat_map(|prompt| &prompt.parts);
    let texts: Vec<&str> = parts().filter_map(|part| if let Part::Text { text } = part { Some(text.as_str()) } else { None }).collect();
    Some(Queued {
        agent: first.agent.clone().unwrap_or_else(|| session.agent.clone()),
        variant: first.variant.clone().unwrap_or_else(|| session.variant.clone()),
        text: texts.join("\n\n"),
        files: parts().filter(|part| matches!(part, Part::File { .. })).count(),
        error: rows.iter().find_map(|row| row.error.clone()),
        since: rows.first()?.created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::types::{ModelRef, Visibility};
    use crate::store::tests::store;
    use crate::store::{Admit, Handover, NewSession, Pick};

    fn row(id: &str, json: &str) -> QueuedRow {
        QueuedRow { submission_id: id.into(), payload_hash: format!("h-{id}"), prompt_json: json.into(), error: None, created_at: 0 }
    }

    #[test]
    fn what_waits_shows_on_the_session_and_leaves_with_its_admission() {
        let store = store();
        let session = store.create_session(NewSession { workspace_id: "w", parent_id: None, visibility: Visibility::Sibling, title: "", agent: "build", model: None }).unwrap();
        assert_eq!(store.session(&session.id).unwrap().unwrap().queued, None);
        store.queue(&session.id, &row("a", r#"{"parts":[{"type":"text","text":"plan it"}],"agent":"plan","variant":null}"#), false).unwrap();
        store.queue(&session.id, &row("b", r#"{"parts":[{"type":"text","text":"and this"}]}"#), false).unwrap();
        let queued = store.session(&session.id).unwrap().unwrap().queued.unwrap();
        assert_eq!((queued.agent.as_str(), queued.variant.as_deref(), queued.text.as_str()), ("plan", None, "plan it\n\nand this"));
        assert_eq!(store.queued_submission("b").unwrap(), Some((session.id.clone(), "h-b".into())));
        store.fail_queued(&session.id, "no credentials").unwrap();
        assert!(!store.is_waiting(&session.id).unwrap() && store.waiting_sessions().unwrap().is_empty(), "a failed queue does not hold a turn");
        let replaced = store.queue(&session.id, &row("c", r#"{"parts":[{"type":"text","text":"instead"}],"agent":"build"}"#), true).unwrap();
        assert_eq!(replaced.iter().map(|r| r.submission_id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert!(store.is_waiting(&session.id).unwrap(), "adding tries again");
        let model = ModelRef { provider: "p".into(), model: "m".into() };
        let Admit::New(admitted) = store.admit_delivering(&session.id, Pick::model(&model), vec![], &[("c", "h-c")], Handover::default()).unwrap() else { panic!() };
        assert_eq!(admitted.session.queued, None, "admitted in the same write that ends its wait");
        assert!(store.queued(&session.id).unwrap().is_empty());
    }
}
