use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const SHARED_DATABASE_ENV: &str = "OPENCODE_DISABLE_CHANNEL_DB";
const LEGACY_DATABASE: &str = "opencode-master.db";
/// Independent legacy child rows only merge into a session the target actually keeps, so
/// children of a session deleted in the target are dropped instead of breaking its
/// foreign keys.
const SESSION_PARENT_EXISTS: &str = "EXISTS (
    SELECT 1 FROM main.session parent WHERE parent.id = source.session_id
)";

pub fn prepare_shared() -> Result<usize, String> {
    let source = database_path(false)?;
    let target = database_path(true)?;
    if source == target {
        return Ok(0);
    }
    if !target.is_file() {
        return Err(format!(
            "shared OpenCode database does not exist: {}",
            target.display()
        ));
    }
    if !source.is_file() {
        return Ok(0);
    }
    merge_sessions(&source, &target).map_err(|error| error.to_string())
}

pub fn database_path(shared: bool) -> Result<PathBuf, String> {
    Ok(opencode_data_dir()?.join(if shared {
        "opencode.db"
    } else {
        LEGACY_DATABASE
    }))
}

fn opencode_data_dir() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(path).join("opencode"));
    }
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .map(|home| home.join(".local").join("share").join("opencode"))
        .ok_or_else(|| "cannot resolve the OpenCode data directory".into())
}

fn merge_sessions(source: &Path, target: &Path) -> rusqlite::Result<usize> {
    let mut conn = Connection::open_with_flags(target, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    conn.busy_timeout(Duration::from_secs(10))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute(
        "ATTACH DATABASE ?1 AS drift_channel",
        [source.to_string_lossy().as_ref()],
    )?;

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current_events =
        table_schemas_match(&tx, "event_sequence")? && table_schemas_match(&tx, "event")?;
    tx.execute_batch(
        "CREATE TEMP TABLE target_owned_session(
            session_id TEXT PRIMARY KEY
         ) WITHOUT ROWID;
         CREATE TEMP TABLE source_session_aggregate(
            session_id TEXT PRIMARY KEY
         ) WITHOUT ROWID;",
    )?;
    // The target owns a session id when it still holds either the session row or the
    // durable event aggregate. A session deleted in the target keeps its aggregate, so
    // the missing session row alone never makes the source copy importable.
    tx.execute_batch(
        "INSERT INTO temp.target_owned_session(session_id)
         SELECT source.id
         FROM drift_channel.session source
         WHERE EXISTS (
             SELECT 1 FROM main.session target WHERE target.id = source.id
         );",
    )?;
    if current_events {
        tx.execute_batch(
            "INSERT OR IGNORE INTO temp.target_owned_session(session_id)
             SELECT source.id
             FROM drift_channel.session source
             WHERE EXISTS (
                 SELECT 1 FROM main.event_sequence sequence
                 WHERE sequence.aggregate_id = source.id
             )
             OR EXISTS (
                 SELECT 1 FROM main.event event WHERE event.aggregate_id = source.id
             );",
        )?;
        tx.execute_batch(
            "INSERT INTO temp.source_session_aggregate(session_id)
             SELECT source.id
             FROM drift_channel.session source
             JOIN drift_channel.event_sequence sequence ON sequence.aggregate_id = source.id
             WHERE source.id NOT IN (SELECT session_id FROM temp.target_owned_session)
             AND sequence.seq >= 0
             AND (
                 SELECT COUNT(*) FROM drift_channel.event event
                 WHERE event.aggregate_id = source.id
             ) = sequence.seq + 1
             AND (
                 SELECT COUNT(DISTINCT event.seq) FROM drift_channel.event event
                 WHERE event.aggregate_id = source.id
                   AND event.seq BETWEEN 0 AND sequence.seq
             ) = sequence.seq + 1
             AND NOT EXISTS (
                 SELECT 1
                 FROM drift_channel.event event
                 JOIN main.event target ON target.id = event.id
                 WHERE event.aggregate_id = source.id
             );",
        )?;
    }

    let mut imported = 0;
    imported += copy_table(
        &tx,
        "project",
        true,
        Some("source.id IN (SELECT project_id FROM drift_channel.session)"),
    )?;
    imported += copy_table(
        &tx,
        "session",
        true,
        Some("source.id NOT IN (SELECT session_id FROM temp.target_owned_session)"),
    )?;
    if current_events {
        imported += copy_table(
            &tx,
            "event_sequence",
            false,
            Some(
                "source.aggregate_id IN (
                    SELECT session_id FROM temp.source_session_aggregate
                )",
            ),
        )?;
        imported += copy_table(
            &tx,
            "event",
            false,
            Some(
                "source.aggregate_id IN (
                    SELECT session_id FROM temp.source_session_aggregate
                )",
            ),
        )?;
    }
    imported += copy_table(&tx, "message", false, Some(SESSION_PARENT_EXISTS))?;
    imported += copy_table(
        &tx,
        "part",
        false,
        Some(
            "EXISTS (
                SELECT 1 FROM main.message parent
                WHERE parent.id = source.message_id AND parent.session_id = source.session_id
            )",
        ),
    )?;
    if current_events && table_schemas_match(&tx, "session_message")? {
        imported += copy_table(
            &tx,
            "session_message",
            false,
            Some(
                "source.session_id IN (
                    SELECT session_id FROM temp.source_session_aggregate
                 ) AND EXISTS (
                    SELECT 1 FROM drift_channel.event event
                    WHERE event.aggregate_id = source.session_id AND event.seq = source.seq
                 )",
            ),
        )?;
    }
    if current_events && table_schemas_match(&tx, "session_input")? {
        imported += copy_table(
            &tx,
            "session_input",
            false,
            Some(
                "source.session_id IN (
                    SELECT session_id FROM temp.source_session_aggregate
                 ) AND EXISTS (
                    SELECT 1 FROM drift_channel.event event
                    WHERE event.aggregate_id = source.session_id
                      AND event.seq = source.admitted_seq
                 ) AND (
                    source.promoted_seq IS NULL OR EXISTS (
                        SELECT 1 FROM drift_channel.event event
                        WHERE event.aggregate_id = source.session_id
                          AND event.seq = source.promoted_seq
                    )
                 )",
            ),
        )?;
    }
    if current_events && table_schemas_match(&tx, "session_context_epoch")? {
        imported += copy_table(
            &tx,
            "session_context_epoch",
            false,
            Some(
                "source.session_id IN (
                    SELECT session_id FROM temp.source_session_aggregate
                )",
            ),
        )?;
    }
    imported += copy_table(&tx, "todo", false, Some(SESSION_PARENT_EXISTS))?;
    imported += copy_table(&tx, "session_share", false, Some(SESSION_PARENT_EXISTS))?;
    verify_foreign_keys(&tx)?;
    tx.commit()?;
    Ok(imported)
}

struct Column {
    name: String,
    required: bool,
}

fn table_columns(tx: &Transaction<'_>, schema: &str, table: &str) -> rusqlite::Result<Vec<Column>> {
    let mut statement = tx.prepare(&format!(
        "PRAGMA {schema}.table_info({})",
        quote_identifier(table)
    ))?;
    let columns = statement
        .query_map([], |row| {
            let not_null = row.get::<_, bool>(3)?;
            let default = row.get::<_, Option<String>>(4)?;
            let primary_key = row.get::<_, i64>(5)? != 0;
            Ok(Column {
                name: row.get(1)?,
                required: primary_key || (not_null && default.is_none()),
            })
        })?
        .collect();
    columns
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn table_schemas_match(tx: &Transaction<'_>, table: &str) -> rusqlite::Result<bool> {
    let target = table_columns(tx, "main", table)?;
    let source = table_columns(tx, "drift_channel", table)?;
    Ok(!target.is_empty()
        && target.len() == source.len()
        && target
            .iter()
            .zip(source)
            .all(|(target, source)| target.name == source.name))
}

fn copy_table(
    tx: &Transaction<'_>,
    table: &str,
    required: bool,
    filter: Option<&str>,
) -> rusqlite::Result<usize> {
    let target_columns = table_columns(tx, "main", table)?;
    let source_columns = table_columns(tx, "drift_channel", table)?;
    if target_columns.is_empty() || source_columns.is_empty() {
        return if required {
            Err(rusqlite::Error::InvalidParameterName(format!(
                "required migration table is missing: {table}"
            )))
        } else {
            Ok(0)
        };
    }

    let source_names = source_columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<HashSet<_>>();
    let mut insert_columns = Vec::new();
    let mut select_columns = Vec::new();
    for column in &target_columns {
        let source_name = source_names
            .contains(column.name.as_str())
            .then_some(column.name.as_str());
        if let Some(source_name) = source_name {
            insert_columns.push(quote_identifier(&column.name));
            select_columns.push(format!("source.{}", quote_identifier(source_name)));
        } else if column.required {
            return if required {
                Err(rusqlite::Error::InvalidParameterName(format!(
                    "source table {table} is missing required column {}",
                    column.name
                )))
            } else {
                Ok(0)
            };
        }
    }

    let table = quote_identifier(table);
    let filter = filter
        .map(|value| format!(" WHERE {value}"))
        .unwrap_or_default();
    tx.execute(
        &format!(
            "INSERT OR IGNORE INTO main.{table} ({}) SELECT {} FROM drift_channel.{table} source{filter}",
            insert_columns.join(", "),
            select_columns.join(", ")
        ),
        [],
    )
}

fn verify_foreign_keys(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    let violation = {
        let mut statement = tx.prepare("PRAGMA foreign_key_check")?;
        let mut rows = statement.query([])?;
        if let Some(row) = rows.next()? {
            Some((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
            ))
        } else {
            None
        }
    };
    if let Some((table, rowid, parent)) = violation {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY),
            Some(format!(
                "foreign key violation after legacy migration: {table} row {rowid:?} references {parent}"
            )),
        ));
    }
    Ok(())
}

pub fn configure_shared(command: &mut Command) {
    command.env(SHARED_DATABASE_ENV, "1");
}

#[cfg(test)]
#[path = "engine_db_tests.rs"]
mod tests;
