use rusqlite::{params, OptionalExtension, Row};

use super::Store;
use crate::id;
use crate::mcp::{ServerConfig, ServerRow};

const COLUMNS: &str = "name, config_json, enabled, updated_at";

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

    pub fn save_mcp_server(&self, name: &str, config: &ServerConfig) -> rusqlite::Result<ServerRow> {
        let json = serde_json::to_string(config).unwrap();
        let conn = self.lock();
        conn.prepare_cached(
            "INSERT INTO mcp_config(name, config_json, enabled, updated_at) VALUES(?1, ?2, 1, ?3)
             ON CONFLICT(name) DO UPDATE SET config_json = ?2, updated_at = ?3",
        )?
        .execute(params![name, json, id::now_ms()])?;
        let row = conn.prepare_cached(&format!("SELECT {COLUMNS} FROM mcp_config WHERE name = ?1"))?.query_row([name], map_row)?;
        Ok(row)
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

    /// Renames `from` to `to`, saved secrets and all, unless `to` is taken; `None` when `from` does not exist.
    pub fn rename_mcp_server(&self, from: &str, to: &str) -> rusqlite::Result<Option<Renamed>> {
        let conn = self.lock();
        if conn.prepare_cached("SELECT 1 FROM mcp_config WHERE name = ?1")?.exists([to])? {
            return Ok(Some(Renamed::Taken));
        }
        if conn.prepare_cached("UPDATE mcp_config SET name = ?2, updated_at = ?3 WHERE name = ?1")?.execute(params![from, to, id::now_ms()])? == 0 {
            return Ok(None);
        }
        let row = conn.prepare_cached(&format!("SELECT {COLUMNS} FROM mcp_config WHERE name = ?1"))?.query_row([to], map_row)?;
        Ok(Some(Renamed::To(row)))
    }
}

pub enum Renamed {
    To(ServerRow),
    /// A server already has the new name; nothing changed.
    Taken,
}

/// The config's identity, so a connection can tell whether the server it serves still has the definition it opened with.
fn config_hash(config: &ServerConfig) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(serde_json::to_string(config).unwrap().as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn map_row(row: &Row) -> rusqlite::Result<ServerRow> {
    let json: String = row.get(1)?;
    let config = serde_json::from_str(&json).map_err(|e| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e)))?;
    let hash = config_hash(&config);
    Ok(ServerRow { name: row.get(0)?, config, enabled: row.get(2)?, hash, updated_at: row.get(3)? })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::store;

    #[test]
    fn servers_save_rename_disable_and_remove() {
        let store = store();
        let config = ServerConfig::Stdio { command: "npx".into(), args: vec!["server".into()], env: Default::default() };
        let row = store.save_mcp_server("docs", &config).unwrap();
        assert!(row.enabled, "a saved server is on and needs nothing more to run");
        let changed = ServerConfig::Stdio { command: "npx".into(), args: vec!["other".into()], env: Default::default() };
        assert_ne!(store.save_mcp_server("docs", &changed).unwrap().hash, row.hash, "a changed definition is a different connection");
        assert!(matches!(store.rename_mcp_server("docs", "notes").unwrap(), Some(Renamed::To(renamed)) if renamed.config == changed));
        assert!(store.set_mcp_enabled("notes", false).unwrap());
        assert!(!store.mcp_servers().unwrap()[0].enabled);
        assert!(store.remove_mcp_server("notes").unwrap());
        assert!(store.mcp_servers().unwrap().is_empty());
    }
}
