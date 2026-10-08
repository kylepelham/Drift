use drift_engine::session::types::{Ending, MessageStatus, Part, Role, ToolStatus, Visibility};
use drift_engine::store::Store;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::*;

mod conversations;
mod history;
mod lifecycle;
mod parts;

// Content hashes let assertions resolve the versions named by undo records.
#[derive(Default)]
struct Kept(HashMap<String, String>);

impl Blobs for Kept {
    fn store(&mut self, _owner: &str, _root: &Path, bytes: &[u8]) -> Option<String> {
        let id = format!("{:x}", Sha256::digest(bytes));
        self.0.insert(id.clone(), String::from_utf8(bytes.to_vec()).unwrap());

        Some(id)
    }
}

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Conversation<'a> {
    conn: &'a Connection,
    session: &'a str,
}

impl<'a> Conversation<'a> {
    fn new(conn: &'a Connection, session: &'a str) -> Self {
        Self { conn, session }
    }

    fn message(&self, id: &str, created: i64, data: Value, parts: &[Value]) {
        self.conn
            .execute(
                "INSERT INTO message VALUES(?1, ?2, ?3, ?3, ?4)",
                params![id, self.session, created, data.to_string()],
            )
            .unwrap();

        for (index, part) in parts.iter().enumerate() {
            self.conn
                .execute(
                    "INSERT INTO part VALUES(?1, ?2, ?3, ?4, ?4, ?5)",
                    params![
                        format!("prt_{id}_{index:02}"),
                        id,
                        self.session,
                        created,
                        part.to_string()
                    ],
                )
                .unwrap();
        }
    }
}

fn dir() -> Dir {
    let path = std::env::temp_dir().join(format!("drift-migrate-{}", drift_engine::id::new("t")));
    std::fs::create_dir_all(&path).unwrap();

    Dir(path)
}

fn opencode(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE project(id TEXT PRIMARY KEY, worktree TEXT NOT NULL);
         CREATE TABLE session(
             id TEXT PRIMARY KEY, project_id TEXT, parent_id TEXT, directory TEXT NOT NULL,
             title TEXT NOT NULL, agent TEXT, model TEXT, time_created INTEGER NOT NULL,
             time_updated INTEGER NOT NULL, time_archived INTEGER
         );
         CREATE TABLE message(
             id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL,
             time_updated INTEGER NOT NULL, data TEXT NOT NULL
         );
         CREATE TABLE part(
             id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL,
             time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL
         );
         CREATE TABLE todo(
             session_id TEXT NOT NULL, content TEXT NOT NULL, status TEXT NOT NULL, priority TEXT NOT NULL,
             position INTEGER NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL
         );",
    )
    .unwrap();

    conn
}

fn session(conn: &Connection, id: &str, parent: Option<&str>, directory: &str, archived: Option<i64>) {
    let model = json!({ "id": "claude-opus-5-5", "providerID": "anthropic", "variant": "high" }).to_string();
    conn.execute(
        "INSERT INTO session VALUES(?1, 'p', ?2, ?3, ?4, 'build', ?5, 1000, 9000, ?6)",
        params![id, parent, directory, format!("Title {id}"), model, archived],
    )
    .unwrap();
}

fn user(created: i64) -> Value {
    json!({
        "role": "user", "time": { "created": created }, "agent": "build",
        "model": { "providerID": "anthropic", "modelID": "claude-opus-5-5" },
    })
}

fn assistant(created: i64, extra: Value) -> Value {
    let mut data = json!({
        "role": "assistant", "agent": "build", "providerID": "anthropic", "modelID": "claude-opus-5-5",
        "cost": 0.5, "tokens": { "input": 10, "output": 20, "reasoning": 5, "cache": { "read": 100, "write": 7 } },
        "time": { "created": created, "completed": created + 5 },
    });
    data.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());

    data
}

fn store_with(dir: &Path, workspaces: &[&str]) -> Store {
    let store = drift_engine::store::open(&dir.join("drift")).unwrap();
    for path in workspaces {
        store.add_workspace(path, "w", "").unwrap();
    }

    store
}

fn run_import(
    store: &Store,
    source: &Path,
    archived: &HashSet<String>,
    blobs: &mut dyn Blobs,
    progress: &mut dyn FnMut(Progress),
) -> rusqlite::Result<Report> {
    import_sessions(SessionImport {
        store,
        source,
        archived,
        blobs,
        progress,
    })
}
