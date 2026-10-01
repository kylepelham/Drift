use rusqlite::{params, Connection, OptionalExtension, Row};

use super::Store;
use crate::id;
use crate::session::types::{
    Message, MessageStatus, MessageWithParts, ModelRef, Part, PartRow, Revert, Role, Session, Usage,
    Visibility,
};

pub(super) const SESSION_COLUMNS: &str = "id, workspace_id, parent_id, visibility, title, agent, model_provider, model_id, created_at, updated_at, archived_at, branch_cutoff, revert_json";
const MESSAGE_COLUMNS: &str = "id, session_id, role, status, model_provider, model_id, usage_json, cost, error, created_at, finished_at, summary";

pub struct NewSession<'a> {
    pub workspace_id: &'a str,
    pub parent_id: Option<&'a str>,
    pub visibility: Visibility,
    pub title: &'a str,
    pub agent: &'a str,
    pub model: Option<&'a ModelRef>,
}

pub struct SessionFilter<'a> {
    pub workspace_id: Option<&'a str>,
    pub archived: bool,
    pub before: Option<&'a str>,
    pub limit: usize,
}

impl Store {
    pub fn create_session(&self, new: NewSession) -> rusqlite::Result<Session> {
        self.create_branch(new, None)
    }

    /// A session that records which source message its handoff was cut from.
    pub fn create_branch(&self, new: NewSession, cutoff: Option<&str>) -> rusqlite::Result<Session> {
        let session = session_from(new, cutoff);
        insert_session(&self.lock(), &session)?;
        Ok(session)
    }

    pub fn session(&self, id: &str) -> rusqlite::Result<Option<Session>> {
        self.lock()
            .prepare_cached(&format!("SELECT {SESSION_COLUMNS} FROM session WHERE id = ?1"))?
            .query_row([id], map_session)
            .optional()
    }

    /// Listed newest first, subagents included; the UI nests them under their parent.
    pub fn sessions(&self, filter: SessionFilter) -> rusqlite::Result<Vec<Session>> {
        let conn = self.lock();
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {SESSION_COLUMNS} FROM session
             WHERE (?1 IS NULL OR workspace_id = ?1)
               AND (archived_at IS NOT NULL) = ?2
               AND (?3 IS NULL OR (updated_at, id) < (SELECT updated_at, id FROM session WHERE id = ?3))
             ORDER BY updated_at DESC, id DESC LIMIT ?4"
        ))?;
        let rows = stmt.query_map(
            params![filter.workspace_id, filter.archived, filter.before, filter.limit as i64],
            map_session,
        )?;
        rows.collect()
    }

    pub fn update_session(&self, id: &str, title: Option<&str>, model: Option<&ModelRef>, agent: Option<&str>) -> rusqlite::Result<Option<Session>> {
        let conn = self.lock();
        conn.prepare_cached(
            "UPDATE session SET title = COALESCE(?2, title),
                model_provider = COALESCE(?3, model_provider), model_id = COALESCE(?4, model_id),
                agent = COALESCE(?6, agent),
                updated_at = ?5 WHERE id = ?1",
        )?
        .execute(params![id, title, model.map(|m| &m.provider), model.map(|m| &m.model), id::now_ms(), agent])?;
        session_in(&conn, id)
    }

    /// Retitles only while the title is still `expected`, so a concurrent rename wins. `None` if it changed.
    pub fn retitle_if(&self, id: &str, expected: &str, title: &str) -> rusqlite::Result<Option<Session>> {
        let conn = self.lock();
        let changed = conn
            .prepare_cached("UPDATE session SET title = ?3, updated_at = ?4 WHERE id = ?1 AND title = ?2")?
            .execute(params![id, expected, title, id::now_ms()])?;
        if changed == 0 {
            return Ok(None);
        }
        session_in(&conn, id)
    }

    pub fn touch_session(&self, id: &str) -> rusqlite::Result<()> {
        self.lock()
            .prepare_cached("UPDATE session SET updated_at = ?2 WHERE id = ?1")?
            .execute(params![id, id::now_ms()])?;
        Ok(())
    }

    pub fn set_session_archived(&self, id: &str, archived: bool) -> rusqlite::Result<Option<Session>> {
        let conn = self.lock();
        let at = archived.then(id::now_ms);
        conn.prepare_cached("UPDATE session SET archived_at = ?2 WHERE id = ?1")?
            .execute(params![id, at])?;
        session_in(&conn, id)
    }

    pub fn create_message(&self, session_id: &str, role: Role, model: Option<&ModelRef>) -> rusqlite::Result<Message> {
        insert_message(&self.lock(), session_id, role, model, false)
    }

    /// The streaming assistant message a compaction writes its summary into.
    pub fn create_summary_message(&self, session_id: &str, model: &ModelRef) -> rusqlite::Result<Message> {
        insert_message(&self.lock(), session_id, Role::Assistant, Some(model), true)
    }

    pub fn save_message(&self, message: &Message) -> rusqlite::Result<()> {
        save_message_in(&self.lock(), message)
    }

    /// Stores a summary's text and its finished state as one write: a summary is never done without its text.
    pub fn complete_summary(&self, message: &Message, text: &str) -> rusqlite::Result<PartRow> {
        transaction(&self.lock(), |conn| {
            let row = insert_part(conn, &message.id, &message.session_id, Part::Text { text: text.into() })?;
            save_message_in(conn, message)?;
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
    pub fn messages(&self, session_id: &str, before: Option<&str>, limit: usize) -> rusqlite::Result<Vec<MessageWithParts>> {
        let conn = self.lock();
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM (
                SELECT * FROM message WHERE session_id = ?1 AND (?2 IS NULL OR id < ?2)
                ORDER BY id DESC LIMIT ?3
             ) ORDER BY id ASC"
        ))?;
        let infos: Vec<Message> = stmt
            .query_map(params![session_id, before, limit as i64], map_message)?
            .collect::<Result<_, _>>()?;
        let mut parts = conn.prepare_cached("SELECT id, session_id, json FROM part WHERE message_id = ?1 ORDER BY id")?;
        infos
            .into_iter()
            .map(|info| {
                let parts = parts
                    .query_map([&info.id], |row| map_part(row, &info.id))?
                    .collect::<Result<_, _>>()?;
                Ok(MessageWithParts { info, parts })
            })
            .collect()
    }

    /// All messages of a session in order: what a turn sends back to the model.
    pub fn transcript(&self, session_id: &str) -> rusqlite::Result<Vec<MessageWithParts>> {
        self.messages(session_id, None, usize::MAX / 2)
    }

    pub fn add_part(&self, message_id: &str, session_id: &str, part: Part) -> rusqlite::Result<PartRow> {
        insert_part(&self.lock(), message_id, session_id, part)
    }

    pub fn save_part(&self, row: &PartRow) -> rusqlite::Result<()> {
        save_part_in(&self.lock(), row)
    }

    /// Anything still streaming when the engine last stopped did not finish.
    pub fn abandon_streaming_messages(&self) -> rusqlite::Result<usize> {
        self.lock().execute(
            "UPDATE message SET status = 'aborted', finished_at = ?1 WHERE status = 'streaming'",
            [id::now_ms()],
        )
    }
}

pub(super) fn save_part_in(conn: &Connection, row: &PartRow) -> rusqlite::Result<()> {
    conn.prepare_cached("UPDATE part SET json = ?2 WHERE id = ?1")?.execute(params![row.id, serde_json::to_string(&row.part).unwrap()])?;
    Ok(())
}

pub(super) fn session_in(conn: &Connection, id: &str) -> rusqlite::Result<Option<Session>> {
    conn.prepare_cached(&format!("SELECT {SESSION_COLUMNS} FROM session WHERE id = ?1"))?
        .query_row([id], map_session)
        .optional()
}

pub(super) fn map_session(row: &Row) -> rusqlite::Result<Session> {
    let provider: Option<String> = row.get(6)?;
    let model: Option<String> = row.get(7)?;
    Ok(Session {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        parent_id: row.get(2)?,
        visibility: match row.get::<_, String>(3)?.as_str() {
            "hidden" => Visibility::Hidden,
            _ => Visibility::Sibling,
        },
        title: row.get(4)?,
        agent: row.get(5)?,
        model: provider.zip(model).map(|(provider, model)| ModelRef { provider, model }),
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        archived_at: row.get(10)?,
        branch_cutoff: row.get(11)?,
        revert: row.get::<_, Option<String>>(12)?.and_then(|json| serde_json::from_str(&json).ok()),
        running: false,
    })
}

fn map_message(row: &Row) -> rusqlite::Result<Message> {
    let provider: Option<String> = row.get(4)?;
    let model: Option<String> = row.get(5)?;
    let usage: String = row.get(6)?;
    Ok(Message {
        id: row.get(0)?,
        session_id: row.get(1)?,
        role: if row.get::<_, String>(2)? == "user" { Role::User } else { Role::Assistant },
        status: parse_status(&row.get::<_, String>(3)?),
        model: provider.zip(model).map(|(provider, model)| ModelRef { provider, model }),
        usage: serde_json::from_str(&usage).unwrap_or_default(),
        cost: row.get(7)?,
        error: row.get(8)?,
        created_at: row.get(9)?,
        finished_at: row.get(10)?,
        summary: row.get(11)?,
    })
}

fn map_part(row: &Row, message_id: &str) -> rusqlite::Result<PartRow> {
    let json: String = row.get(2)?;
    let part = serde_json::from_str(&json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(PartRow {
        id: row.get(0)?,
        message_id: message_id.into(),
        session_id: row.get(1)?,
        part,
    })
}

fn visibility_str(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Hidden => "hidden",
        Visibility::Sibling => "sibling",
    }
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

fn status_str(status: MessageStatus) -> &'static str {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::store;

    fn new(workspace: &str) -> NewSession<'_> {
        NewSession {
            workspace_id: workspace,
            parent_id: None,
            visibility: Visibility::Sibling,
            title: "",
            agent: "build",
            model: None,
        }
    }

    #[test]
    fn create_list_update_archive() {
        let store = store();
        let a = store.create_session(new("w1")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = store.create_session(new("w1")).unwrap();
        store.create_session(new("w2")).unwrap();
        let listed = store
            .sessions(SessionFilter { workspace_id: Some("w1"), archived: false, before: None, limit: 10 })
            .unwrap();
        assert_eq!(listed.iter().map(|s| &s.id).collect::<Vec<_>>(), [&b.id, &a.id]);

        let model = ModelRef { provider: "anthropic".into(), model: "claude".into() };
        let updated = store.update_session(&a.id, Some("Title"), Some(&model), Some("plan")).unwrap().unwrap();
        assert_eq!(updated.title, "Title");
        assert_eq!(updated.model, Some(model));
        assert_eq!(updated.agent, "plan");

        store.set_session_archived(&b.id, true).unwrap();
        let active = store
            .sessions(SessionFilter { workspace_id: Some("w1"), archived: false, before: None, limit: 10 })
            .unwrap();
        assert_eq!(active.len(), 1);
        let archived = store
            .sessions(SessionFilter { workspace_id: None, archived: true, before: None, limit: 10 })
            .unwrap();
        assert_eq!(archived[0].id, b.id);
    }

    #[test]
    fn subagents_are_listed_with_their_parent() {
        let store = store();
        let parent = store.create_session(new("w")).unwrap();
        let child = store
            .create_session(NewSession { parent_id: Some(&parent.id), visibility: Visibility::Hidden, ..new("w") })
            .unwrap();
        let listed = store
            .sessions(SessionFilter { workspace_id: Some("w"), archived: false, before: None, limit: 10 })
            .unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().any(|s| s.id == child.id && s.parent_id.as_deref() == Some(parent.id.as_str())));
    }

    #[test]
    fn messages_page_backwards_with_parts() {
        let store = store();
        let session = store.create_session(new("w")).unwrap();
        let mut ids = Vec::new();
        for i in 0..5 {
            let message = store.create_message(&session.id, Role::User, None).unwrap();
            store.add_part(&message.id, &session.id, Part::Text { text: format!("m{i}") }).unwrap();
            ids.push(message.id);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let page = store.messages(&session.id, None, 2).unwrap();
        assert_eq!(page.iter().map(|m| &m.info.id).collect::<Vec<_>>(), [&ids[3], &ids[4]]);
        let older = store.messages(&session.id, Some(&ids[3]), 2).unwrap();
        assert_eq!(older.iter().map(|m| &m.info.id).collect::<Vec<_>>(), [&ids[1], &ids[2]]);
        assert_eq!(older[0].parts[0].part, Part::Text { text: "m1".into() });
        assert_eq!(store.transcript(&session.id).unwrap().len(), 5);
    }

    #[test]
    fn assistant_messages_stream_then_save() {
        let store = store();
        let session = store.create_session(new("w")).unwrap();
        let mut message = store.create_message(&session.id, Role::Assistant, None).unwrap();
        assert_eq!(message.status, MessageStatus::Streaming);
        message.status = MessageStatus::Done;
        message.usage = Usage { input: 10, output: 5, ..Usage::default() };
        message.finished_at = Some(1);
        store.save_message(&message).unwrap();
        assert_eq!(store.message(&message.id).unwrap().unwrap(), message);
    }

    #[test]
    fn streaming_messages_are_abandoned_on_open() {
        let store = store();
        let session = store.create_session(new("w")).unwrap();
        store.create_message(&session.id, Role::Assistant, None).unwrap();
        assert_eq!(store.abandon_streaming_messages().unwrap(), 1);
        assert_eq!(store.abandon_streaming_messages().unwrap(), 0);
    }

    #[test]
    fn parts_round_trip_through_json() {
        let store = store();
        let session = store.create_session(new("w")).unwrap();
        let message = store.create_message(&session.id, Role::Assistant, None).unwrap();
        let mut row = store
            .add_part(&message.id, &session.id, Part::Reasoning { text: "hm".into(), signature: Some("sig".into()), redacted: None })
            .unwrap();
        row.part = Part::Reasoning { text: "hmm".into(), signature: Some("sig".into()), redacted: None };
        store.save_part(&row).unwrap();
        let loaded = store.transcript(&session.id).unwrap();
        assert_eq!(loaded[0].parts, vec![row]);
    }
}

impl Store {
    /// Records a user prompt as one unit: message, parts and the session's model, or nothing at all.
    /// A prompt sent while undone commits the undo: the hidden messages go, in the same write.
    pub fn admit_prompt(&self, session_id: &str, model: &ModelRef, parts: Vec<Part>, submission: Option<(&str, &str)>) -> rusqlite::Result<Admitted> {
        self.admit_delivering(session_id, model, parts, submission, Handover::Nothing)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    /// [`Self::admit_prompt`] that also hands worker results over in the same write; `None` if a delivery already landed.
    pub fn admit_delivering(&self, session_id: &str, model: &ModelRef, parts: Vec<Part>, submission: Option<(&str, &str)>, handover: Handover) -> rusqlite::Result<Option<Admitted>> {
        let conn = self.lock();
        let delivery = matches!(handover, Handover::Delivery(_));
        let admitted = transaction(&conn, |conn| {
            let carried = match handover {
                Handover::Nothing => Vec::new(),
                Handover::Delivery(task_id) if super::tasks::acknowledge(conn, task_id, session_id)? => Vec::new(),
                Handover::Delivery(_) => return Err(rusqlite::Error::StatementChangedRows(0)),
                Handover::Held(held) => held_parts(conn, session_id, held)?,
            };
            admit_in(conn, session_id, model, carried.into_iter().chain(parts).collect(), submission)
        });
        match admitted {
            Err(rusqlite::Error::StatementChangedRows(0)) if delivery => Ok(None),
            other => other.map(Some),
        }
    }
}

/// Worker results a prompt carries into its session.
pub enum Handover<'a> {
    Nothing,
    /// The prompt is this result's delivery; it lands only if the result is still owed.
    Delivery(&'a str),
    /// Held results that ride along with a prompt; each goes in only if still owed.
    Held(Vec<(String, Part)>),
}

fn held_parts(conn: &Connection, session_id: &str, held: Vec<(String, Part)>) -> rusqlite::Result<Vec<Part>> {
    let mut carried = Vec::new();
    for (task_id, part) in held {
        if super::tasks::acknowledge(conn, &task_id, session_id)? {
            carried.push(part);
        }
    }
    Ok(carried)
}

fn admit_in(conn: &Connection, session_id: &str, model: &ModelRef, parts: Vec<Part>, submission: Option<(&str, &str)>) -> rusqlite::Result<Admitted> {
    let discarded = discard_reverted(conn, session_id)?;
    let message = insert_message(conn, session_id, Role::User, Some(model), false)?;
    if let Some((id, hash)) = submission {
        conn.prepare_cached("INSERT INTO submission(id, session_id, message_id, payload_hash, created_at) VALUES(?1, ?2, ?3, ?4, ?5)")?
            .execute(params![id, session_id, message.id, hash, id::now_ms()])?;
    }
    let rows = parts.into_iter().map(|part| insert_part(conn, &message.id, session_id, part)).collect::<rusqlite::Result<Vec<_>>>()?;
    conn.prepare_cached("UPDATE session SET model_provider = ?2, model_id = ?3, updated_at = ?4 WHERE id = ?1")?
        .execute(params![session_id, model.provider, model.model, id::now_ms()])?;
    let session = session_in(conn, session_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    Ok(Admitted { message, parts: rows, session, discarded })
}

impl Store {
    /// Sets or clears the session's undo marker.
    pub fn set_revert(&self, session_id: &str, revert: Option<&Revert>) -> rusqlite::Result<Option<Session>> {
        let conn = self.lock();
        conn.prepare_cached("UPDATE session SET revert_json = ?2, updated_at = ?3 WHERE id = ?1")?
            .execute(params![session_id, revert.map(|r| serde_json::to_string(r).unwrap()), id::now_ms()])?;
        session_in(&conn, session_id)
    }
}

pub struct Admitted {
    pub message: Message,
    pub parts: Vec<PartRow>,
    pub session: Session,
    /// Messages an undo had hidden, now deleted because a new prompt went ahead from before them.
    pub discarded: Vec<String>,
}

fn discard_reverted(conn: &Connection, session_id: &str) -> rusqlite::Result<Vec<String>> {
    let revert: Option<Revert> = conn
        .prepare_cached("SELECT revert_json FROM session WHERE id = ?1")?
        .query_row([session_id], |row| row.get::<_, Option<String>>(0))
        .optional()?
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok());
    let Some(revert) = revert else { return Ok(Vec::new()) };
    let ids: Vec<String> = conn
        .prepare_cached("SELECT id FROM message WHERE session_id = ?1 AND id >= ?2 ORDER BY id")?
        .query_map(params![session_id, revert.message_id], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    conn.prepare_cached("DELETE FROM submission WHERE session_id = ?1 AND message_id >= ?2")?.execute(params![session_id, revert.message_id])?;
    conn.prepare_cached("DELETE FROM message WHERE session_id = ?1 AND id >= ?2")?.execute(params![session_id, revert.message_id])?;
    conn.prepare_cached("UPDATE session SET revert_json = NULL WHERE id = ?1")?.execute([session_id])?;
    Ok(ids)
}


pub(super) fn session_from(new: NewSession, cutoff: Option<&str>) -> Session {
    let now = id::now_ms();
    Session {
        id: id::new("ses"),
        workspace_id: new.workspace_id.into(),
        parent_id: new.parent_id.map(Into::into),
        visibility: new.visibility,
        title: new.title.into(),
        agent: new.agent.into(),
        model: new.model.cloned(),
        created_at: now,
        updated_at: now,
        archived_at: None,
        branch_cutoff: cutoff.map(Into::into),
        revert: None,
        running: false,
    }
}

pub(super) fn insert_session(conn: &Connection, session: &Session) -> rusqlite::Result<()> {
    conn.prepare_cached(
        "INSERT INTO session(id, workspace_id, parent_id, visibility, title, agent, model_provider, model_id, created_at, updated_at, branch_cutoff)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?
    .execute(params![
        session.id,
        session.workspace_id,
        session.parent_id,
        visibility_str(session.visibility),
        session.title,
        session.agent,
        session.model.as_ref().map(|m| &m.provider),
        session.model.as_ref().map(|m| &m.model),
        session.created_at,
        session.updated_at,
        session.branch_cutoff
    ])?;
    Ok(())
}

fn save_message_in(conn: &Connection, message: &Message) -> rusqlite::Result<()> {
    conn.prepare_cached("UPDATE message SET status = ?2, usage_json = ?3, cost = ?4, error = ?5, finished_at = ?6 WHERE id = ?1")?
        .execute(params![
            message.id,
            status_str(message.status),
            serde_json::to_string(&message.usage).unwrap(),
            message.cost,
            message.error,
            message.finished_at
        ])?;
    Ok(())
}

/// Runs `f` inside one transaction: all of its writes land, or none do.
pub(super) fn transaction<T>(conn: &Connection, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    conn.execute_batch("BEGIN")?;
    match f(conn) {
        Ok(value) => {
            conn.execute_batch("COMMIT")?;
            Ok(value)
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn insert_message(conn: &Connection, session_id: &str, role: Role, model: Option<&ModelRef>, summary: bool) -> rusqlite::Result<Message> {
    let message = Message {
        id: id::new("msg"),
        session_id: session_id.into(),
        role,
        status: if role == Role::User { MessageStatus::Done } else { MessageStatus::Streaming },
        model: model.cloned(),
        usage: Usage::default(),
        cost: 0.0,
        error: None,
        created_at: id::now_ms(),
        finished_at: None,
        summary,
    };
    conn.prepare_cached(
        "INSERT INTO message(id, session_id, role, status, model_provider, model_id, usage_json, cost, created_at, summary)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9)",
    )?
    .execute(params![
        message.id,
        message.session_id,
        role_str(role),
        status_str(message.status),
        model.map(|m| &m.provider),
        model.map(|m| &m.model),
        serde_json::to_string(&message.usage).unwrap(),
        message.created_at,
        summary
    ])?;
    Ok(message)
}

fn insert_part(conn: &Connection, message_id: &str, session_id: &str, part: Part) -> rusqlite::Result<PartRow> {
    let row = PartRow { id: id::new("prt"), message_id: message_id.into(), session_id: session_id.into(), part };
    conn.prepare_cached("INSERT INTO part(id, message_id, session_id, json) VALUES(?1, ?2, ?3, ?4)")?
        .execute(params![row.id, row.message_id, row.session_id, serde_json::to_string(&row.part).unwrap()])?;
    Ok(row)
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    use crate::store::tests::store;

    #[test]
    fn admission_is_all_or_nothing() {
        let store = store();
        let session = store
            .create_session(NewSession { workspace_id: "w", parent_id: None, visibility: Visibility::Sibling, title: "", agent: "build", model: None })
            .unwrap();
        let model = ModelRef { provider: "p".into(), model: "m".into() };
        let Admitted { message, parts: rows, session: updated, .. } = store.admit_prompt(&session.id, &model, vec![Part::Text { text: "hi".into() }], Some(("sub_1", "h1"))).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(updated.model, Some(model.clone()));
        assert_eq!(store.transcript(&session.id).unwrap()[0].info.id, message.id);
        let failed = store.admit_prompt("ses_missing", &model, vec![Part::Text { text: "x".into() }], None);
        assert!(failed.is_err());
        let count: i64 = store.lock().query_row("SELECT COUNT(*) FROM message", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 1, "the failed admission must leave no message behind");
        let found = store.submission("sub_1").unwrap().unwrap();
        assert_eq!((found.session_id.as_str(), found.message_id.as_str(), found.payload_hash.as_str()), (session.id.as_str(), message.id.as_str(), "h1"));
        assert!(store.submission("sub_nope").unwrap().is_none());
        assert!(store.delete_session(&session.id).unwrap());
        assert!(store.submission("sub_1").unwrap().is_none(), "cascade");
        assert!(store.transcript(&session.id).unwrap().is_empty());
        assert!(!store.delete_session(&session.id).unwrap());
    }
}

pub struct Submission {
    pub session_id: String,
    pub message_id: String,
    pub payload_hash: String,
}

impl Store {
    pub fn submission(&self, id: &str) -> rusqlite::Result<Option<Submission>> {
        self.lock()
            .prepare_cached("SELECT session_id, message_id, payload_hash FROM submission WHERE id = ?1")?
            .query_row([id], |row| Ok(Submission { session_id: row.get(0)?, message_id: row.get(1)?, payload_hash: row.get(2)? }))
            .optional()
    }

    /// Removes a session and everything under it. Archive first; this is the purge that follows.
    pub fn delete_session(&self, id: &str) -> rusqlite::Result<bool> {
        let deleted = self.lock().prepare_cached("DELETE FROM session WHERE id = ?1")?.execute([id])?;
        Ok(deleted > 0)
    }
}

#[cfg(test)]
mod paging_tests {
    use super::*;
    use crate::store::tests::store;

    #[test]
    fn equal_timestamps_do_not_skip_sessions_across_pages() {
        let store = store();
        let ids: Vec<String> = (0..5)
            .map(|_| store.create_session(NewSession { workspace_id: "w", parent_id: None, visibility: Visibility::Sibling, title: "", agent: "build", model: None }).unwrap().id)
            .collect();
        store.lock().execute("UPDATE session SET updated_at = 1000", []).unwrap();
        let mut seen = Vec::new();
        let mut before: Option<String> = None;
        loop {
            let page = store.sessions(SessionFilter { workspace_id: Some("w"), archived: false, before: before.as_deref(), limit: 2 }).unwrap();
            seen.extend(page.iter().map(|s| s.id.clone()));
            if page.len() < 2 {
                break;
            }
            before = page.last().map(|s| s.id.clone());
        }
        let mut expected = ids.clone();
        expected.sort();
        expected.reverse();
        assert_eq!(seen, expected);
    }
}
