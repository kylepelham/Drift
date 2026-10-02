//! Content-addressed bytes (images a call returned), kept out of part JSON, events and transcript
//! loads; a part's metadata names them by hash.

use rusqlite::{params, OptionalExtension};
use sha2::Digest;

use super::Store;
use crate::id;

/// A blob no part names yet may belong to a call still settling; it is kept this long first.
const UNREFERENCED_GRACE_MS: i64 = 60 * 60 * 1000;

impl Store {
    /// Stores `data` once, however often it is put; returns its hash.
    pub fn put_blob(&self, data: &[u8]) -> rusqlite::Result<String> {
        let hash: String = sha2::Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect();
        self.lock().prepare_cached("INSERT OR IGNORE INTO blob(hash, data, created_at) VALUES(?1, ?2, ?3)")?.execute(params![hash, data, id::now_ms()])?;
        Ok(hash)
    }

    pub fn blob(&self, hash: &str) -> rusqlite::Result<Option<Vec<u8>>> {
        self.lock().prepare_cached("SELECT data FROM blob WHERE hash = ?1")?.query_row([hash], |row| row.get(0)).optional()
    }

    /// Drops blobs no stored part names, once they are old enough not to belong to a call still settling.
    pub fn prune_blobs(&self) -> rusqlite::Result<usize> {
        self.lock().execute(
            "DELETE FROM blob WHERE created_at < ?1 AND NOT EXISTS (SELECT 1 FROM part WHERE instr(part.json, blob.hash) > 0)",
            [id::now_ms() - UNREFERENCED_GRACE_MS],
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::session::types::{Part, Role, Visibility};
    use crate::store::tests::store;
    use crate::store::NewSession;

    fn new(workspace: &str) -> NewSession<'_> {
        NewSession { workspace_id: workspace, parent_id: None, visibility: Visibility::Sibling, title: "", agent: "build", model: None }
    }

    #[test]
    fn blobs_are_stored_once_and_pruned_only_when_nothing_names_them() {
        let store = store();
        let hash = store.put_blob(b"png bytes").unwrap();
        assert_eq!(store.put_blob(b"png bytes").unwrap(), hash, "the same bytes, the same blob");
        assert_eq!(store.blob(&hash).unwrap().as_deref(), Some(&b"png bytes"[..]));
        let session = store.create_session(new("w")).unwrap();
        let message = store.create_message(&session.id, Role::Assistant, None).unwrap();
        store.add_part(&message.id, &session.id, Part::Text { text: format!("image {hash}") }).unwrap();
        let orphan = store.put_blob(b"nobody").unwrap();
        store.lock().execute("UPDATE blob SET created_at = 0", []).unwrap();
        assert_eq!(store.prune_blobs().unwrap(), 1);
        assert!(store.blob(&hash).unwrap().is_some() && store.blob(&orphan).unwrap().is_none());
    }
}
