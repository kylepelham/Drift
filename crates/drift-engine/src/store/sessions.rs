use rusqlite::{params, Connection, OptionalExtension, Row};

use super::Store;
use crate::id;
use crate::session::types::{
    Message, MessageStatus, MessageWithParts, ModelRef, Part, PartRow, Role, Session, Usage,
    Visibility,
};

const SESSION_COLUMNS: &str = "id, workspace_id, parent_id, visibility, title, agent, model_provider, model_id, created_at, updated_at, archived_at";
const MESSAGE_COLUMNS: &str = "id, session_id, role, status, model_provider, model_id, usage_json, cost, error, created_at, finished_at";

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
        let now = id::now_ms();
        let session = Session {
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
        };
        self.lock().prepare_cached(
            "INSERT INTO session(id, workspace_id, parent_id, visibility, title, agent, model_provider, model_id, created_at, updated_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
            now,
            now
        ])?;
        Ok(session)
    }

    pub fn session(&self, id: &str) -> rusqlite::Result<Option<Session>> {
        self.lock()
            .prepare_cached(&format!("SELECT {SESSION_COLUMNS} FROM session WHERE id = ?1"))?
            .query_row([id], map_session)
            .optional()
    }

    /// Listed newest first, siblings and roots only; hidden children never appear here.
    pub fn sessions(&self, filter: SessionFilter) -> rusqlite::Result<Vec<Session>> {
        let conn = self.lock();
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {SESSION_COLUMNS} FROM session
             WHERE (?1 IS NULL OR workspace_id = ?1)
               AND (archived_at IS NOT NULL) = ?2
               AND visibility = 'sibling'
               AND (?3 IS NULL OR updated_at < (SELECT updated_at FROM session WHERE id = ?3))
             ORDER BY updated_at DESC LIMIT ?4"
        ))?;
        let rows = stmt.query_map(
            params![filter.workspace_id, filter.archived, filter.before, filter.limit as i64],
            map_session,
        )?;
        rows.collect()
    }

    pub fn update_session(&self, id: &str, title: Option<&str>, model: Option<&ModelRef>) -> rusqlite::Result<Option<Session>> {
        let conn = self.lock();
        conn.prepare_cached(
            "UPDATE session SET title = COALESCE(?2, title),
                model_provider = COALESCE(?3, model_provider), model_id = COALESCE(?4, model_id),
                updated_at = ?5 WHERE id = ?1",
        )?
        .execute(params![id, title, model.map(|m| &m.provider), model.map(|m| &m.model), id::now_ms()])?;
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
        };
        self.lock()
            .prepare_cached(
                "INSERT INTO message(id, session_id, role, status, model_provider, model_id, usage_json, cost, created_at)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8)",
            )?
            .execute(params![
                message.id,
                message.session_id,
                role_str(role),
                status_str(message.status),
                model.map(|m| &m.provider),
                model.map(|m| &m.model),
                serde_json::to_string(&message.usage).unwrap(),
                message.created_at
            ])?;
        Ok(message)
    }

    pub fn save_message(&self, message: &Message) -> rusqlite::Result<()> {
        self.lock()
            .prepare_cached(
                "UPDATE message SET status = ?2, usage_json = ?3, cost = ?4, error = ?5, finished_at = ?6 WHERE id = ?1",
            )?
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
        let row = PartRow {
            id: id::new("prt"),
            message_id: message_id.into(),
            session_id: session_id.into(),
            part,
        };
        self.lock()
            .prepare_cached("INSERT INTO part(id, message_id, session_id, json) VALUES(?1, ?2, ?3, ?4)")?
            .execute(params![row.id, row.message_id, row.session_id, serde_json::to_string(&row.part).unwrap()])?;
        Ok(row)
    }

    pub fn save_part(&self, row: &PartRow) -> rusqlite::Result<()> {
        self.lock()
            .prepare_cached("UPDATE part SET json = ?2 WHERE id = ?1")?
            .execute(params![row.id, serde_json::to_string(&row.part).unwrap()])?;
        Ok(())
    }

    /// Anything still streaming when the engine last stopped did not finish.
    pub fn abandon_streaming_messages(&self) -> rusqlite::Result<usize> {
        self.lock().execute(
            "UPDATE message SET status = 'aborted', finished_at = ?1 WHERE status = 'streaming'",
            [id::now_ms()],
        )
    }
}

fn session_in(conn: &Connection, id: &str) -> rusqlite::Result<Option<Session>> {
    conn.prepare_cached(&format!("SELECT {SESSION_COLUMNS} FROM session WHERE id = ?1"))?
        .query_row([id], map_session)
        .optional()
}

fn map_session(row: &Row) -> rusqlite::Result<Session> {
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
    }
}

fn parse_status(status: &str) -> MessageStatus {
    match status {
        "streaming" => MessageStatus::Streaming,
        "aborted" => MessageStatus::Aborted,
        "error" => MessageStatus::Error,
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
        let updated = store.update_session(&a.id, Some("Title"), Some(&model)).unwrap().unwrap();
        assert_eq!(updated.title, "Title");
        assert_eq!(updated.model, Some(model));

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
    fn hidden_children_are_not_listed() {
        let store = store();
        let parent = store.create_session(new("w")).unwrap();
        store
            .create_session(NewSession { parent_id: Some(&parent.id), visibility: Visibility::Hidden, ..new("w") })
            .unwrap();
        let listed = store
            .sessions(SessionFilter { workspace_id: Some("w"), archived: false, before: None, limit: 10 })
            .unwrap();
        assert_eq!(listed.len(), 1);
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
