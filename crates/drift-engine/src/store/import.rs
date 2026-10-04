//! Conversations written whole from another store (`drift-migrate`), each recorded so it comes in once.

use rusqlite::{params, Connection, OptionalExtension};

use super::sessions::{role_str, status_str, transaction, visibility_str};
use super::Store;
use crate::id;
use crate::session::types::{Message, MessageWithParts, Session, Todo};

/// One conversation as it is to be stored: its parts already in this engine's shape and ids.
pub struct ImportedSession {
    pub session: Session,
    pub messages: Vec<MessageWithParts>,
    pub todos: Vec<Todo>,
}

impl Store {
    /// Whether a conversation with this id was ever imported, even if it has since been deleted.
    pub fn was_imported(&self, session_id: &str) -> rusqlite::Result<bool> {
        self.lock().prepare_cached("SELECT 1 FROM imported_session WHERE id = ?1")?.exists([session_id])
    }

    /// Writes the conversation and records it in one transaction; `false`, writing nothing, when it
    /// was imported before or a conversation with its id already exists.
    pub fn import_session(&self, imported: &ImportedSession) -> rusqlite::Result<bool> {
        transaction(&self.lock(), |conn| {
            let id = &imported.session.id;
            let known = conn.prepare_cached("SELECT 1 FROM imported_session WHERE id = ?1")?.exists([id])?
                || conn.prepare_cached("SELECT 1 FROM session WHERE id = ?1")?.query_row([id], |_| Ok(())).optional()?.is_some();
            if known {
                return Ok(false);
            }
            insert_session(conn, &imported.session)?;
            for message in &imported.messages {
                insert_message(conn, &message.info)?;
                for row in &message.parts {
                    conn.prepare_cached("INSERT INTO part(id, message_id, session_id, json) VALUES(?1, ?2, ?3, ?4)")?.execute(params![row.id, row.message_id, row.session_id, row.part.stored()])?;
                }
            }
            if !imported.todos.is_empty() {
                conn.prepare_cached("INSERT INTO todo(session_id, json, updated_at) VALUES(?1, ?2, ?3)")?.execute(params![id, serde_json::to_string(&imported.todos).unwrap(), imported.session.updated_at])?;
            }
            conn.prepare_cached("INSERT INTO imported_session(id, imported_at) VALUES(?1, ?2)")?.execute(params![id, id::now_ms()])?;
            Ok(true)
        })
    }
}

fn insert_session(conn: &Connection, session: &Session) -> rusqlite::Result<()> {
    conn.prepare_cached(
        "INSERT INTO session(id, workspace_id, parent_id, visibility, title, agent, model_provider, model_id, created_at, updated_at, archived_at, variant)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
        session.archived_at,
        session.variant
    ])?;
    Ok(())
}

fn insert_message(conn: &Connection, message: &Message) -> rusqlite::Result<()> {
    conn.prepare_cached(
        "INSERT INTO message(id, session_id, role, status, model_provider, model_id, usage_json, cost, error, created_at, finished_at, summary, agent, ending)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
    )?
    .execute(params![
        message.id,
        message.session_id,
        role_str(message.role),
        status_str(message.status),
        message.model.as_ref().map(|m| &m.provider),
        message.model.as_ref().map(|m| &m.model),
        serde_json::to_string(&message.usage).unwrap(),
        message.cost,
        message.error,
        message.created_at,
        message.finished_at,
        message.summary,
        message.agent,
        message.ending.map(|ending| ending.as_str())
    ])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::types::{MessageStatus, Part, PartRow, Role, TodoStatus, Usage, Visibility};
    use crate::store::tests::store;

    fn imported(id: &str) -> ImportedSession {
        let session = Session {
            id: id.into(),
            workspace_id: "w".into(),
            parent_id: None,
            visibility: Visibility::Sibling,
            title: "Old talk".into(),
            agent: "build".into(),
            model: None,
            variant: Some("high".into()),
            created_at: 10,
            updated_at: 20,
            archived_at: None,
            branch_cutoff: None,
            revert: None,
            running: false,
        };
        let info = Message { id: "msg_0000000000001000aaaaaaaa".into(), session_id: id.into(), role: Role::User, status: MessageStatus::Done, model: None, agent: Some("build".into()), usage: Usage::default(), cost: 0.0, error: None, created_at: 10, finished_at: Some(10), summary: false, ending: None };
        let part = PartRow { id: "prt_0000000000001000aaaaaaaa".into(), message_id: info.id.clone(), session_id: id.into(), provider_signature: None, part: Part::Text { text: "hello".into() } };
        let todos = vec![Todo { content: "a".into(), status: TodoStatus::Pending, priority: "high".into() }];
        ImportedSession { session, messages: vec![MessageWithParts { info, parts: vec![part] }], todos }
    }

    #[test]
    fn a_conversation_comes_in_once_and_never_again_after_a_delete() {
        let store = store();
        assert!(store.import_session(&imported("ses_old")).unwrap());
        let session = store.session("ses_old").unwrap().unwrap();
        assert_eq!((session.title.as_str(), session.variant.as_deref(), session.updated_at), ("Old talk", Some("high"), 20));
        let transcript = store.transcript("ses_old").unwrap();
        assert_eq!(transcript[0].parts[0].part, Part::Text { text: "hello".into() });
        assert_eq!(store.todos("ses_old").unwrap().len(), 1);
        assert!(store.was_imported("ses_old").unwrap());
        assert!(!store.import_session(&imported("ses_old")).unwrap(), "a second import writes nothing");
        store.lock().execute("DELETE FROM session WHERE id = 'ses_old'", []).unwrap();
        assert!(!store.import_session(&imported("ses_old")).unwrap(), "nor does one after the user deleted it");
        assert!(store.session("ses_old").unwrap().is_none());
    }

    #[test]
    fn a_failed_write_leaves_nothing_behind_and_can_be_tried_again() {
        let store = store();
        let mut broken = imported("ses_old");
        let again = broken.messages[0].parts[0].clone();
        broken.messages[0].parts.push(again);
        assert!(store.import_session(&broken).is_err(), "a duplicate part id fails the write");
        assert!(store.session("ses_old").unwrap().is_none() && !store.was_imported("ses_old").unwrap());
        assert!(store.import_session(&imported("ses_old")).unwrap());
    }
}
