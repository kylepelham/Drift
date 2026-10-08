use rusqlite::{OptionalExtension, params, types::Type};
use serde_json::Value;

use super::{PromptOverride, RemoteDevice, Store, now};

impl Store {
    pub(crate) fn app_setting(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.0
            .lock()
            .query_row("SELECT value FROM app_setting WHERE key = ?1", [key], |row| row.get(0))
            .optional()
    }

    pub(crate) fn initialize_app_setting(&self, key: &str, value: &str) -> rusqlite::Result<String> {
        let mut connection = self.0.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT OR IGNORE INTO app_setting(key, value) VALUES(?1, ?2)",
            params![key, value],
        )?;
        let stored = transaction.query_row("SELECT value FROM app_setting WHERE key = ?1", [key], |row| row.get(0))?;
        transaction.commit()?;

        Ok(stored)
    }

    pub(crate) fn save_app_setting(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.0.lock().execute(
            "INSERT INTO app_setting(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;

        Ok(())
    }

    pub(crate) fn delete_app_setting(&self, key: &str) -> rusqlite::Result<()> {
        self.0.lock().execute("DELETE FROM app_setting WHERE key = ?1", [key])?;

        Ok(())
    }

    pub(crate) fn dictation_enabled(&self) -> rusqlite::Result<bool> {
        let value = self.app_setting("dictation_enabled")?;
        Ok(value.as_deref() == Some("true"))
    }

    pub(crate) fn save_dictation_enabled(&self, enabled: bool) -> rusqlite::Result<()> {
        let value = if enabled { "true" } else { "false" };
        self.save_app_setting("dictation_enabled", value)
    }

    pub(crate) fn remote_access_enabled(&self) -> rusqlite::Result<bool> {
        let enabled = self
            .0
            .lock()
            .query_row("SELECT enabled FROM remote_access WHERE id = 1", [], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?;

        Ok(enabled == Some(1))
    }

    /// Saves access status without changing the retired shared token so older builds can still open the database.
    pub(crate) fn save_remote_access(&self, enabled: bool) -> rusqlite::Result<()> {
        self.0.lock().execute(
            "INSERT INTO remote_access(id, enabled, token) VALUES(1, ?1, '')
                 ON CONFLICT(id) DO UPDATE SET enabled = ?1",
            [enabled as i64],
        )?;

        Ok(())
    }

    pub(crate) fn remote_devices(&self) -> rusqlite::Result<Vec<RemoteDevice>> {
        let connection = self.0.lock();
        let mut statement = connection.prepare_cached(
            "SELECT id, name, token_hash, method, created_at, last_seen_at
             FROM remote_device ORDER BY created_at",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(RemoteDevice {
                id: row.get(0)?,
                name: row.get(1)?,
                token_hash: row.get(2)?,
                method: row.get(3)?,
                created_at: row.get(4)?,
                last_seen_at: row.get(5)?,
            })
        })?;

        rows.collect()
    }

    pub(crate) fn insert_remote_device(&self, device: &RemoteDevice) -> rusqlite::Result<()> {
        self.0.lock().execute(
            "INSERT INTO remote_device(id, name, token_hash, method, created_at, last_seen_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                device.id,
                device.name,
                device.token_hash,
                device.method,
                device.created_at,
                device.last_seen_at
            ],
        )?;

        Ok(())
    }

    pub(crate) fn touch_remote_device(&self, id: &str, at: i64) -> rusqlite::Result<()> {
        self.0.lock().execute(
            "UPDATE remote_device SET last_seen_at = ?2 WHERE id = ?1",
            params![id, at],
        )?;

        Ok(())
    }

    /// Deletes one device, every device when id is None, or every device signed in with method.
    pub(crate) fn delete_remote_devices(&self, id: Option<&str>, method: Option<&str>) -> rusqlite::Result<()> {
        self.0.lock().execute(
            "DELETE FROM remote_device WHERE (?1 IS NULL OR id = ?1) AND (?2 IS NULL OR method = ?2)",
            params![id, method],
        )?;

        Ok(())
    }

    pub(crate) fn prompt_overrides(&self) -> rusqlite::Result<Vec<PromptOverride>> {
        let connection = self.0.lock();
        let mut statement = connection
            .prepare_cached("SELECT key, value_json, original_json, updated_at FROM prompt_override ORDER BY key")?;
        let rows = statement.query_map([], |row| {
            let value: String = row.get(1)?;
            let original: Option<String> = row.get(2)?;
            let key = row.get(0)?;
            let value = serde_json::from_str(&value)
                .map_err(|error| rusqlite::Error::FromSqlConversionFailure(1, Type::Text, Box::new(error)))?;
            let original = original
                .map(|item| {
                    serde_json::from_str(&item)
                        .map_err(|error| rusqlite::Error::FromSqlConversionFailure(2, Type::Text, Box::new(error)))
                })
                .transpose()?;

            Ok(PromptOverride {
                key,
                value,
                original,
                updated_at: row.get(3)?,
            })
        })?;

        rows.collect()
    }

    pub(crate) fn save_prompt_override(
        &self,
        key: &str,
        value: &Value,
        original: Option<&Value>,
    ) -> rusqlite::Result<()> {
        let connection = self.0.lock();
        let value = serde_json::to_string(value).unwrap_or_else(|_| "null".into());
        let original = original.map(|item| serde_json::to_string(item).unwrap_or_else(|_| "null".into()));

        connection
            .prepare_cached(
                "INSERT INTO prompt_override(key, value_json, original_json, updated_at) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(key) DO UPDATE SET value_json = ?2,
               original_json = COALESCE(prompt_override.original_json, ?3), updated_at = ?4",
            )?
            .execute(params![key, value, original, now()])?;

        Ok(())
    }

    pub(crate) fn reset_prompt_override(&self, key: &str) -> rusqlite::Result<()> {
        self.0
            .lock()
            .prepare_cached("DELETE FROM prompt_override WHERE key = ?1")?
            .execute([key])?;

        Ok(())
    }
}
