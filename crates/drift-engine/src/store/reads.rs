//! The files each session has read, kept so edits stay allowed after the engine restarts.

use rusqlite::params;

use super::Store;

impl Store {
    pub fn mark_read(&self, session_id: &str, path: &str) -> rusqlite::Result<()> {
        self.lock().prepare_cached("INSERT OR IGNORE INTO read_file(session_id, path) VALUES(?1, ?2)")?.execute(params![session_id, path])?;
        Ok(())
    }

    pub fn read_files(&self, session_id: &str) -> rusqlite::Result<Vec<String>> {
        let conn = self.lock();
        let mut statement = conn.prepare_cached("SELECT path FROM read_file WHERE session_id = ?1")?;
        let paths = statement.query_map([session_id], |row| row.get(0))?.collect();
        paths
    }
}

#[cfg(test)]
mod tests {
    use crate::session::types::Visibility;
    use crate::store::tests::store;
    use crate::store::NewSession;

    #[test]
    fn reads_are_kept_per_session_and_go_with_it() {
        let store = store();
        let session = store.create_session(NewSession { workspace_id: "w", parent_id: None, visibility: Visibility::Sibling, title: "", agent: "build", model: None }).unwrap();
        store.mark_read(&session.id, "C:/repo/a.rs").unwrap();
        store.mark_read(&session.id, "C:/repo/a.rs").unwrap();
        assert_eq!(store.read_files(&session.id).unwrap(), ["C:/repo/a.rs"]);
        store.lock().execute("DELETE FROM session WHERE id = ?1", [&session.id]).unwrap();
        assert!(store.read_files(&session.id).unwrap().is_empty());
    }
}
