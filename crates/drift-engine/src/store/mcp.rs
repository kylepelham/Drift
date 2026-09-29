use rusqlite::{params, OptionalExtension, Row};

use super::Store;
use crate::id;
use crate::mcp::{ServerConfig, ServerRow};

const COLUMNS: &str = "name, config_json, enabled, approved_hash, updated_at";

impl Store {
    pub fn mcp_servers(&self) -> rusqlite::Result<Vec<ServerRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare_cached(&format!("SELECT {COLUMNS} FROM mcp_config ORDER BY name"))?;
        let rows = stmt.query_map([], map_row)?;
        rows.collect()
    }

    pub fn mcp_server(&self, name: &str) -> rusqlite::Result<Option<ServerRow>> {
        self.lock()
            .prepare_cached(&format!("SELECT {COLUMNS} FROM mcp_config WHERE name = ?1"))?
            .query_row([name], map_row)
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
        let row = conn.prepare_cached(&format!("SELECT {COLUMNS} FROM mcp_config WHERE name = ?1"))?.query_row([name], map_row)?;
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
}

fn map_row(row: &Row) -> rusqlite::Result<ServerRow> {
    let json: String = row.get(1)?;
    let config = serde_json::from_str(&json).map_err(|e| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e)))?;
    Ok(ServerRow { name: row.get(0)?, config, enabled: row.get(2)?, approved_hash: row.get(3)?, updated_at: row.get(4)? })
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
        let hash = row.hash();
        assert!(store.approve_mcp_server("docs", &hash).unwrap());
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
}
