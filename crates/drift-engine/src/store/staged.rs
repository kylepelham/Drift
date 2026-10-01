//! Replacements in flight: a destination and the two files the engine creates beside it, recorded before they exist.

use rusqlite::params;

use super::sessions::transaction;
use super::Store;
use crate::id;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedReplacement {
    pub destination: String,
    pub staged: String,
    pub backup: String,
}

impl Store {
    /// One row, one statement: both engine-owned paths are on record before either exists.
    pub fn record_replacement(&self, replacement: &StagedReplacement) -> rusqlite::Result<()> {
        self.lock()
            .prepare_cached("INSERT OR REPLACE INTO staged_replacement(staged, destination, backup, created_at) VALUES(?1, ?2, ?3, ?4)")?
            .execute(params![replacement.staged, replacement.destination, replacement.backup, id::now_ms()])?;
        Ok(())
    }

    /// Forgets settled replacements together, in one short write.
    pub fn forget_replacements(&self, staged: &[&str]) -> rusqlite::Result<()> {
        if staged.is_empty() {
            return Ok(());
        }
        transaction(&self.lock(), |conn| {
            for path in staged {
                conn.prepare_cached("DELETE FROM staged_replacement WHERE staged = ?1")?.execute([path])?;
            }
            Ok(())
        })
    }

    pub fn replacements(&self) -> rusqlite::Result<Vec<StagedReplacement>> {
        self.lock()
            .prepare_cached("SELECT destination, staged, backup FROM staged_replacement ORDER BY created_at, staged")?
            .query_map([], |row| Ok(StagedReplacement { destination: row.get(0)?, staged: row.get(1)?, backup: row.get(2)? }))?
            .collect()
    }
}
