use rusqlite::{params, Connection, OptionalExtension, Row};

use super::Store;
use crate::id;
use crate::mcp::{ServerConfig, ServerRow};

const COLUMNS: &str = "name, config_json, enabled, approved_hash, updated_at";
const KEY_SETTING: &str = "mcpApprovalKey";

impl Store {
    pub fn mcp_servers(&self) -> rusqlite::Result<Vec<ServerRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare_cached(&format!("SELECT {COLUMNS} FROM mcp_config ORDER BY name"))?;
        let rows = stmt.query_map([], |row| map_row(row, &self.mcp_key))?;
        rows.collect()
    }

    pub fn mcp_server(&self, name: &str) -> rusqlite::Result<Option<ServerRow>> {
        self.lock()
            .prepare_cached(&format!("SELECT {COLUMNS} FROM mcp_config WHERE name = ?1"))?
            .query_row([name], |row| map_row(row, &self.mcp_key))
            .optional()
    }

    /// Saving a changed config withdraws approval; the user must look at what changed.
    pub fn save_mcp_server(&self, name: &str, config: &ServerConfig) -> rusqlite::Result<ServerRow> {
        let json = serde_json::to_string(config).unwrap();
        let conn = self.lock();
        conn.prepare_cached(
            "INSERT INTO mcp_config(name, config_json, enabled, approved_hash, updated_at) VALUES(?1, ?2, 1, NULL, ?3)
             ON CONFLICT(name) DO UPDATE SET config_json = ?2, approved_hash = CASE WHEN config_json = ?2 THEN approved_hash ELSE NULL END, updated_at = ?3",
        )?
        .execute(params![name, json, id::now_ms()])?;
        let row = conn.prepare_cached(&format!("SELECT {COLUMNS} FROM mcp_config WHERE name = ?1"))?.query_row([name], |row| map_row(row, &self.mcp_key))?;
        Ok(row)
    }

    pub fn approve_mcp_server(&self, name: &str, hash: &str) -> rusqlite::Result<bool> {
        let changed = self
            .lock()
            .prepare_cached("UPDATE mcp_config SET approved_hash = ?2, updated_at = ?3 WHERE name = ?1")?
            .execute(params![name, hash, id::now_ms()])?;
        Ok(changed > 0)
    }

    pub fn set_mcp_enabled(&self, name: &str, enabled: bool) -> rusqlite::Result<bool> {
        let changed = self
            .lock()
            .prepare_cached("UPDATE mcp_config SET enabled = ?2, updated_at = ?3 WHERE name = ?1")?
            .execute(params![name, enabled, id::now_ms()])?;
        Ok(changed > 0)
    }

    pub fn remove_mcp_server(&self, name: &str) -> rusqlite::Result<bool> {
        let changed = self.lock().prepare_cached("DELETE FROM mcp_config WHERE name = ?1")?.execute([name])?;
        Ok(changed > 0)
    }

    /// Renames `from` to `to`, approval and all, unless `to` is taken; `None` when `from` does not exist.
    pub fn rename_mcp_server(&self, from: &str, to: &str) -> rusqlite::Result<Option<Renamed>> {
        let conn = self.lock();
        if conn.prepare_cached("SELECT 1 FROM mcp_config WHERE name = ?1")?.exists([to])? {
            return Ok(Some(Renamed::Taken));
        }
        if conn.prepare_cached("UPDATE mcp_config SET name = ?2, updated_at = ?3 WHERE name = ?1")?.execute(params![from, to, id::now_ms()])? == 0 {
            return Ok(None);
        }
        let row = conn.prepare_cached(&format!("SELECT {COLUMNS} FROM mcp_config WHERE name = ?1"))?.query_row([to], |row| map_row(row, &self.mcp_key))?;
        Ok(Some(Renamed::To(row)))
    }
}

/// The engine's approval key, made on first use and kept in the database, never sent anywhere.
pub(super) fn approval_key(conn: &Connection) -> rusqlite::Result<String> {
    let saved: Option<String> = conn.prepare_cached("SELECT value_json FROM setting WHERE key = ?1")?.query_row([KEY_SETTING], |row| row.get(0)).optional()?;
    if let Some(key) = saved.and_then(|json| serde_json::from_str::<String>(&json).ok()) {
        return Ok(key);
    }
    let key = crate::random_hex(32);
    conn.prepare_cached("INSERT INTO setting(key, value_json) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value_json = ?2")?
        .execute(params![KEY_SETTING, serde_json::to_string(&key).unwrap()])?;
    Ok(key)
}

/// The config's identity as approved: a different command, URL or secret is a different thing to approve.
pub(crate) fn approval_hash(key: &str, config: &ServerConfig) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key.as_bytes());
    let tag = ring::hmac::sign(&key, serde_json::to_string(config).unwrap().as_bytes());
    tag.as_ref().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Approvals stored under the unkeyed hash earlier builds used carry over, re-keyed, while their config is unchanged.
pub(super) fn rekey_approvals(conn: &Connection, key: &str) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare_cached(&format!("SELECT {COLUMNS} FROM mcp_config WHERE approved_hash IS NOT NULL"))?;
    let rows = stmt.query_map([], |row| map_row(row, key))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for row in rows.iter().filter(|row| row.approved_hash.as_deref() == Some(unkeyed_hash(&row.config).as_str())) {
        conn.prepare_cached("UPDATE mcp_config SET approved_hash = ?2 WHERE name = ?1")?.execute(params![row.name, row.hash])?;
    }
    Ok(())
}

fn unkeyed_hash(config: &ServerConfig) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(serde_json::to_string(config).unwrap().as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

pub enum Renamed {
    To(ServerRow),
    /// A server already has the new name; nothing changed.
    Taken,
}

fn map_row(row: &Row, key: &str) -> rusqlite::Result<ServerRow> {
    let json: String = row.get(1)?;
    let config = serde_json::from_str(&json).map_err(|e| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e)))?;
    let hash = approval_hash(key, &config);
    Ok(ServerRow { name: row.get(0)?, config, enabled: row.get(2)?, approved_hash: row.get(3)?, hash, updated_at: row.get(4)? })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::store;

    #[test]
    fn a_changed_config_needs_approving_again() {
        let store = store();
        let config = ServerConfig::Stdio { command: "npx".into(), args: vec!["server".into()], env: Default::default() };
        let row = store.save_mcp_server("docs", &config).unwrap();
        assert!(row.approved_hash.is_none());
        assert!(store.approve_mcp_server("docs", &row.hash).unwrap());
        assert!(store.mcp_server("docs").unwrap().unwrap().is_approved());
        store.save_mcp_server("docs", &config).unwrap();
        assert!(store.mcp_server("docs").unwrap().unwrap().is_approved(), "an unchanged save keeps approval");
        let changed = ServerConfig::Stdio { command: "npx".into(), args: vec!["evil".into()], env: Default::default() };
        store.save_mcp_server("docs", &changed).unwrap();
        assert!(!store.mcp_server("docs").unwrap().unwrap().is_approved(), "a changed config withdraws approval");
        assert!(store.set_mcp_enabled("docs", false).unwrap());
        assert!(!store.mcp_servers().unwrap()[0].enabled);
        assert!(store.remove_mcp_server("docs").unwrap());
        assert!(store.mcp_servers().unwrap().is_empty());
    }

    #[test]
    fn the_approval_hash_is_keyed_per_engine_and_old_approvals_carry_over() {
        let dir = std::env::temp_dir().join(format!("drift-mcp-key-{}", crate::random_hex(4)));
        let config = ServerConfig::Http { url: "https://example.com/mcp".into(), headers: [("Authorization".to_string(), "Bearer 1234".to_string())].into() };
        let first = crate::store::open(&dir).unwrap().save_mcp_server("docs", &config).unwrap();
        assert_ne!(first.hash, unkeyed_hash(&config), "a client cannot test guesses at the secret against it");
        assert_ne!(first.hash, store().save_mcp_server("docs", &config).unwrap().hash, "another engine, another key");
        crate::store::open(&dir).unwrap().lock().execute("UPDATE mcp_config SET approved_hash = ?1", [unkeyed_hash(&config)]).unwrap();
        let reopened = crate::store::open(&dir).unwrap().mcp_server("docs").unwrap().unwrap();
        assert_eq!(reopened.hash, first.hash, "the key is kept");
        assert!(reopened.is_approved(), "approved before the hash was keyed, still approved");
        std::fs::remove_dir_all(dir).ok();
    }
}
