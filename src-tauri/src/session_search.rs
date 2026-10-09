//! Searches transcript bodies in drift.db on a separate read-only connection; titles are matched in the frontend.
//! Bounds scans to the newest conversations and message/part indexes so a query with no match returns promptly.
//! The scan never holds the engine's writer or relies on the frontend's limited transcript cache.

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::path::Path;
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
pub(crate) struct SessionMatch {
    pub session_id: String,
    pub message_id: String,
    pub title: String,
    pub directory: String,
    pub updated_at: i64,
    /// A single line of surrounding text, with the match left in place for the UI to highlight.
    pub excerpt: String,
}

fn open(database: &Path) -> rusqlite::Result<Connection> {
    let connection = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(BUSY_TIMEOUT)?;

    Ok(connection)
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

/// Normalizes directory casing and slash direction for comparisons between stored and user-selected paths.
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
    let start_chars = collapsed
        .chars()
        .position(|character| {
            lowercase_end += character.to_lowercase().map(char::len_utf8).sum::<usize>();
            lowercase_end > found
        })
        .unwrap_or(0);

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

/// Conversations in directory whose transcript contains query, newest first.
/// An empty directory searches every workspace on the sidebar; subagent sessions are excluded.
/// Subagent work remains reachable through its parent thread rather than appearing twice in search results.
pub(crate) fn search(database: &Path, query: &str, directory: &str) -> rusqlite::Result<Vec<SessionMatch>> {
    search_in(&open(database)?, query, directory)
}

pub(crate) fn search_in(connection: &Connection, query: &str, directory: &str) -> rusqlite::Result<Vec<SessionMatch>> {
    let trimmed = query.trim();
    if trimmed.chars().count() < MIN_QUERY_CHARS {
        return Ok(Vec::new());
    }

    let needle = trimmed.chars().take(MAX_QUERY_CHARS).collect::<String>();
    let pattern = format!("%{}%", escape_like(&needle));
    let scope = normalize_directory(directory);

    let mut statement = connection.prepare(
        "WITH recent AS (
                SELECT session.id, session.title, workspace.path AS directory, session.updated_at
                FROM session JOIN workspace ON workspace.id = session.workspace_id
                WHERE session.visibility = 'sibling' AND workspace.removed_at IS NULL
                  AND (?1 = '' OR RTRIM(REPLACE(LOWER(workspace.path), '\\', '/'), '/') = ?1)
                ORDER BY session.updated_at DESC
                LIMIT ?2
            )
            SELECT recent.id, part.message_id, recent.title, recent.directory,
                   recent.updated_at, part.json
            FROM recent
            JOIN message ON message.session_id = recent.id
            JOIN part ON part.message_id = message.id
            WHERE part.json LIKE ?3 ESCAPE '\\'
            ORDER BY recent.updated_at DESC, part.id ASC
            LIMIT ?4",
    )?;

    let rows = statement.query_map(
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
    )?;

    let mut matches: Vec<SessionMatch> = Vec::new();
    for row in rows {
        let (session_id, message_id, title, directory, updated_at, data) = row?;
        if matches.iter().any(|found| found.session_id == session_id) {
            continue;
        }
        // Raw JSON matches can hit tool arguments; only readable conversation text belongs in results.
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
