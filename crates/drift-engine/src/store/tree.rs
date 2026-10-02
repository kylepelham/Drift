//! Whole-session operations: copying a transcript into a fork, moving a session with its subagents.

use std::collections::HashMap;

use rusqlite::types::Type;
use rusqlite::{params, Connection};

use super::sessions::{insert_session, session_from, transaction, NewSession};
use super::Store;
use crate::id;

/// Messages copied per transaction when forking.
const FORK_PAGE: usize = 100;
use crate::session::types::{Part, Session};

impl Store {
    /// A new session holding copies of the source's finished messages up to and including `through`;
    /// a spawn records it as its `cutoff`. The copy goes [`FORK_PAGE`] messages per transaction, so a
    /// long history never holds the database for long; the fork stays archived, out of every list,
    /// until the last page lands, and one a crash cut short is purged with the other archived sessions.
    pub fn fork_session(&self, source_id: &str, new: NewSession, through: &str, cutoff: Option<&str>) -> rusqlite::Result<Session> {
        let session = session_from(new, cutoff);
        let messages: Vec<String> = transaction(&self.lock(), |conn| {
            insert_session(conn, &session)?;
            conn.prepare_cached("UPDATE session SET variant = (SELECT variant FROM session WHERE id = ?2), archived_at = ?3 WHERE id = ?1")?.execute(params![session.id, source_id, id::now_ms()])?;
            conn.prepare_cached("SELECT id FROM message WHERE session_id = ?1 AND id <= ?2 AND status != 'streaming' ORDER BY id")?
                .query_map(params![source_id, through], |row| row.get(0))?
                .collect()
        })?;
        let mut copies = HashMap::new();
        for page in messages.chunks(FORK_PAGE) {
            transaction(&self.lock(), |conn| page.iter().try_for_each(|message| copy_message(conn, message, &session.id, &mut copies)))?;
        }
        let session = transaction(&self.lock(), |conn| {
            super::reads::copy_reads(conn, source_id, &session.id, through)?;
            conn.prepare_cached("UPDATE session SET archived_at = NULL WHERE id = ?1")?.execute([&session.id])?;
            super::sessions::session_in(conn, &session.id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
        })?;
        Ok(session)
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
            let Ok(Part::ToolCall { metadata: Some(metadata), .. }) = serde_json::from_str::<Part>(&json) else { continue };
            let Some(changes) = metadata.get("changes").and_then(|c| c.as_array()) else { continue };
            let owner = metadata["owner"].as_str().map_or(session_workspace, str::to_string);
            let kept = blobs.entry(owner).or_default();
            for change in changes {
                kept.extend(["before", "after"].iter().filter_map(|side| change[side].as_str().map(str::to_string)));
            }
        }
        Ok(blobs.into_iter().map(|(owner, set)| (owner, set.into_iter().collect())).collect())
    }

    pub fn move_sessions(&self, ids: &[String], workspace_id: &str) -> rusqlite::Result<()> {
        transaction(&self.lock(), |conn| {
            let mut update = conn.prepare_cached("UPDATE session SET workspace_id = ?2, updated_at = ?3 WHERE id = ?1")?;
            let now = id::now_ms();
            for session in ids {
                update.execute(params![session, workspace_id, now])?;
            }
            Ok(())
        })
    }
}

/// `copies` maps source message ids to their copies, so a compaction boundary keeps pointing at its
/// own tail. Parts are copied inside SQLite; only a compaction boundary, which names a message, is
/// read and rewritten. A message removed meanwhile is skipped.
fn copy_message(conn: &Connection, source: &str, session_id: &str, copies: &mut HashMap<String, String>) -> rusqlite::Result<()> {
    let message_id = id::new("msg");
    let copied = conn
        .prepare_cached(
            "INSERT INTO message(id, session_id, role, status, model_provider, model_id, usage_json, cost, error, created_at, finished_at, summary, agent, ending)
             SELECT ?1, ?2, role, status, model_provider, model_id, usage_json, cost, error, created_at, finished_at, summary, agent, ending FROM message WHERE id = ?3",
        )?
        .execute(params![message_id, session_id, source])?;
    if copied == 0 {
        return Ok(());
    }
    copies.insert(source.to_string(), message_id.clone());
    // The copy names the same images, so they stay as long as either message does.
    conn.prepare_cached("INSERT INTO blob_ref(hash, message_id) SELECT hash, ?1 FROM blob_ref WHERE message_id = ?2")?.execute(params![message_id, source])?;
    let parts: Vec<(String, bool)> = conn
        .prepare_cached("SELECT id, json_extract(json, '$.type') = 'compaction' FROM part WHERE message_id = ?1 ORDER BY id")?
        .query_map([source], |row| Ok((row.get(0)?, row.get::<_, Option<bool>>(1)?.unwrap_or(false))))?
        .collect::<rusqlite::Result<_>>()?;
    for (part, boundary) in parts {
        let copy = id::new("prt");
        if boundary {
            copy_boundary(conn, &part, &copy, &message_id, session_id, copies)?;
            continue;
        }
        conn.prepare_cached("INSERT INTO part(id, message_id, session_id, json) SELECT ?1, ?2, ?3, json FROM part WHERE id = ?4")?.execute(params![copy, message_id, session_id, part])?;
    }
    Ok(())
}

/// A compaction boundary, pointed at the copy of the tail it kept.
fn copy_boundary(conn: &Connection, part: &str, copy: &str, message_id: &str, session_id: &str, copies: &HashMap<String, String>) -> rusqlite::Result<()> {
    let json: String = conn.prepare_cached("SELECT json FROM part WHERE id = ?1")?.query_row([part], |row| row.get(0))?;
    let mut parsed: Part = serde_json::from_str(&json).map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(e)))?;
    if let Part::Compaction { tail_from, .. } = &mut parsed {
        *tail_from = tail_from.as_ref().and_then(|tail| copies.get(tail).cloned());
    }
    conn.prepare_cached("INSERT INTO part(id, message_id, session_id, json) VALUES(?1, ?2, ?3, ?4)")?.execute(params![copy, message_id, session_id, serde_json::to_string(&parsed).unwrap()])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::session::types::{MessageStatus, Part, Role, Visibility};
    use crate::store::tests::store;
    use crate::store::NewSession;

    fn new(workspace: &str) -> NewSession<'_> {
        NewSession { workspace_id: workspace, parent_id: None, visibility: Visibility::Sibling, title: "", agent: "build", model: None }
    }

    #[test]
    fn a_long_history_is_copied_page_by_page_with_its_compaction_boundary_rewritten() {
        let store = store();
        let source = store.create_session(new("w")).unwrap();
        let mut ids = Vec::new();
        for i in 0..(super::FORK_PAGE * 2 + 30) {
            let mut message = store.create_message(&source.id, if i % 2 == 0 { Role::User } else { Role::Assistant }, None).unwrap();
            message.status = MessageStatus::Done;
            store.save_message(&message).unwrap();
            store.add_part(&message.id, &source.id, Part::Text { text: format!("m{i}") }).unwrap();
            ids.push(message.id);
        }
        let boundary = store.create_message(&source.id, Role::User, None).unwrap();
        store.add_part(&boundary.id, &source.id, Part::Compaction { auto: true, tail_from: Some(ids[3].clone()) }).unwrap();
        let fork = store.fork_session(&source.id, new("w"), &boundary.id, None).unwrap();
        assert!(fork.archived_at.is_none(), "listed once every page landed");
        let copied = store.transcript(&fork.id).unwrap();
        assert_eq!(copied.len(), ids.len() + 1);
        assert!(matches!(&copied[0].parts[0].part, Part::Text { text } if text == "m0"));
        let Part::Compaction { tail_from: Some(tail), .. } = &copied.last().unwrap().parts[0].part else { panic!("the boundary was copied") };
        assert_eq!(tail, &copied[3].info.id, "it points at the copy of its tail, pages earlier");
    }
}
