use rusqlite::{Connection, OptionalExtension, Row, params};

use super::Store;
use super::sessions::transaction;
use crate::id;
use crate::mcp::{Era, Given, ServerConfig, ServerRow, WorkspaceChoice, wire_names};

const COLUMNS: &str = "name, config_json, enabled, updated_at, era, read_only_trusted";

impl Store {
    /// The names the model calls these `(server, tool)` pairs by, keeping every name given before
    /// and recording the new ones, under one lock so two turns never hand one name out twice.
    pub fn name_mcp_tools(&self, tools: &[(&str, &str)]) -> rusqlite::Result<Vec<String>> {
        let conn = self.lock();
        let given: Given = conn
            .prepare_cached("SELECT server, tool, name FROM mcp_tool_name")?
            .query_map([], |row| Ok(((row.get(0)?, row.get(1)?), row.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let names = wire_names(&given, tools);
        let mut keep =
            conn.prepare_cached("INSERT OR IGNORE INTO mcp_tool_name(server, tool, name) VALUES(?1, ?2, ?3)")?;
        for ((server, tool), name) in tools.iter().zip(&names) {
            keep.execute(params![server, tool, name])?;
        }
        Ok(names)
    }

    /// The servers this build can read; one it cannot ([`Self::unreadable_mcp_servers`]) never takes the others down.
    pub fn mcp_servers(&self) -> rusqlite::Result<Vec<ServerRow>> {
        Ok(self
            .stored_servers()?
            .into_iter()
            .filter_map(Stored::readable)
            .collect())
    }

    /// Servers whose saved definition this build cannot read, most likely written by a newer Drift:
    /// listed so the user can save them again or remove them.
    pub fn unreadable_mcp_servers(&self) -> rusqlite::Result<Vec<String>> {
        Ok(self
            .stored_servers()?
            .into_iter()
            .filter_map(|stored| match stored {
                Stored::Unreadable(name) => Some(name),
                Stored::Row(_) => None,
            })
            .collect())
    }

    fn stored_servers(&self) -> rusqlite::Result<Vec<Stored>> {
        let conn = self.lock();
        let mut stmt = conn.prepare_cached(&format!("SELECT {COLUMNS} FROM mcp_config ORDER BY name"))?;
        let rows = stmt.query_map([], stored)?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(|stored| with_choices(&conn, stored)).collect()
    }

    /// The server, or `None` when there is none or its definition cannot be read.
    pub fn mcp_server(&self, name: &str) -> rusqlite::Result<Option<ServerRow>> {
        server_in(&self.lock(), name)
    }

    /// A new server starts trusted by read-only agents; saving over one keeps the trust it has.
    pub fn save_mcp_server(&self, name: &str, config: &ServerConfig) -> rusqlite::Result<ServerRow> {
        let json = serde_json::to_string(config).unwrap();
        let conn = self.lock();
        conn.prepare_cached(
            "INSERT INTO mcp_config(name, config_json, enabled, updated_at, read_only_trusted) VALUES(?1, ?2, 1, ?3, 1)
             ON CONFLICT(name) DO UPDATE SET config_json = ?2, updated_at = ?3, era = NULL",
        )?
        .execute(params![name, json, id::now_ms()])?;
        let row = conn
            .prepare_cached(&format!("SELECT {COLUMNS} FROM mcp_config WHERE name = ?1"))?
            .query_row([name], map_row)?;
        choices_of(&conn, row)
    }

    /// Remembers the era a server answered in, only while it still has the config it answered under.
    pub fn set_mcp_era(&self, name: &str, config: &ServerConfig, era: Option<Era>) -> rusqlite::Result<bool> {
        let json = serde_json::to_string(config).unwrap();
        let changed = self
            .lock()
            .prepare_cached("UPDATE mcp_config SET era = ?3 WHERE name = ?1 AND config_json = ?2")?
            .execute(params![name, json, era.map(Era::as_str)])?;
        Ok(changed > 0)
    }

    /// Lets read-only agents use the server's read-only tools, or stops them.
    pub fn set_mcp_read_only_trusted(&self, name: &str, trusted: bool) -> rusqlite::Result<bool> {
        let changed = self
            .lock()
            .prepare_cached("UPDATE mcp_config SET read_only_trusted = ?2 WHERE name = ?1")?
            .execute(params![name, trusted])?;
        Ok(changed > 0)
    }

    /// The server's switch: on or off in every workspace, so the workspaces' own choices go.
    pub fn set_mcp_enabled(&self, name: &str, enabled: bool) -> rusqlite::Result<bool> {
        transaction(&self.lock(), |conn| {
            conn.prepare_cached("DELETE FROM mcp_workspace WHERE server = ?1")?
                .execute([name])?;
            Ok(conn
                .prepare_cached("UPDATE mcp_config SET enabled = ?2, updated_at = ?3 WHERE name = ?1")?
                .execute(params![name, enabled, id::now_ms()])?
                > 0)
        })
    }

    /// The server on or off in one workspace; kept only where it differs from the switch. False when there is no such server.
    pub fn set_mcp_choice(&self, name: &str, workspace_id: &str, enabled: bool) -> rusqlite::Result<bool> {
        transaction(&self.lock(), |conn| {
            conn.prepare_cached("DELETE FROM mcp_workspace WHERE server = ?1 AND workspace_id = ?2")?
                .execute([name, workspace_id])?;
            conn.prepare_cached("INSERT INTO mcp_workspace(server, workspace_id, enabled) SELECT name, ?2, ?3 FROM mcp_config WHERE name = ?1 AND enabled != ?3")?
                .execute(params![name, workspace_id, enabled])?;
            conn.prepare_cached("SELECT 1 FROM mcp_config WHERE name = ?1")?
                .exists([name])
        })
    }

    /// Removes the server and the names its tools were given, which a later server may then use.
    pub fn remove_mcp_server(&self, name: &str) -> rusqlite::Result<bool> {
        transaction(&self.lock(), |conn| {
            conn.prepare_cached("DELETE FROM mcp_tool_name WHERE server = ?1")?
                .execute([name])?;
            Ok(conn
                .prepare_cached("DELETE FROM mcp_config WHERE name = ?1")?
                .execute([name])?
                > 0)
        })
    }

    /// Renames `from` to `to`, saved secrets and all, unless `to` is taken; `None` when `from` does not exist.
    pub fn rename_mcp_server(&self, from: &str, to: &str) -> rusqlite::Result<Option<Renamed>> {
        let conn = self.lock();
        if conn
            .prepare_cached("SELECT 1 FROM mcp_config WHERE name = ?1")?
            .exists([to])?
        {
            return Ok(Some(Renamed::Taken));
        }
        if server_in(&conn, from)?.is_none() {
            return Ok(None);
        }
        if conn
            .prepare_cached("UPDATE mcp_config SET name = ?2, updated_at = ?3 WHERE name = ?1")?
            .execute(params![from, to, id::now_ms()])?
            == 0
        {
            return Ok(None);
        }
        // Its tools are named after it, so they take new names; the old ones are free again.
        conn.prepare_cached("DELETE FROM mcp_tool_name WHERE server = ?1")?
            .execute([from])?;
        let row = conn
            .prepare_cached(&format!("SELECT {COLUMNS} FROM mcp_config WHERE name = ?1"))?
            .query_row([to], map_row)?;
        Ok(Some(Renamed::To(Box::new(choices_of(&conn, row)?))))
    }
}

pub enum Renamed {
    To(Box<ServerRow>),
    /// A server already has the new name; nothing changed.
    Taken,
}

/// The config's identity, so a connection can tell whether the server it serves still has the definition it opened with.
fn config_hash(config: &ServerConfig) -> String {
    use sha2::Digest;
    let json = serde_json::to_string(config).unwrap();
    let digest = sha2::Sha256::digest(json.as_bytes());

    crate::hex_bytes(&digest[..8])
}

/// A saved server, or the name of one whose definition does not parse.
enum Stored {
    Row(Box<ServerRow>),
    Unreadable(String),
}

impl Stored {
    fn readable(self) -> Option<ServerRow> {
        match self {
            Self::Row(row) => Some(*row),
            Self::Unreadable(_) => None,
        }
    }
}

fn stored(row: &Row) -> rusqlite::Result<Stored> {
    match map_row(row) {
        Ok(server) => Ok(Stored::Row(Box::new(server))),
        Err(rusqlite::Error::FromSqlConversionFailure(1, ..)) => Ok(Stored::Unreadable(row.get(0)?)),
        Err(error) => Err(error),
    }
}

fn server_in(conn: &Connection, name: &str) -> rusqlite::Result<Option<ServerRow>> {
    let found = conn
        .prepare_cached(&format!("SELECT {COLUMNS} FROM mcp_config WHERE name = ?1"))?
        .query_row([name], stored)
        .optional()?;
    found
        .and_then(Stored::readable)
        .map(|row| choices_of(conn, row))
        .transpose()
}

fn with_choices(conn: &Connection, stored: Stored) -> rusqlite::Result<Stored> {
    match stored {
        Stored::Row(row) => Ok(Stored::Row(Box::new(choices_of(conn, *row)?))),
        unreadable => Ok(unreadable),
    }
}

/// The row with the workspaces that chose otherwise than its switch, and their folders.
fn choices_of(conn: &Connection, mut row: ServerRow) -> rusqlite::Result<ServerRow> {
    row.workspaces = conn
        .prepare_cached("SELECT c.workspace_id, w.path, c.enabled FROM mcp_workspace c JOIN workspace w ON w.id = c.workspace_id WHERE c.server = ?1 ORDER BY c.workspace_id")?
        .query_map([&row.name], |r| Ok(WorkspaceChoice { workspace_id: r.get(0)?, path: r.get(1)?, enabled: r.get(2)? }))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(row)
}

fn map_row(row: &Row) -> rusqlite::Result<ServerRow> {
    let json: String = row.get(1)?;
    let config = serde_json::from_str(&json)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e)))?;
    let hash = config_hash(&config);
    let era = row.get::<_, Option<String>>(4)?.as_deref().and_then(Era::parse);
    Ok(ServerRow {
        name: row.get(0)?,
        config,
        enabled: row.get(2)?,
        hash,
        updated_at: row.get(3)?,
        era,
        read_only_trusted: row.get(5)?,
        workspaces: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::store;

    #[test]
    fn a_workspace_keeps_only_a_choice_that_differs_from_the_switch_and_the_switch_clears_them() {
        let store = store();
        let config = ServerConfig::Stdio {
            command: "node".into(),
            args: vec![],
            env: Default::default(),
            cwd: None,
            timeout_seconds: None,
        };
        store.save_mcp_server("ida", &config).unwrap();
        let re = store.add_workspace("C:/re", "re", "").unwrap();
        let web = store.add_workspace("C:/web", "web", "").unwrap();
        store.set_mcp_enabled("ida", false).unwrap();
        assert!(store.set_mcp_choice("ida", &re.id, true).unwrap());
        assert!(
            store.set_mcp_choice("ida", &web.id, false).unwrap(),
            "the same as the switch"
        );
        let row = store.mcp_server("ida").unwrap().unwrap();
        assert_eq!(
            row.workspaces,
            [WorkspaceChoice {
                workspace_id: re.id.clone(),
                path: "C:/re".into(),
                enabled: true
            }],
            "only the choice that differs is kept"
        );
        assert!(row.on_anywhere() && !row.enabled);
        assert!(!store.set_mcp_choice("missing", &re.id, true).unwrap());

        let Some(Renamed::To(renamed)) = store.rename_mcp_server("ida", "ida-pro").unwrap() else {
            panic!()
        };
        assert_eq!(renamed.workspaces.len(), 1, "a rename keeps the workspaces' choices");
        store.set_mcp_enabled("ida-pro", true).unwrap();
        assert!(
            store.mcp_server("ida-pro").unwrap().unwrap().workspaces.is_empty(),
            "the switch is on everywhere, so the choices go"
        );
        store.set_mcp_choice("ida-pro", &web.id, false).unwrap();
        store.remove_mcp_server("ida-pro").unwrap();
        assert_eq!(
            store
                .lock()
                .query_row("SELECT count(*) FROM mcp_workspace", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn tool_names_are_kept_across_calls() {
        let store = store();
        assert_eq!(store.name_mcp_tools(&[("a", "b_c")]).unwrap(), ["a_b_c"]);
        let both = store.name_mcp_tools(&[("a_b", "c"), ("a", "b_c")]).unwrap();
        assert_eq!(both[1], "a_b_c", "the first keeps its name");
        assert!(both[0].starts_with("a_b_c_"), "{both:?}");
        assert_eq!(
            store.name_mcp_tools(&[("a_b", "c")]).unwrap(),
            [both[0].clone()],
            "and so does the second, once given"
        );
        let config = ServerConfig::Stdio {
            command: "npx".into(),
            args: vec![],
            env: Default::default(),
            cwd: None,
            timeout_seconds: None,
        };
        store.save_mcp_server("a", &config).unwrap();
        store.remove_mcp_server("a").unwrap();
        assert_eq!(
            store.name_mcp_tools(&[("a_b", "c")]).unwrap(),
            [both[0].clone()],
            "a name stays with its tool"
        );
        store.save_mcp_server("a_b", &config).unwrap();
        assert!(matches!(
            store.rename_mcp_server("a_b", "x").unwrap(),
            Some(Renamed::To(_))
        ));
        assert_eq!(
            store.name_mcp_tools(&[("a", "b_c")]).unwrap(),
            ["a_b_c"],
            "removed and renamed servers free their names"
        );
    }

    #[test]
    fn servers_save_rename_disable_and_remove() {
        let store = store();
        let config = ServerConfig::Stdio {
            command: "npx".into(),
            args: vec!["server".into()],
            env: Default::default(),
            cwd: None,
            timeout_seconds: None,
        };
        let row = store.save_mcp_server("docs", &config).unwrap();
        assert!(row.enabled, "a saved server is on and needs nothing more to run");
        assert!(
            row.read_only_trusted,
            "and read-only agents may use its read-only tools"
        );
        let changed = ServerConfig::Stdio {
            command: "npx".into(),
            args: vec!["other".into()],
            env: Default::default(),
            cwd: None,
            timeout_seconds: None,
        };
        assert_ne!(
            store.save_mcp_server("docs", &changed).unwrap().hash,
            row.hash,
            "a changed definition is a different connection"
        );
        assert!(
            matches!(store.rename_mcp_server("docs", "notes").unwrap(), Some(Renamed::To(renamed)) if renamed.config == changed)
        );
        assert!(store.set_mcp_era("notes", &changed, Some(Era::Stateless)).unwrap());
        assert!(
            !store.set_mcp_era("notes", &config, Some(Era::Legacy)).unwrap(),
            "an era found under an older config is not kept"
        );
        assert_eq!(store.mcp_server("notes").unwrap().unwrap().era, Some(Era::Stateless));
        assert_eq!(
            store.save_mcp_server("notes", &changed).unwrap().era,
            None,
            "a save forgets it"
        );
        assert!(store.set_mcp_read_only_trusted("notes", false).unwrap());
        assert!(
            !store.save_mcp_server("notes", &config).unwrap().read_only_trusted,
            "a save keeps the trust the user set"
        );
        store
            .lock()
            .execute(
                "UPDATE mcp_config SET config_json = '{\"type\":\"future\"}' WHERE name = 'notes'",
                [],
            )
            .unwrap();
        assert!(
            store.save_mcp_server("notes", &changed).unwrap().config == changed,
            "a config that no longer parses is overwritten"
        );
        assert_unreadable_server_isolation(&store, &config, &changed);
    }

    fn assert_unreadable_server_isolation(store: &Store, config: &ServerConfig, changed: &ServerConfig) {
        store.save_mcp_server("good", config).unwrap();
        store
            .lock()
            .execute(
                "UPDATE mcp_config SET config_json = '{\"type\":\"future\"}' WHERE name = 'notes'",
                [],
            )
            .unwrap();
        assert_eq!(
            store
                .mcp_servers()
                .unwrap()
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            ["good"],
            "one unreadable server does not hide the rest"
        );
        assert_eq!(store.unreadable_mcp_servers().unwrap(), ["notes"]);
        assert!(store.mcp_server("notes").unwrap().is_none());
        assert!(
            store.rename_mcp_server("notes", "other").unwrap().is_none(),
            "only a readable server is renamed"
        );
        store.save_mcp_server("notes", changed).unwrap();
        store.remove_mcp_server("good").unwrap();
        assert!(store.set_mcp_enabled("notes", false).unwrap());
        assert!(!store.mcp_servers().unwrap()[0].enabled);
        assert!(store.remove_mcp_server("notes").unwrap());
        assert!(store.mcp_servers().unwrap().is_empty());
    }
}
