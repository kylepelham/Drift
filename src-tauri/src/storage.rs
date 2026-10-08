//! What Drift keeps on disk, and room to give some back.
//!
//! The engine stores transcripts in `drift.db` beside two folders: its undo history (a shadow git
//! repository per workspace) and spooled shell output. It already cleans all three every few hours
//! (`Engine::clean_up`), so this screen reports sizes, runs that cleanup on request, and compacts.
//!
//! Row counts and file sizes are instant, but summing payload lengths over a large `part` table is
//! not, so table sizes are estimated from a stratified sample.

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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableUsage {
    pub table: String,
    pub rows: i64,
    /// Estimated for a table (row count times a sampled mean row size), exact for a folder.
    pub bytes: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCounts {
    pub total: i64,
    pub top_level: i64,
    pub subagent: i64,
    pub archived: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStats {
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
pub struct PruneResult {
    /// Images nothing referred to any more.
    pub removed_rows: i64,
    /// How much smaller the database and the engine's folders are now.
    pub freed_bytes: i64,
    /// Free space now available for reuse inside the database file.
    pub free_bytes: i64,
}

/// Where the engine keeps its database and folders.
pub struct Location {
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
    std::fs::metadata(path).map(|meta| meta.len() as i64).unwrap_or(0)
}

fn folder_bytes(path: &Path) -> i64 {
    let Ok(entries) = std::fs::read_dir(path) else { return 0 };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => folder_bytes(&entry.path()),
            _ => entry.metadata().map(|meta| meta.len() as i64).unwrap_or(0),
        })
        .sum()
}

fn open(database: &Path, read_only: bool) -> Result<Connection, String> {
    let flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    };
    let conn = Connection::open_with_flags(database, flags).map_err(|error| error.to_string())?;
    conn.busy_timeout(BUSY_TIMEOUT).map_err(|error| error.to_string())?;
    Ok(conn)
}

fn scalar(conn: &Connection, sql: &str) -> Result<i64, String> {
    conn.query_row(sql, [], |row| row.get::<_, Option<i64>>(0))
        .map(|value| value.unwrap_or(0))
        .map_err(|error| error.to_string())
}

fn free_bytes(conn: &Connection) -> Result<i64, String> {
    Ok(scalar(conn, "PRAGMA freelist_count")? * scalar(conn, "PRAGMA page_size")?)
}

/// Mean payload size for a column, sampled from evenly spaced windows of the table.
fn sampled_mean_bytes(conn: &Connection, table: &str, column: &str) -> Result<f64, String> {
    let max_rowid = scalar(conn, &format!("SELECT MAX(rowid) FROM \"{table}\""))?;
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
        let (bytes, counted): (i64, i64) = conn
            .query_row(&sql, (stratum * stride, SAMPLE_ROWS_PER_STRATUM), |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|error| error.to_string())?;
        total += bytes;
        rows += counted;
    }
    Ok(if rows == 0 { 0.0 } else { total as f64 / rows as f64 })
}

/// A conversation counts as archived when the engine marks it or Drift's archive list names it.
fn session_counts(conn: &Connection, archived: &[String]) -> Result<SessionCounts, String> {
    let listed = quote_list(archived);
    let extra = if listed.is_empty() {
        String::new()
    } else {
        format!(" OR id IN ({listed})")
    };
    Ok(SessionCounts {
        total: scalar(conn, "SELECT COUNT(*) FROM session")?,
        top_level: scalar(conn, "SELECT COUNT(*) FROM session WHERE visibility = 'sibling'")?,
        subagent: scalar(conn, "SELECT COUNT(*) FROM session WHERE visibility = 'hidden'")?,
        archived: scalar(
            conn,
            &format!("SELECT COUNT(*) FROM session WHERE archived_at IS NOT NULL{extra}"),
        )?,
    })
}

/// Renders ids as a SQL list; anything that is not a plain identifier is dropped, so nothing can be injected.
fn quote_list(ids: &[String]) -> String {
    ids.iter()
        .filter(|id| id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .map(|id| format!("'{id}'"))
        .collect::<Vec<_>>()
        .join(",")
}

pub fn stats(location: &Location, archived: &[String]) -> Result<StorageStats, String> {
    let conn = open(&location.database(), true)?;
    let mut tables = Vec::new();
    for (table, column) in PAYLOAD_TABLES {
        let rows = scalar(&conn, &format!("SELECT COUNT(*) FROM \"{table}\""))?;
        let mean = sampled_mean_bytes(&conn, table, column)?;
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
        free_bytes: free_bytes(&conn)?,
        tables,
        sessions: session_counts(&conn, archived)?,
        estimated: true,
    })
}

/// What a cleanup took away, measured around it.
pub fn cleaned(location: &Location, before: i64, images: usize) -> Result<PruneResult, String> {
    let conn = open(&location.database(), true)?;
    Ok(PruneResult {
        removed_rows: images as i64,
        freed_bytes: (before - location.total_bytes()).max(0),
        free_bytes: free_bytes(&conn)?,
    })
}

pub fn total_bytes(location: &Location) -> i64 {
    location.total_bytes()
}

/// Rewrites the database to give its free pages back to the disk. The caller refuses while a
/// conversation runs: the rewrite holds the database for its whole length.
pub fn compact(location: &Location) -> Result<PruneResult, String> {
    let before = location.total_bytes();
    let conn = open(&location.database(), false)?;
    conn.execute_batch("VACUUM")
        .map_err(|error| format!("could not compact the database (it is in use): {error}"))?;
    // The rewrite went through the log; folding it back is what makes the file smaller on disk.
    let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    Ok(PruneResult {
        removed_rows: 0,
        freed_bytes: (before - location.total_bytes()).max(0),
        free_bytes: free_bytes(&conn)?,
    })
}

/// Session ids Drift has archived, counted as archived beside the engine's own flag.
pub fn archived_ids(store: &Store) -> Vec<String> {
    store
        .archived()
        .map(|rows| rows.into_iter().map(|row| row.session_id).collect())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
