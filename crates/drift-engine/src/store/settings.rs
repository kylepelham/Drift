//! Engine-wide preferences, one JSON value per key.

use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::Store;

impl Store {
    /// `None` when the key was never set or no longer parses as `T`.
    pub fn setting<T: DeserializeOwned>(&self, key: &str) -> rusqlite::Result<Option<T>> {
        let json: Option<String> = self
            .lock()
            .prepare_cached("SELECT value_json FROM setting WHERE key = ?1")?
            .query_row([key], |row| row.get(0))
            .optional()?;
        Ok(json.and_then(|json| serde_json::from_str(&json).ok()))
    }

    pub fn set_setting<T: Serialize>(&self, key: &str, value: &T) -> rusqlite::Result<()> {
        self.lock()
            .prepare_cached(
                "INSERT INTO setting(key, value_json) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value_json = ?2",
            )?
            .execute(params![key, serde_json::to_string(value).unwrap()])?;
        Ok(())
    }

    pub fn remove_setting(&self, key: &str) -> rusqlite::Result<()> {
        self.lock()
            .prepare_cached("DELETE FROM setting WHERE key = ?1")?
            .execute([key])?;
        Ok(())
    }
}
