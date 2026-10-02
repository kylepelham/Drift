//! Content-addressed bytes (images a call returned), kept out of part JSON, events and transcript
//! loads; a part's metadata names them by hash, and `blob_ref` records which message holds each.

use rusqlite::{params, OptionalExtension};
use sha2::Digest;

use super::sessions::transaction;
use super::Store;
use crate::id;

impl Store {
    /// Stores `data` once, however often it is put, and records that `message_id` names it; returns its hash.
    pub fn put_blob(&self, message_id: &str, data: &[u8]) -> rusqlite::Result<String> {
        let hash: String = sha2::Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect();
        transaction(&self.lock(), |conn| {
            conn.prepare_cached("INSERT OR IGNORE INTO blob(hash, data, created_at) VALUES(?1, ?2, ?3)")?.execute(params![hash, data, id::now_ms()])?;
            conn.prepare_cached("INSERT OR IGNORE INTO blob_ref(hash, message_id) VALUES(?1, ?2)")?.execute(params![hash, message_id])?;
            Ok(())
        })?;
        Ok(hash)
    }

    pub fn blob(&self, hash: &str) -> rusqlite::Result<Option<Vec<u8>>> {
        self.lock().prepare_cached("SELECT data FROM blob WHERE hash = ?1")?.query_row([hash], |row| row.get(0)).optional()
    }

    /// Drops blobs no message names any more: a deleted message takes its references with it.
    pub fn prune_blobs(&self) -> rusqlite::Result<usize> {
        self.lock().execute("DELETE FROM blob WHERE NOT EXISTS (SELECT 1 FROM blob_ref WHERE blob_ref.hash = blob.hash)", [])
    }
}

#[cfg(test)]
mod tests {
    use crate::session::types::{Role, Visibility};
    use crate::store::tests::store;
    use crate::store::NewSession;

    fn new(workspace: &str) -> NewSession<'_> {
        NewSession { workspace_id: workspace, parent_id: None, visibility: Visibility::Sibling, title: "", agent: "build", model: None }
    }

    #[test]
    fn blobs_are_stored_once_and_go_when_no_message_names_them() {
        let store = store();
        let session = store.create_session(new("w")).unwrap();
        let kept = store.create_message(&session.id, Role::Assistant, None).unwrap();
        let doomed = store.create_message(&session.id, Role::Assistant, None).unwrap();
        let hash = store.put_blob(&kept.id, b"png bytes").unwrap();
        assert_eq!(store.put_blob(&doomed.id, b"png bytes").unwrap(), hash, "the same bytes, the same blob");
        let orphan = store.put_blob(&doomed.id, b"only the doomed one").unwrap();
        store.lock().execute("DELETE FROM message WHERE id = ?1", [&doomed.id]).unwrap();
        assert_eq!(store.prune_blobs().unwrap(), 1);
        assert_eq!(store.blob(&hash).unwrap().as_deref(), Some(&b"png bytes"[..]), "still named by the kept message");
        assert!(store.blob(&orphan).unwrap().is_none());
    }

    #[test]
    fn a_forks_copy_keeps_an_image_its_source_no_longer_has() {
        let store = store();
        let source = store.create_session(new("w")).unwrap();
        let mut message = store.create_message(&source.id, Role::Assistant, None).unwrap();
        message.status = crate::session::types::MessageStatus::Done;
        store.save_message(&message).unwrap();
        let hash = store.put_blob(&message.id, b"screenshot").unwrap();
        let fork = store.fork_session(&source.id, new("w"), &message.id, None).unwrap();
        assert_eq!(store.transcript(&fork.id).unwrap().len(), 1, "the message was copied");
        store.lock().execute("DELETE FROM message WHERE session_id = ?1", [&source.id]).unwrap();
        store.prune_blobs().unwrap();
        assert!(store.blob(&hash).unwrap().is_some());
    }
}
