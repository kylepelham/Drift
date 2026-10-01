//! Files the engine creates beside a file it is replacing, recorded before they exist.

use rusqlite::params;

use super::Store;
use crate::id;

impl Store {
    pub fn record_staged(&self, paths: &[&str]) -> rusqlite::Result<()> {
        let conn = self.lock();
        for path in paths {
            conn.prepare_cached("INSERT OR REPLACE INTO staged_file(path, created_at) VALUES(?1, ?2)")?.execute(params![path, id::now_ms()])?;
        }
        Ok(())
    }

    pub fn forget_staged(&self, paths: &[&str]) -> rusqlite::Result<()> {
        let conn = self.lock();
        for path in paths {
            conn.prepare_cached("DELETE FROM staged_file WHERE path = ?1")?.execute([path])?;
        }
        Ok(())
    }

    pub fn staged_files(&self) -> rusqlite::Result<Vec<String>> {
        self.lock().prepare_cached("SELECT path FROM staged_file ORDER BY path")?.query_map([], |row| row.get(0))?.collect()
    }
}
