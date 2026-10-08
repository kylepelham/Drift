use rusqlite::{OptionalExtension, params};

use super::Store;
use crate::id;
use crate::session::types::Todo;

impl Store {
    pub fn todos(&self, session_id: &str) -> rusqlite::Result<Vec<Todo>> {
        let json: Option<String> = self
            .lock()
            .prepare_cached("SELECT json FROM todo WHERE session_id = ?1")?
            .query_row([session_id], |row| row.get(0))
            .optional()?;

        Ok(json
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default())
    }

    pub fn set_todos(&self, session_id: &str, todos: &[Todo]) -> rusqlite::Result<()> {
        self.lock()
            .prepare_cached(
                "INSERT INTO todo(session_id, json, updated_at) VALUES(?1, ?2, ?3)
                 ON CONFLICT(session_id) DO UPDATE SET json = ?2, updated_at = ?3",
            )?
            .execute(params![session_id, serde_json::to_string(todos).unwrap(), id::now_ms()])?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::types::{TodoStatus, Visibility};
    use crate::store::{NewSession, tests::store};

    #[test]
    fn todos_replace_wholesale() {
        let store = store();
        let session = store
            .create_session(NewSession {
                workspace_id: "w",
                parent_id: None,
                visibility: Visibility::Sibling,
                title: "",
                agent: "build",
                model: None,
            })
            .unwrap();
        assert!(store.todos(&session.id).unwrap().is_empty());
        let first = vec![Todo {
            content: "a".into(),
            status: TodoStatus::Pending,
            priority: "high".into(),
        }];
        store.set_todos(&session.id, &first).unwrap();
        assert_eq!(store.todos(&session.id).unwrap(), first);
        let second = vec![Todo {
            content: "b".into(),
            status: TodoStatus::Completed,
            priority: "low".into(),
        }];
        store.set_todos(&session.id, &second).unwrap();
        assert_eq!(store.todos(&session.id).unwrap(), second);
    }
}
