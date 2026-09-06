//! Content search across engine transcripts.
//!
//! Session titles are already in the frontend, so those are matched there. Message bodies are not:
//! they live in the engine database, and a workspace can hold far more of them than the transcript
//! cache ever loads. Searching them in the frontend would mean pulling every transcript first, so
//! the query runs here instead, against the same read-only connection the storage tab uses.
//!
//! The scan is bounded rather than exhaustive. `part` holds one row per streamed snapshot and grows
//! to millions of rows, so an unbounded `LIKE` would read the whole table for a query that matches
//! nothing. Restricting to the newest sessions in the workspace keeps the work proportional to what
//! a person plausibly wants to find again, and `part_session_idx` makes that restriction cheap.

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::time::Duration;

/// The engine may hold a write lock while streaming; wait rather than fail immediately.
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);
/// How many of the most recently updated sessions in a workspace are searched.
const MAX_SESSIONS_SCANNED: i64 = 500;
/// Distinct sessions returned. One row per session: the first match is enough to open it.
const MAX_RESULTS: usize = 40;
/// Rows examined before giving up, so a query matching nothing still returns promptly.
const MAX_ROWS_EXAMINED: i64 = 200_000;
/// Characters of surrounding context kept on each side of a match.
const EXCERPT_RADIUS: usize = 70;
const MIN_QUERY_CHARS: usize = 2;
const MAX_QUERY_CHARS: usize = 200;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMatch {
    pub session_id: String,
    pub message_id: String,
    pub title: String,
    pub directory: String,
    pub updated_at: i64,
    /// A single line of surrounding text, with the match left in place for the UI to highlight.
    pub excerpt: String,
}

fn database_path() -> Result<std::path::PathBuf, String> {
    crate::engine_db::database_path(true)
}

fn open() -> Result<Connection, String> {
    let conn = Connection::open_with_flags(database_path()?, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| error.to_string())?;
    conn.busy_timeout(BUSY_TIMEOUT)
        .map_err(|error| error.to_string())?;
    Ok(conn)
}

/// Escapes the wildcards SQLite's `LIKE` would otherwise interpret, so a query is matched literally.
pub(crate) fn escape_like(query: &str) -> String {
    let mut escaped = String::with_capacity(query.len());
    for character in query.chars() {
        if matches!(character, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// Slash direction and case both vary between what the engine stored and what Drift holds, so
/// directories are compared in one normalized form.
pub(crate) fn normalize_directory(directory: &str) -> String {
    directory.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// The readable text inside a part payload, or `None` for payloads that carry no prose.
pub(crate) fn part_text(data: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(data).ok()?;
    let object = value.as_object()?;
    match object.get("type")?.as_str()? {
        "text" | "reasoning" => Some(object.get("text")?.as_str()?.to_string()),
        _ => None,
    }
}

/// A single-line window around the first case-insensitive occurrence of `query`.
pub(crate) fn excerpt(text: &str, query: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let haystack = collapsed.to_lowercase();
    let needle = query.to_lowercase();
    let found = haystack.find(&needle);
    let Some(found) = found else {
        return collapsed.chars().take(EXCERPT_RADIUS * 2).collect();
    };
    // Lowercasing can change UTF-8 widths or expand a character. Map the match back before cropping.
    let mut lowercase_end = 0;
    let start_chars = collapsed.chars().position(|character| {
        lowercase_end += character.to_lowercase().map(char::len_utf8).sum::<usize>();
        lowercase_end > found
    }).unwrap_or(0);
    let begin = start_chars.saturating_sub(EXCERPT_RADIUS);
    let length = query.chars().count() + EXCERPT_RADIUS * 2;
    let mut window: String = collapsed.chars().skip(begin).take(length).collect();
    if begin > 0 {
        window.insert(0, '…');
    }
    if start_chars + query.chars().count() + EXCERPT_RADIUS < collapsed.chars().count() {
        window.push('…');
    }
    window
}

/// Sessions in `directory` whose transcript contains `query`, newest first.
///
/// An empty `directory` searches every workspace. Subagent sessions are excluded: their work is
/// reachable from the parent thread, and listing both would return the same conversation twice.
pub fn search(query: &str, directory: &str) -> Result<Vec<SessionMatch>, String> {
    search_in(&open()?, query, directory)
}

pub(crate) fn search_in(
    conn: &Connection,
    query: &str,
    directory: &str,
) -> Result<Vec<SessionMatch>, String> {
    let trimmed = query.trim();
    if trimmed.chars().count() < MIN_QUERY_CHARS {
        return Ok(Vec::new());
    }
    let needle = trimmed.chars().take(MAX_QUERY_CHARS).collect::<String>();
    let pattern = format!("%{}%", escape_like(&needle));
    let scope = normalize_directory(directory);

    let mut statement = conn
        .prepare(
            "WITH recent AS (
                SELECT id, title, directory, time_updated
                FROM session
                WHERE parent_id IS NULL
                  AND (?1 = '' OR REPLACE(LOWER(directory), '\\', '/') = ?1)
                ORDER BY time_updated DESC
                LIMIT ?2
            )
            SELECT recent.id, part.message_id, recent.title, recent.directory,
                   recent.time_updated, part.data
            FROM recent
            JOIN part ON part.session_id = recent.id
            WHERE part.data LIKE ?3 ESCAPE '\\'
            ORDER BY recent.time_updated DESC, part.time_created ASC
            LIMIT ?4",
        )
        .map_err(|error| error.to_string())?;

    let rows = statement
        .query_map(
            rusqlite::params![scope, MAX_SESSIONS_SCANNED, pattern, MAX_ROWS_EXAMINED],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    row.get::<_, Option<i64>>(4)?.unwrap_or_default(),
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .map_err(|error| error.to_string())?;

    let mut matches: Vec<SessionMatch> = Vec::new();
    for row in rows {
        let (session_id, message_id, title, directory, updated_at, data) =
            row.map_err(|error| error.to_string())?;
        if matches.iter().any(|found| found.session_id == session_id) {
            continue;
        }
        // The payload matched as raw JSON, which also hits tool arguments and encoded fields. Only
        // a match in the readable text is worth showing, so anything else is skipped here.
        let Some(text) = part_text(&data) else { continue };
        if !text.to_lowercase().contains(&needle.to_lowercase()) {
            continue;
        }
        matches.push(SessionMatch {
            session_id,
            message_id,
            title,
            directory,
            updated_at,
            excerpt: excerpt(&text, &needle),
        });
        if matches.len() >= MAX_RESULTS {
            break;
        }
    }
    Ok(matches)
}

#[cfg(test)]
#[path = "session_search_tests.rs"]
mod tests;
