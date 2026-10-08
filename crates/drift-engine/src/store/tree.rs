//! Whole-session operations: copying a transcript into a fork, moving a session with its subagents.

use std::collections::HashMap;

use rusqlite::{Connection, params};

use super::Store;
use super::sessions::{NewSession, insert_session, session_from, transaction};
use crate::id;
use crate::session::types::{Part, Session};

/// Messages copied per transaction when forking.
const FORK_PAGE: usize = 100;

impl Store {
    /// Copies finished history in pages; `None` means a selected message disappeared and the copy was removed.
    pub fn fork_session(
        &self,
        source_id: &str,
        new: NewSession,
        through: &str,
        cutoff: Option<&str>,
    ) -> rusqlite::Result<Option<Session>> {
        let session = session_from(new, cutoff);
        let messages: Vec<String> = transaction(&self.lock(), |conn| {
            insert_session(conn, &session)?;
            conn.prepare_cached("UPDATE session SET variant = (SELECT variant FROM session WHERE id = ?2), archived_at = ?3 WHERE id = ?1")?.execute(params![session.id, source_id, id::now_ms()])?;
            conn.prepare_cached(
                "SELECT id FROM message WHERE session_id = ?1 AND id <= ?2 AND status != 'streaming' ORDER BY id",
            )?
            .query_map(params![source_id, through], |row| row.get(0))?
            .collect()
        })?;
        let result = self.finish_fork(source_id, &session.id, through, &messages);
        if matches!(&result, Ok(Some(_))) {
            return result;
        }
        let cleanup = self
            .lock()
            .prepare_cached("DELETE FROM session WHERE id = ?1")
            .and_then(|mut statement| statement.execute([&session.id]));
        if let Err(error) = cleanup {
            if result.is_ok() {
                return Err(error);
            }
            eprintln!("could not clean up failed fork {}: {error}", session.id);
        }
        result
    }

    fn finish_fork(
        &self,
        source_id: &str,
        fork_id: &str,
        through: &str,
        messages: &[String],
    ) -> rusqlite::Result<Option<Session>> {
        if messages.last().is_none_or(|id| id != through) || !self.copy_pages(messages, fork_id)? {
            return Ok(None);
        }
        transaction(&self.lock(), |conn| {
            super::reads::copy_reads(conn, source_id, fork_id, through)?;
            conn.prepare_cached("UPDATE session SET archived_at = NULL WHERE id = ?1")?
                .execute([fork_id])?;
            super::sessions::session_in(conn, fork_id)
        })
    }

    /// Copies `messages` into `fork_id`, [`FORK_PAGE`] per transaction; `false` once one is found gone.
    fn copy_pages(&self, messages: &[String], fork_id: &str) -> rusqlite::Result<bool> {
        let mut copies = HashMap::new();
        for page in messages.chunks(FORK_PAGE) {
            let whole = transaction(&self.lock(), |conn| {
                page.iter().try_fold(true, |whole, message| {
                    Ok(whole && copy_message(conn, message, fork_id, &mut copies)?)
                })
            })?;
            if !whole {
                return Ok(false);
            }
        }
        Ok(true)
    }
    /// The session and its subagents, at any depth. Branches are independent and stay behind.
    pub fn session_tree(&self, id: &str) -> rusqlite::Result<Vec<String>> {
        self.lock()
            .prepare_cached(
                "WITH RECURSIVE tree(id) AS (
                     SELECT id FROM session WHERE id = ?1
                     UNION SELECT s.id FROM session s JOIN tree ON s.parent_id = tree.id WHERE s.visibility = 'hidden'
                 ) SELECT id FROM tree",
            )?
            .query_map([id], |row| row.get(0))?
            .collect()
    }

    /// Every shadow blob a recorded change refers to, archived sessions included, grouped by the
    /// workspace that owns its history (recorded with the change; the session's workspace for older
    /// records): what undo and redo could still need. Rows are collected under the database lock;
    /// the JSON is read after it is released.
    pub fn recorded_blobs(&self) -> rusqlite::Result<HashMap<String, Vec<String>>> {
        let rows: Vec<(String, String)> = {
            let conn = self.lock();
            let mut statement = conn.prepare_cached("SELECT s.workspace_id, p.json FROM part p JOIN session s ON s.id = p.session_id WHERE p.json LIKE '%\"changes\"%'")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut blobs: HashMap<String, std::collections::BTreeSet<String>> = HashMap::new();
        for (session_workspace, json) in rows {
            let Ok(Part::ToolCall {
                metadata: Some(metadata),
                ..
            }) = serde_json::from_str::<Part>(&json)
            else {
                continue;
            };
            let Some(changes) = metadata.get("changes").and_then(|c| c.as_array()) else {
                continue;
            };
            let owner = metadata["owner"].as_str().map_or(session_workspace, str::to_string);
            let kept = blobs.entry(owner).or_default();
            for change in changes {
                kept.extend(
                    ["before", "after"]
                        .iter()
                        .filter_map(|side| change[side].as_str().map(str::to_string)),
                );
            }
        }
        Ok(blobs
            .into_iter()
            .map(|(owner, set)| (owner, set.into_iter().collect()))
            .collect())
    }

    pub fn move_sessions(&self, ids: &[String], workspace_id: &str) -> rusqlite::Result<()> {
        transaction(&self.lock(), |conn| {
            let mut update =
                conn.prepare_cached("UPDATE session SET workspace_id = ?2, updated_at = ?3 WHERE id = ?1")?;
            let now = id::now_ms();
            for session in ids {
                update.execute(params![session, workspace_id, now])?;
            }
            Ok(())
        })
    }
}

/// Copies one message inside SQLite, its compaction boundary pointed at its copied tail through `copies`; `false` if it is gone.
fn copy_message(
    conn: &Connection,
    source: &str,
    session_id: &str,
    copies: &mut HashMap<String, String>,
) -> rusqlite::Result<bool> {
    let message_id = id::new("msg");
    let copied = conn
        .prepare_cached(
            "INSERT INTO message(id, session_id, role, status, model_provider, model_id, usage_json, cost, error, created_at, finished_at, summary, agent, ending)
             SELECT ?1, ?2, role, status, model_provider, model_id, usage_json, cost, error, created_at, finished_at, summary, agent, ending FROM message WHERE id = ?3",
        )?
        .execute(params![message_id, session_id, source])?;
    if copied == 0 {
        return Ok(false);
    }
    copies.insert(source.to_string(), message_id.clone());
    // The copy names the same images, so they stay as long as either message does.
    conn.prepare_cached("INSERT INTO blob_ref(hash, message_id) SELECT hash, ?1 FROM blob_ref WHERE message_id = ?2")?
        .execute(params![message_id, source])?;
    let parts: Vec<(String, bool)> = conn
        .prepare_cached(
            "SELECT id, json_extract(json, '$.type') = 'compaction' FROM part WHERE message_id = ?1 ORDER BY id",
        )?
        .query_map([source], |row| {
            Ok((row.get(0)?, row.get::<_, Option<bool>>(1)?.unwrap_or(false)))
        })?
        .collect::<rusqlite::Result<_>>()?;
    for (part, boundary) in parts {
        let copy = id::new("prt");
        if boundary {
            copy_boundary(conn, &part, &copy, &message_id, session_id, copies)?;
            continue;
        }
        conn.prepare_cached("INSERT INTO part(id, message_id, session_id, json, provider_signature) SELECT ?1, ?2, ?3, json, provider_signature FROM part WHERE id = ?4")?.execute(params![copy, message_id, session_id, part])?;
    }
    Ok(true)
}

/// A compaction boundary, pointed at the copy of the tail it kept.
fn copy_boundary(
    conn: &Connection,
    part: &str,
    copy: &str,
    message_id: &str,
    session_id: &str,
    copies: &HashMap<String, String>,
) -> rusqlite::Result<()> {
    let json: String = conn
        .prepare_cached("SELECT json FROM part WHERE id = ?1")?
        .query_row([part], |row| row.get(0))?;
    let mut parsed = Part::from_stored(&json);
    if let Part::Compaction { tail_from, .. } = &mut parsed {
        *tail_from = tail_from.as_ref().and_then(|tail| copies.get(tail).cloned());
    }
    conn.prepare_cached("INSERT INTO part(id, message_id, session_id, json) VALUES(?1, ?2, ?3, ?4)")?
        .execute(params![copy, message_id, session_id, parsed.stored()])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::session::types::{MessageStatus, Part, Role, Visibility};
    use crate::store::NewSession;
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
    fn a_long_history_is_copied_page_by_page_with_its_compaction_boundary_rewritten() {
        let store = store();
        let source = store.create_session(new("w")).unwrap();
        let mut ids = Vec::new();
        for i in 0..(super::FORK_PAGE * 2 + 30) {
            let mut message = store
                .create_message(&source.id, if i % 2 == 0 { Role::User } else { Role::Assistant }, None)
                .unwrap();
            message.status = MessageStatus::Done;
            store.save_message(&message).unwrap();
            store
                .add_part(&message.id, &source.id, Part::Text { text: format!("m{i}") })
                .unwrap();
            ids.push(message.id);
        }
        let boundary = store.create_message(&source.id, Role::User, None).unwrap();
        store
            .add_part(
                &boundary.id,
                &source.id,
                Part::Compaction {
                    auto: true,
                    tail_from: Some(ids[3].clone()),
                },
            )
            .unwrap();
        let fork = store
            .fork_session(&source.id, new("w"), &boundary.id, None)
            .unwrap()
            .unwrap();
        assert!(fork.archived_at.is_none(), "listed once every page landed");
        let copied = store.transcript(&fork.id).unwrap();
        assert_eq!(copied.len(), ids.len() + 1);
        assert!(matches!(&copied[0].parts[0].part, Part::Text { text } if text == "m0"));
        let Part::Compaction {
            tail_from: Some(tail), ..
        } = &copied.last().unwrap().parts[0].part
        else {
            panic!("the boundary was copied")
        };
        assert_eq!(
            tail, &copied[3].info.id,
            "it points at the copy of its tail, pages earlier"
        );
    }

    #[test]
    fn a_fork_copies_parts_this_build_cannot_read_byte_for_byte() {
        let store = store();
        let source = store.create_session(new("w")).unwrap();
        let message = store.create_message(&source.id, Role::User, None).unwrap();
        let stored = [
            r#"{"type":"subtask","prompt":"go"}"#,
            r#"{"type":"compaction","auto":"not a bool"}"#,
        ];
        for (n, json) in stored.iter().enumerate() {
            store
                .lock()
                .execute(
                    "INSERT INTO part(id, message_id, session_id, json) VALUES(?1, ?2, ?3, ?4)",
                    rusqlite::params![format!("prt_z{n}"), message.id, source.id, json],
                )
                .unwrap();
        }
        let fork = store
            .fork_session(&source.id, new("w"), &message.id, None)
            .unwrap()
            .unwrap();
        let copied: Vec<String> = store.transcript(&fork.id).unwrap()[0]
            .parts
            .iter()
            .map(|row| row.part.stored())
            .collect();
        assert_eq!(
            copied, stored,
            "a broken compaction boundary too, which the fork reads to repoint"
        );
    }

    #[test]
    fn a_message_gone_before_its_page_is_copied_is_noticed() {
        let store = store();
        let source = store.create_session(new("w")).unwrap();
        let fork = store.create_session(new("w")).unwrap();
        let kept = store.create_message(&source.id, Role::User, None).unwrap();
        assert!(
            !store
                .copy_pages(&[kept.id.clone(), "msg_gone".into()], &fork.id)
                .unwrap(),
            "the fork would be missing history"
        );
        assert!(store.copy_pages(&[kept.id], &fork.id).unwrap());
    }

    #[test]
    fn a_fork_losing_a_selected_message_cleans_up_every_copied_page() {
        let store = store();
        let source = store.create_session(new("w")).unwrap();
        let mut ids = Vec::new();
        for _ in 0..super::FORK_PAGE + 2 {
            ids.push(store.create_message(&source.id, Role::User, None).unwrap().id);
        }
        store.lock().execute_batch(&format!(
            "CREATE TRIGGER delete_selected AFTER INSERT ON message WHEN NEW.session_id != '{}' BEGIN DELETE FROM message WHERE id = '{}'; END;",
            source.id, ids[super::FORK_PAGE],
        )).unwrap();
        assert!(
            store
                .fork_session(&source.id, new("w"), ids.last().unwrap(), None)
                .unwrap()
                .is_none()
        );
        let conn = store.lock();
        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM session", [], |row| row.get(0))
            .unwrap();
        let orphaned: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM message WHERE session_id != ?1",
                [&source.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!((sessions, orphaned), (1, 0), "no failed fork or copied messages remain");
        drop(conn);
        assert!(
            store
                .fork_session(&source.id, new("w"), "msg_gone", None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn finalization_failures_clean_up_and_cleanup_errors_do_not_hide_the_original() {
        for fail_cleanup in [false, true] {
            let store = store();
            let source = store.create_session(new("w")).unwrap();
            let message = store.create_message(&source.id, Role::User, None).unwrap();
            store.mark_read(&source.id, "a.txt").unwrap();
            store.lock().execute_batch(&format!(
                "CREATE TRIGGER fail_finalize BEFORE INSERT ON read_file WHEN NEW.session_id != '{}' BEGIN SELECT RAISE(FAIL, 'injected finalization error'); END;",
                source.id,
            )).unwrap();
            if fail_cleanup {
                store.lock().execute_batch(&format!(
                    "CREATE TRIGGER fail_cleanup BEFORE DELETE ON session WHEN OLD.id != '{}' BEGIN SELECT RAISE(FAIL, 'injected cleanup error'); END;",
                    source.id,
                )).unwrap();
            }
            let error = store.fork_session(&source.id, new("w"), &message.id, None).unwrap_err();
            assert!(error.to_string().contains("injected finalization error"), "{error}");
            let conn = store.lock();
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM session WHERE id != ?1", [&source.id], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, i64::from(fail_cleanup));
            if fail_cleanup {
                conn.execute_batch("DROP TRIGGER fail_cleanup").unwrap();
                conn.execute("DELETE FROM session WHERE id != ?1", [&source.id])
                    .unwrap();
            }
            let copied: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM message WHERE session_id != ?1",
                    [&source.id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(copied, 0);
        }
    }
}
