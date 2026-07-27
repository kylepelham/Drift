//! Reports and reclaims space in the OpenCode session database.
//!
//! The engine is event sourced: `event` is an append-only log and `message` / `part` are projections
//! derived from it. Nothing trims the log, and `message.part.updated` / `message.updated` events
//! each store a *full* JSON snapshot rather than a delta, so one streamed reply leaves behind an
//! ever-larger copy per update. The log therefore grows to several times the size of the data it
//! describes.
//!
//! Transcripts are read from the projections, not by replaying the log, so superseded snapshots can
//! be removed without changing anything the user can see.
//!
//! Two cost tiers matter here. Row counts and file size are instant, but summing payload lengths
//! over a million-row table takes the better part of a minute. `stats` therefore *estimates* bytes
//! from a stratified sample so the settings tab opens immediately, and `analyze` does the exact
//! (slow) accounting only when the user asks for it.

use crate::store::Store;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// The engine may hold a write lock while streaming; wait rather than fail immediately.
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);
/// Rows read per sampling stratum, and how many strata to spread across the table.
const SAMPLE_ROWS_PER_STRATUM: i64 = 200;
const SAMPLE_STRATA: i64 = 12;

/// Tables whose `data` column holds the bulk of the database.
const PAYLOAD_TABLES: [(&str, &str); 3] = [("event", "data"), ("part", "data"), ("message", "data")];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableUsage {
    pub table: String,
    pub rows: i64,
    /// Estimated payload bytes: exact row count multiplied by a sampled mean row size.
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
    /// Size of the database file on disk.
    pub total_bytes: i64,
    /// Pages already free inside the file. Reclaimed by compacting, not by pruning.
    pub free_bytes: i64,
    pub tables: Vec<TableUsage>,
    pub sessions: SessionCounts,
    /// True when byte figures come from sampling rather than a full scan.
    pub estimated: bool,
}

/// Exact reclaimable bytes per rule. Rules overlap, so these are upper bounds, not a sum.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleEstimate {
    pub rule: String,
    pub rows: i64,
    pub bytes: i64,
}

/// Which prune rules to apply. Each maps to one rule in `rules_for`.
#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub struct PruneRules {
    /// Remove all but the newest snapshot per part and per message.
    pub superseded_snapshots: bool,
    /// Remove the event log of subagent (child) sessions; their summary lives in the parent.
    pub subagent_events: bool,
    /// Remove the event log of archived sessions.
    pub archived_events: bool,
    /// Remove events whose session no longer exists.
    pub orphan_events: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneResult {
    pub removed_rows: i64,
    /// Change in file size. Zero unless the database was also compacted, because deleted pages stay
    /// in the file as free space until then.
    pub freed_bytes: i64,
    /// Free space now available for reuse inside the file.
    pub free_bytes: i64,
}

fn database_path() -> Result<PathBuf, String> {
    crate::engine_db::database_path(true)
}

fn open(read_only: bool) -> Result<Connection, String> {
    let path = database_path()?;
    let flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    };
    let conn = Connection::open_with_flags(&path, flags).map_err(|error| error.to_string())?;
    conn.busy_timeout(BUSY_TIMEOUT)
        .map_err(|error| error.to_string())?;
    conn.pragma_update(None, "foreign_keys", true)
        .map_err(|error| error.to_string())?;
    Ok(conn)
}

fn scalar(conn: &Connection, sql: &str) -> Result<i64, String> {
    conn.query_row(sql, [], |row| row.get::<_, Option<i64>>(0))
        .map(|value| value.unwrap_or(0))
        .map_err(|error| error.to_string())
}

/// Moves the write-ahead log back into the database and releases the log file.
///
/// A large delete grows the WAL to hold every modified page, and SQLite then reuses that space
/// rather than shrinking the file. Without this a caller who just freed several gigabytes is left
/// with a multi-gigabyte `-wal` beside the database and no apparent saving. Checkpointing is
/// best effort: another reader can hold the log open, in which case it is released later.
fn checkpoint(conn: &Connection) {
    let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
}

/// Mean payload size for a column, sampled from evenly spaced windows.
///
/// A full `SUM(LENGTH(...))` costs a minute on the `event` table. Reading a few hundred rows from
/// each of a dozen positions costs milliseconds and is close enough to size a bar chart, while
/// avoiding the bias of sampling only the newest or oldest rows.
fn sampled_mean_bytes(conn: &Connection, table: &str, column: &str) -> Result<f64, String> {
    let max_rowid = scalar(conn, &format!("SELECT MAX(rowid) FROM \"{table}\""))?;
    if max_rowid == 0 {
        return Ok(0.0);
    }
    let stride = (max_rowid / SAMPLE_STRATA).max(1);
    let mut total = 0i64;
    let mut rows = 0i64;
    for stratum in 0..SAMPLE_STRATA {
        let start = stratum * stride;
        let sql = format!(
            "SELECT COALESCE(SUM(LENGTH(CAST(\"{column}\" AS BLOB))), 0), COUNT(*) FROM (
                 SELECT \"{column}\" FROM \"{table}\" WHERE rowid >= ?1 LIMIT ?2
             )"
        );
        let (bytes, counted): (i64, i64) = conn
            .query_row(&sql, (start, SAMPLE_ROWS_PER_STRATUM), |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|error| error.to_string())?;
        total += bytes;
        rows += counted;
    }
    Ok(if rows == 0 {
        0.0
    } else {
        total as f64 / rows as f64
    })
}

fn session_counts(conn: &Connection, archived: &[String]) -> Result<SessionCounts, String> {
    // A session counts as archived if either archive marks it: the engine's own `time_archived`, or
    // Drift's separate archive list in drift.db. The two overlap but are not the same set.
    let engine_archived = scalar(conn, "SELECT COUNT(*) FROM session WHERE time_archived IS NOT NULL")?;
    let archived_total = if archived.is_empty() {
        engine_archived
    } else {
        let list = quote_list(archived);
        scalar(
            conn,
            &format!(
                "SELECT COUNT(*) FROM session WHERE time_archived IS NOT NULL OR id IN ({list})"
            ),
        )?
    };
    Ok(SessionCounts {
        total: scalar(conn, "SELECT COUNT(*) FROM session")?,
        top_level: scalar(conn, "SELECT COUNT(*) FROM session WHERE parent_id IS NULL")?,
        subagent: scalar(conn, "SELECT COUNT(*) FROM session WHERE parent_id IS NOT NULL")?,
        archived: archived_total,
    })
}

/// Renders ids as a SQL list. Session ids are engine-generated and validated below, so this cannot
/// carry an injected fragment.
fn quote_list(ids: &[String]) -> String {
    ids.iter()
        .filter(|id| id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .map(|id| format!("'{id}'"))
        .collect::<Vec<_>>()
        .join(",")
}

pub fn stats(archived: &[String]) -> Result<StorageStats, String> {
    let path = database_path()?;
    let total_bytes = std::fs::metadata(&path)
        .map(|meta| meta.len() as i64)
        .unwrap_or(0);
    let conn = open(true)?;
    let page_size = scalar(&conn, "PRAGMA page_size")?;
    let free_bytes = scalar(&conn, "PRAGMA freelist_count")? * page_size;

    let mut tables = Vec::new();
    for (table, column) in PAYLOAD_TABLES {
        let rows = scalar(&conn, &format!("SELECT COUNT(*) FROM \"{table}\""))?;
        let mean = sampled_mean_bytes(&conn, table, column)?;
        tables.push(TableUsage {
            table: table.to_string(),
            rows,
            bytes: (rows as f64 * mean) as i64,
        });
    }

    Ok(StorageStats {
        path: path.to_string_lossy().to_string(),
        total_bytes,
        free_bytes,
        tables,
        sessions: session_counts(&conn, archived)?,
        estimated: true,
    })
}

/// SQL selecting the rowids each rule would delete, paired with the rule name.
fn rules_for(rules: PruneRules, archived: &[String]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if rules.superseded_snapshots {
        for (event_type, id_path) in [
            ("message.part.updated.1", "$.part.id"),
            ("message.updated.1", "$.info.id"),
        ] {
            out.push((
                format!("superseded:{event_type}"),
                format!(
                    "SELECT rowid FROM (
                         SELECT rowid, ROW_NUMBER() OVER (
                             PARTITION BY json_extract(data, '{id_path}') ORDER BY seq DESC
                         ) AS rank
                         FROM event WHERE type = '{event_type}'
                             AND json_extract(data, '{id_path}') IS NOT NULL
                     ) WHERE rank > 1"
                ),
            ));
        }
    }
    if rules.subagent_events {
        out.push((
            "subagent-events".into(),
            "SELECT rowid FROM event WHERE aggregate_id IN
                 (SELECT id FROM session WHERE parent_id IS NOT NULL)"
                .into(),
        ));
    }
    if rules.archived_events {
        let extra = if archived.is_empty() {
            String::new()
        } else {
            format!(" OR id IN ({})", quote_list(archived))
        };
        out.push((
            "archived-events".into(),
            format!(
                "SELECT rowid FROM event WHERE aggregate_id IN
                     (SELECT id FROM session WHERE time_archived IS NOT NULL{extra})"
            ),
        ));
    }
    if rules.orphan_events {
        out.push((
            "orphan-events".into(),
            "SELECT rowid FROM event WHERE aggregate_id NOT IN (SELECT id FROM session)".into(),
        ));
    }
    out
}

/// Exact reclaimable rows and bytes per rule. Scans the event table, so this is slow by design.
pub fn analyze(archived: &[String]) -> Result<Vec<RuleEstimate>, String> {
    let conn = open(true)?;
    let all = PruneRules {
        superseded_snapshots: true,
        subagent_events: true,
        archived_events: true,
        orphan_events: true,
    };
    rules_for(all, archived)
        .into_iter()
        .map(|(rule, selector)| {
            let sql = format!(
                "SELECT COUNT(*), COALESCE(SUM(LENGTH(CAST(data AS BLOB))), 0)
                 FROM event WHERE rowid IN ({selector})"
            );
            let (rows, bytes): (i64, i64) = conn
                .query_row(&sql, [], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|error| error.to_string())?;
            Ok(RuleEstimate { rule, rows, bytes })
        })
        .collect()
}

pub fn prune(rules: PruneRules, archived: &[String]) -> Result<PruneResult, String> {
    let path = database_path()?;
    let before = std::fs::metadata(&path).map(|m| m.len() as i64).unwrap_or(0);
    let mut conn = open(false)?;
    let mut removed_rows = 0i64;
    {
        let tx = conn.transaction().map_err(|error| error.to_string())?;
        for (_, selector) in rules_for(rules, archived) {
            removed_rows += tx
                .execute(&format!("DELETE FROM event WHERE rowid IN ({selector})"), [])
                .map_err(|error| error.to_string())? as i64;
        }
        if rules.orphan_events {
            tx.execute(
                "DELETE FROM event_sequence WHERE aggregate_id NOT IN (SELECT id FROM session)",
                [],
            )
            .map_err(|error| error.to_string())?;
        }
        tx.commit().map_err(|error| error.to_string())?;
    }
    checkpoint(&conn);
    let page_size = scalar(&conn, "PRAGMA page_size")?;
    let free_bytes = scalar(&conn, "PRAGMA freelist_count")? * page_size;
    let after = std::fs::metadata(&path).map(|m| m.len() as i64).unwrap_or(0);
    Ok(PruneResult {
        removed_rows,
        freed_bytes: (before - after).max(0),
        free_bytes,
    })
}

/// Rewrites the database to release free pages back to the filesystem.
///
/// VACUUM needs exclusive access, so this fails while the engine holds the database open. The error
/// is surfaced verbatim so the UI can tell the user to close their sessions and retry.
pub fn compact() -> Result<PruneResult, String> {
    let path = database_path()?;
    let before = std::fs::metadata(&path).map(|m| m.len() as i64).unwrap_or(0);
    let conn = open(false)?;
    conn.execute_batch("VACUUM").map_err(|error| {
        format!("could not compact the database (it is in use): {error}")
    })?;
    checkpoint(&conn);
    let page_size = scalar(&conn, "PRAGMA page_size")?;
    let after = std::fs::metadata(&path).map(|m| m.len() as i64).unwrap_or(0);
    Ok(PruneResult {
        removed_rows: 0,
        freed_bytes: (before - after).max(0),
        free_bytes: scalar(&conn, "PRAGMA freelist_count")? * page_size,
    })
}

/// Session ids Drift has archived, used to widen the archive rules beyond the engine's own flag.
pub fn archived_ids(store: &Store) -> Vec<String> {
    store
        .archived()
        .map(|rows| rows.into_iter().map(|row| row.session_id).collect())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
