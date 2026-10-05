//! Conversations written from another store (`drift-migrate`) in pages, each recorded so it comes in
//! once. A conversation stays archived, out of the list, until its last page lands; one interrupted
//! midway is removed and written again by the next run.

use rusqlite::{params, Connection, OptionalExtension};

use super::sessions::{role_str, session_in, status_str, transaction, visibility_str};
use super::Store;
use crate::id;
use crate::session::types::{Message, MessageWithParts, Session, Todo};

/// While an import writes gigabytes, SQLite's automatic checkpoint (copying the log into the
/// database every few MB) would run inside the shared connection's commits and hold everyone up.
/// It is switched off for the import and run from a connection of its own between pages instead; a
/// checkpoint moves pages already committed and writes no data of its own.
pub struct ImportCheckpoints<'a> {
    store: &'a Store,
    conn: Option<Connection>,
}

impl ImportCheckpoints<'_> {
    /// Copies what the log holds into the database without blocking the store's connection.
    pub fn run(&self) {
        if let Some(conn) = &self.conn {
            let _ = conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE)");
        }
    }
}

impl Drop for ImportCheckpoints<'_> {
    fn drop(&mut self) {
        if self.conn.is_some() {
            let _ = self.store.lock().pragma_update(None, "wal_autocheckpoint", 1000);
        }
    }
}

impl Store {
    /// Checkpoints for the length of an import; automatic ones resume when it is dropped.
    pub fn import_checkpoints(&self) -> ImportCheckpoints<'_> {
        let conn = {
            let shared = self.lock();
            let path = shared.path().filter(|path| !path.is_empty()).map(std::path::PathBuf::from);
            let own = path.and_then(|path| Connection::open(path).ok());
            if own.is_some() {
                let _ = shared.pragma_update(None, "wal_autocheckpoint", 0);
            }
            own
        };
        ImportCheckpoints { store: self, conn }
    }

    /// Whether a conversation with this id was fully imported, even if it has since been deleted.
    pub fn was_imported(&self, session_id: &str) -> rusqlite::Result<bool> {
        self.lock().prepare_cached("SELECT 1 FROM imported_session WHERE id = ?1 AND complete = 1")?.exists([session_id])
    }

    /// Whether a conversation is here and is not an import an earlier run left unfinished.
    pub fn holds_session(&self, session_id: &str) -> rusqlite::Result<bool> {
        let conn = self.lock();
        let unfinished = conn.prepare_cached("SELECT 1 FROM imported_session WHERE id = ?1 AND complete = 0")?.exists([session_id])?;
        Ok(!unfinished && conn.prepare_cached("SELECT 1 FROM session WHERE id = ?1")?.exists([session_id])?)
    }

    /// Starts writing `session`, hidden; `false`, writing nothing, when it was imported before or a
    /// conversation with its id already exists. A start left unfinished by an earlier run is discarded first.
    pub fn begin_import(&self, session: &Session) -> rusqlite::Result<bool> {
        transaction(&self.lock(), |conn| {
            let complete: Option<bool> = conn.prepare_cached("SELECT complete FROM imported_session WHERE id = ?1")?.query_row([&session.id], |row| row.get(0)).optional()?;
            match complete {
                Some(true) => return Ok(false),
                Some(false) => {
                    conn.prepare_cached("DELETE FROM session WHERE id = ?1")?.execute([&session.id])?;
                    conn.prepare_cached("DELETE FROM imported_session WHERE id = ?1")?.execute([&session.id])?;
                }
                None if conn.prepare_cached("SELECT 1 FROM session WHERE id = ?1")?.exists([&session.id])? => return Ok(false),
                None => {}
            }
            insert_session(conn, session, Some(id::now_ms()))?;
            conn.prepare_cached("INSERT INTO imported_session(id, imported_at, complete) VALUES(?1, ?2, 0)")?.execute(params![session.id, id::now_ms()])?;
            Ok(true)
        })
    }

    /// Writes one page of the conversation's messages, with their parts, in one short transaction.
    pub fn import_page(&self, messages: &[MessageWithParts]) -> rusqlite::Result<()> {
        transaction(&self.lock(), |conn| {
            for message in messages {
                insert_message(conn, &message.info)?;
                for row in &message.parts {
                    conn.prepare_cached("INSERT INTO part(id, message_id, session_id, json) VALUES(?1, ?2, ?3, ?4)")?.execute(params![row.id, row.message_id, row.session_id, row.part.stored()])?;
                }
            }
            Ok(())
        })
    }

    /// Removes an import that failed partway, so its half never shows; only an unfinished import is touched.
    pub fn discard_import(&self, session_id: &str) -> rusqlite::Result<()> {
        transaction(&self.lock(), |conn| {
            if conn.prepare_cached("DELETE FROM imported_session WHERE id = ?1 AND complete = 0")?.execute([session_id])? > 0 {
                conn.prepare_cached("DELETE FROM session WHERE id = ?1")?.execute([session_id])?;
            }
            Ok(())
        })
    }

    /// Lists the conversation as it should be (archived or not) and records it as imported.
    pub fn finish_import(&self, session_id: &str, archived_at: Option<i64>, todos: &[Todo]) -> rusqlite::Result<Option<Session>> {
        transaction(&self.lock(), |conn| {
            conn.prepare_cached("UPDATE session SET archived_at = ?2 WHERE id = ?1")?.execute(params![session_id, archived_at])?;
            if !todos.is_empty() {
                conn.prepare_cached("INSERT OR REPLACE INTO todo(session_id, json, updated_at) VALUES(?1, ?2, ?3)")?.execute(params![session_id, serde_json::to_string(todos).unwrap(), id::now_ms()])?;
            }
            conn.prepare_cached("UPDATE imported_session SET complete = 1, imported_at = ?2 WHERE id = ?1")?.execute(params![session_id, id::now_ms()])?;
            session_in(conn, session_id)
        })
    }
}

fn insert_session(conn: &Connection, session: &Session, archived_at: Option<i64>) -> rusqlite::Result<()> {
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
        archived_at,
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
    use crate::store::SessionFilter;

    fn session(id: &str) -> Session {
        Session { id: id.into(), workspace_id: "w".into(), parent_id: None, visibility: Visibility::Sibling, title: "Old talk".into(), agent: "build".into(), model: None, variant: Some("high".into()), created_at: 10, updated_at: 20, archived_at: None, branch_cutoff: None, revert: None, running: false }
    }

    fn page(session_id: &str, n: u64) -> Vec<MessageWithParts> {
        let id = format!("msg_{n:016x}aaaaaaaa");
        let info = Message { id: id.clone(), session_id: session_id.into(), role: Role::User, status: MessageStatus::Done, model: None, agent: Some("build".into()), usage: Usage::default(), cost: 0.0, error: None, created_at: 10, finished_at: Some(10), summary: false, ending: None };
        let part = PartRow { id: format!("prt_{n:016x}aaaaaaaa"), message_id: id, session_id: session_id.into(), provider_signature: None, part: Part::Text { text: format!("page {n}") } };
        vec![MessageWithParts { info, parts: vec![part] }]
    }

    fn listed(store: &Store) -> usize {
        store.sessions(SessionFilter { workspace_id: Some("w"), archived: false, before: None, limit: 10 }).unwrap().len()
    }

    #[test]
    fn a_conversation_is_hidden_until_its_last_page_and_comes_in_once() {
        let store = store();
        assert!(store.begin_import(&session("ses_old")).unwrap());
        store.import_page(&page("ses_old", 1)).unwrap();
        assert_eq!(listed(&store), 0, "not listed while pages are still coming");
        store.import_page(&page("ses_old", 2)).unwrap();
        let todos = vec![Todo { content: "a".into(), status: TodoStatus::Pending, priority: "high".into() }];
        let done = store.finish_import("ses_old", None, &todos).unwrap().unwrap();
        assert_eq!((done.title.as_str(), done.variant.as_deref(), done.updated_at, done.archived_at), ("Old talk", Some("high"), 20, None));
        assert_eq!((listed(&store), store.transcript("ses_old").unwrap().len(), store.todos("ses_old").unwrap().len()), (1, 2, 1));
        assert!(store.was_imported("ses_old").unwrap());
        assert!(!store.begin_import(&session("ses_old")).unwrap(), "a second import writes nothing");
        store.lock().execute("DELETE FROM session WHERE id = 'ses_old'", []).unwrap();
        assert!(!store.begin_import(&session("ses_old")).unwrap(), "nor does one after the user deleted it");
    }

    #[test]
    fn automatic_checkpoints_pause_for_an_import_and_resume_after_it() {
        let dir = std::env::temp_dir().join(format!("drift-import-{}", crate::random_hex(4)));
        let store = crate::store::open(&dir).unwrap();
        let automatic = |store: &Store| store.lock().pragma_query_value(None, "wal_autocheckpoint", |row| row.get::<_, i64>(0)).unwrap();
        {
            let checkpoints = store.import_checkpoints();
            assert_eq!(automatic(&store), 0, "the import's own connection checkpoints instead");
            assert!(store.begin_import(&session("ses_old")).unwrap());
            store.import_page(&page("ses_old", 1)).unwrap();
            checkpoints.run();
        }
        assert_eq!(automatic(&store), 1000);
        assert_eq!(store.transcript("ses_old").unwrap().len(), 1);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_import_cut_off_midway_is_written_again_from_the_start() {
        let store = store();
        assert!(store.begin_import(&session("ses_old")).unwrap());
        store.import_page(&page("ses_old", 1)).unwrap();
        assert!(!store.was_imported("ses_old").unwrap());
        assert!(store.begin_import(&session("ses_old")).unwrap(), "the unfinished copy is discarded and started over");
        assert!(store.transcript("ses_old").unwrap().is_empty());
        store.import_page(&page("ses_old", 1)).unwrap();
        store.finish_import("ses_old", Some(99), &[]).unwrap();
        assert_eq!(store.session("ses_old").unwrap().unwrap().archived_at, Some(99), "lands archived when asked");
        assert!(store.begin_import(&Session { id: "ses_native".into(), ..session("x") }).unwrap());
        store.lock().execute("DELETE FROM imported_session WHERE id = 'ses_native'", []).unwrap();
        assert!(!store.begin_import(&Session { id: "ses_native".into(), ..session("x") }).unwrap(), "a conversation that is not an import is never replaced");
    }
}
