//! Reports space used by drift.db, per-workspace shadow-git undo history, and spooled shell output.
//! Runs cleanup and compaction; the engine also cleans these stores every few hours through Engine::clean_up.
//! Uses row counts and file sizes directly; payload sizes are estimated from stratified samples to avoid full scans.

use crate::store::Store;
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A scan waits this long for the engine's writer rather than failing at once.
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);
/// Rows read per sampling stratum, and how many strata to spread across the table.
const SAMPLE_ROWS_PER_STRATUM: i64 = 200;
const SAMPLE_STRATA: i64 = 12;
/// Tables whose payload column holds the bulk of the database: transcripts and pasted images.
const PAYLOAD_TABLES: [(&str, &str); 2] = [("part", "json"), ("blob", "data")];
/// The engine's folders beside the database, by the name the UI shows them under.
const FOLDERS: [(&str, &str); 2] = [("undo", "snapshots"), ("output", "tool-output")];

#[derive(Debug, thiserror::Error)]
pub(crate) enum StorageError {
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error("could not compact the database (it is in use): {0}")]
    Compact(#[source] rusqlite::Error),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TableUsage {
    pub table: String,
    pub rows: i64,
    /// Estimated for a table (row count times a sampled mean row size), exact for a folder.
    pub bytes: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionCounts {
    pub total: i64,
    pub top_level: i64,
    pub subagent: i64,
    pub archived: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageStats {
    pub path: String,
    /// The database with its log, and the engine's folders.
    pub total_bytes: i64,
    /// Pages already free inside the database file. Reclaimed by compacting.
    pub free_bytes: i64,
    pub tables: Vec<TableUsage>,
    pub sessions: SessionCounts,
    /// True when table figures come from sampling rather than a full scan.
    pub estimated: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PruneResult {
    /// Count of removed images that were no longer referenced.
    pub removed_rows: i64,
    /// How much smaller the database and the engine's folders are now.
    pub freed_bytes: i64,
    /// Free space now available for reuse inside the database file.
    pub free_bytes: i64,
}

/// Where the engine keeps its database and folders.
pub(crate) struct Location {
    pub data_dir: PathBuf,
}

impl Location {
    fn database(&self) -> PathBuf {
        self.data_dir.join("drift.db")
    }

    /// The database, its write-ahead log and the engine's folders.
    fn total_bytes(&self) -> i64 {
        let database = self.database();
        let log = PathBuf::from(format!("{}-wal", database.display()));
        file_bytes(&database)
            + file_bytes(&log)
            + FOLDERS
                .iter()
                .map(|(_, folder)| folder_bytes(&self.data_dir.join(folder)))
                .sum::<i64>()
    }
}

fn file_bytes(path: &Path) -> i64 {
    std::fs::metadata(path).map_or(0, |meta| meta.len() as i64)
}

fn folder_bytes(path: &Path) -> i64 {
    let Ok(entries) = std::fs::read_dir(path) else { return 0 };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => folder_bytes(&entry.path()),
            _ => entry.metadata().map_or(0, |meta| meta.len() as i64),
        })
        .sum()
}

fn open(database: &Path, read_only: bool) -> Result<Connection, StorageError> {
    let flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    };
    let connection = Connection::open_with_flags(database, flags)?;
    connection.busy_timeout(BUSY_TIMEOUT)?;

    Ok(connection)
}

fn scalar(connection: &Connection, sql: &str) -> Result<i64, StorageError> {
    let value = connection.query_row(sql, [], |row| row.get::<_, Option<i64>>(0))?;
    Ok(value.unwrap_or(0))
}

fn free_bytes(connection: &Connection) -> Result<i64, StorageError> {
    let pages = scalar(connection, "PRAGMA freelist_count")?;
    let page_size = scalar(connection, "PRAGMA page_size")?;

    Ok(pages * page_size)
}

/// Mean payload size for a column, sampled from evenly spaced windows of the table.
fn sampled_mean_bytes(connection: &Connection, table: &str, column: &str) -> Result<f64, StorageError> {
    let max_rowid = scalar(connection, &format!("SELECT MAX(rowid) FROM \"{table}\""))?;
    if max_rowid == 0 {
        return Ok(0.0);
    }

    let stride = (max_rowid / SAMPLE_STRATA).max(1);
    let mut total = 0i64;
    let mut rows = 0i64;

    for stratum in 0..SAMPLE_STRATA {
        let sql = format!(
            "SELECT COALESCE(SUM(LENGTH(CAST(\"{column}\" AS BLOB))), 0), COUNT(*) FROM (
                 SELECT \"{column}\" FROM \"{table}\" WHERE rowid >= ?1 LIMIT ?2
             )"
        );
        let (bytes, counted): (i64, i64) =
            connection.query_row(&sql, (stratum * stride, SAMPLE_ROWS_PER_STRATUM), |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
        total += bytes;
        rows += counted;
    }

    Ok(if rows == 0 { 0.0 } else { total as f64 / rows as f64 })
}

/// A conversation counts as archived when the engine marks it or Drift's archive list names it.
fn session_counts(connection: &Connection, archived: &[String]) -> Result<SessionCounts, StorageError> {
    let listed = quote_list(archived);
    let extra = if listed.is_empty() {
        String::new()
    } else {
        format!(" OR id IN ({listed})")
    };
    Ok(SessionCounts {
        total: scalar(connection, "SELECT COUNT(*) FROM session")?,
        top_level: scalar(connection, "SELECT COUNT(*) FROM session WHERE visibility = 'sibling'")?,
        subagent: scalar(connection, "SELECT COUNT(*) FROM session WHERE visibility = 'hidden'")?,
        archived: scalar(
            connection,
            &format!("SELECT COUNT(*) FROM session WHERE archived_at IS NOT NULL{extra}"),
        )?,
    })
}

/// Renders ids as a SQL list; anything that is not a plain identifier is dropped, so nothing can be injected.
fn quote_list(ids: &[String]) -> String {
    ids.iter()
        .filter(|id| {
            id.chars()
                .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        })
        .map(|id| format!("'{id}'"))
        .collect::<Vec<_>>()
        .join(",")
}

pub(crate) fn stats(location: &Location, archived: &[String]) -> Result<StorageStats, StorageError> {
    let connection = open(&location.database(), true)?;
    let mut tables = Vec::new();

    for (table, column) in PAYLOAD_TABLES {
        let rows = scalar(&connection, &format!("SELECT COUNT(*) FROM \"{table}\""))?;
        let mean = sampled_mean_bytes(&connection, table, column)?;
        tables.push(TableUsage {
            table: table.into(),
            rows,
            bytes: (rows as f64 * mean) as i64,
        });
    }

    for (name, folder) in FOLDERS {
        tables.push(TableUsage {
            table: name.into(),
            rows: 0,
            bytes: folder_bytes(&location.data_dir.join(folder)),
        });
    }

    Ok(StorageStats {
        path: location.database().to_string_lossy().into_owned(),
        total_bytes: location.total_bytes(),
        free_bytes: free_bytes(&connection)?,
        tables,
        sessions: session_counts(&connection, archived)?,
        estimated: true,
    })
}

/// Cleanup counts and byte changes measured before and after engine housekeeping.
pub(crate) fn cleaned(location: &Location, before: i64, images: usize) -> Result<PruneResult, StorageError> {
    let connection = open(&location.database(), true)?;

    Ok(PruneResult {
        removed_rows: images as i64,
        freed_bytes: (before - location.total_bytes()).max(0),
        free_bytes: free_bytes(&connection)?,
    })
}

pub(crate) fn total_bytes(location: &Location) -> i64 {
    location.total_bytes()
}

/// Rewrites the database to return free pages to disk.
/// The caller refuses during a conversation because the rewrite holds the database for its whole duration.
pub(crate) fn compact(location: &Location) -> Result<PruneResult, StorageError> {
    let before = location.total_bytes();
    let connection = open(&location.database(), false)?;
    connection.execute_batch("VACUUM").map_err(StorageError::Compact)?;

    // Checkpoint the VACUUM writes so the database file releases its unused pages on disk.
    let _ = connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));

    Ok(PruneResult {
        removed_rows: 0,
        freed_bytes: (before - location.total_bytes()).max(0),
        free_bytes: free_bytes(&connection)?,
    })
}

/// Session ids Drift has archived, counted as archived beside the engine's own flag.
pub(crate) fn archived_ids(store: &Store) -> Vec<String> {
    store
        .archived()
        .map(|rows| rows.into_iter().map(|row| row.session_id).collect())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
