//! opencode's sign-ins: `auth.json`, and opencode v2's `credential` table in `opencode.db`.
//! v2 renews its sign-ins in the table, so for a provider in both the table's is the newer.

use rusqlite::{Connection, OpenFlags};
use serde_json::{Map, Value, json};
use std::path::Path;

/// Every sign-in in opencode's data directory in `auth.json`'s shape, or None when there are none.
pub fn sign_ins(data_dir: &Path) -> Option<Value> {
    let file = std::fs::read_to_string(data_dir.join("auth.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&drift_engine::config::jsonc::strip(&text)).ok());
    let stored = stored(&data_dir.join("opencode.db")).unwrap_or_default();
    if file.is_none() && stored.is_empty() {
        return None;
    }

    let mut merged = match file {
        Some(Value::Object(entries)) => entries,
        _ => Map::new(),
    };
    merged.extend(stored);

    Some(Value::Object(merged))
}

/// Each provider's selected credential, as v2 picks it: the active one, else the newest.
/// A database before v2 has no such table, which is no sign-ins rather than an error.
fn stored(path: &Path) -> rusqlite::Result<Map<String, Value>> {
    let mut found = Map::new();
    if !path.is_file() {
        return Ok(found);
    }

    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    let mut statement = conn.prepare(
        "SELECT integration_id, value FROM credential WHERE integration_id IS NOT NULL
         ORDER BY integration_id, active DESC, time_created DESC, id DESC",
    )?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;

    for row in rows {
        let (provider, value) = row?;
        found.entry(provider).or_insert_with(|| entry(&value));
    }

    Ok(found)
}

/// A v2 credential in `auth.json`'s shape; an `external` one (a CLI's own sign-in) keeps only its type, to be reported.
fn entry(text: &str) -> Value {
    let value: Value = serde_json::from_str(text).unwrap_or(Value::Null);

    match value["type"].as_str() {
        Some("key") => json!({ "type": "api", "key": value["key"] }),
        Some("oauth") => json!({
            "type": "oauth", "access": value["access"], "refresh": value["refresh"],
            "expires": value["expires"], "accountId": value["metadata"]["accountID"],
        }),
        _ => json!({ "type": value["type"] }),
    }
}
