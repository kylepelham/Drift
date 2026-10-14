//! Backups of chosen conversations as one portable file, and restoring them into a workspace. A
//! conversation keeps its ids, so the same file restored twice, or on two machines, is one conversation.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::tree::TreeError;
use super::types::{MessageStatus, MessageWithParts, Session, Todo, Visibility};
use crate::Engine;
use crate::event::Event;

/// What a backup file says it is, so another JSON file is refused rather than half read.
pub const BACKUP_FORMAT: &str = "drift-sessions";
pub const BACKUP_VERSION: u32 = 1;
/// Messages written per transaction while restoring, so a long conversation never holds the store for long.
const PAGE: usize = 200;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Backup {
    pub format: String,
    pub version: u32,
    pub exported_at: i64,
    /// Each conversation before its subagents.
    pub sessions: Vec<SessionBackup>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionBackup {
    pub session: Session,
    pub messages: Vec<MessageWithParts>,
    #[serde(default)]
    pub todos: Vec<Todo>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Restored {
    pub imported: Vec<String>,
    /// Already here, so left as they are.
    pub skipped: Vec<String>,
}

#[derive(Debug)]
pub enum BackupError {
    Tree(TreeError),
    /// Not a Drift backup, or one from a newer Drift.
    Unreadable(String),
}

impl From<TreeError> for BackupError {
    fn from(error: TreeError) -> Self {
        Self::Tree(error)
    }
}

impl From<rusqlite::Error> for BackupError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Tree(TreeError::Store(error))
    }
}

impl Engine {
    /// The conversations and their subagents as they stand, without anything an undo has hidden.
    pub fn export_sessions(&self, ids: &[String]) -> Result<Backup, BackupError> {
        let mut members = Vec::new();
        for id in ids {
            self.store.session(id)?.ok_or(TreeError::NoSession)?;
            for member in self.store.session_tree(id)? {
                if !members.contains(&member) {
                    members.push(member);
                }
            }
        }

        let sessions = members
            .iter()
            .filter_map(|id| self.session_backup(id).transpose())
            .collect::<Result<_, _>>()?;

        Ok(Backup {
            format: BACKUP_FORMAT.into(),
            version: BACKUP_VERSION,
            exported_at: crate::id::now_ms(),
            sessions,
        })
    }

    fn session_backup(&self, id: &str) -> rusqlite::Result<Option<SessionBackup>> {
        let Some(mut session) = self.store.session(id)? else {
            return Ok(None);
        };
        let hidden_from = session.revert.take().map(|revert| revert.message_id);
        let mut messages = self.store.transcript(id)?;
        if let Some(from) = hidden_from {
            messages.retain(|message| message.info.id < from);
        }
        // A reply still being written is saved as stopped, as the engine itself records it after a crash.
        for message in &mut messages {
            if message.info.status == MessageStatus::Streaming {
                message.info.status = MessageStatus::Aborted;
            }
        }
        session.running = false;

        Ok(Some(SessionBackup {
            session,
            messages,
            todos: self.store.todos(id)?,
        }))
    }

    /// Restores a backup into `workspace_id`, live and unarchived. A conversation already here is left alone.
    pub fn import_sessions(&self, backup: Backup, workspace_id: &str) -> Result<Restored, BackupError> {
        if backup.format != BACKUP_FORMAT || backup.version > BACKUP_VERSION {
            return Err(BackupError::Unreadable(format!(
                "not a Drift sessions backup this version can read ({} v{})",
                backup.format, backup.version
            )));
        }
        self.store.workspace(workspace_id)?.ok_or(TreeError::NoWorkspace)?;

        let ids: Vec<&str> = backup.sessions.iter().map(|entry| entry.session.id.as_str()).collect();
        let mut parents = Vec::new();
        for entry in &backup.sessions {
            let parent = entry.session.parent_id.as_deref();
            let linked = parent
                .is_some_and(|parent| ids.contains(&parent) || self.store.session(parent).ok().flatten().is_some());
            parents.push(linked);
        }

        let mut restored = Restored::default();
        for (entry, linked) in backup.sessions.into_iter().zip(parents) {
            let id = entry.session.id.clone();
            match self.restore_one(entry, workspace_id, linked)? {
                Some(session) => {
                    restored.imported.push(id);
                    self.hub.publish(Event::SessionCreated { session });
                }
                None => restored.skipped.push(id),
            }
        }

        Ok(restored)
    }

    fn restore_one(&self, entry: SessionBackup, workspace_id: &str, linked: bool) -> rusqlite::Result<Option<Session>> {
        let SessionBackup {
            mut session,
            messages,
            todos,
        } = entry;
        session.workspace_id = workspace_id.into();
        session.archived_at = None;
        session.running = false;
        // A thread spawned from a conversation not here stands on its own; a subagent keeps its parent.
        if !linked && session.visibility != Visibility::Hidden {
            session.parent_id = None;
        }

        self.store.allow_reimport(&session.id)?;
        if !self.store.begin_import(&session)? {
            return Ok(None);
        }
        let written = messages.chunks(PAGE).try_for_each(|page| self.store.import_page(page));
        if let Err(error) = written {
            let _ = self.store.discard_import(&session.id);
            return Err(error);
        }

        self.store.finish_import(&session.id, None, &todos)
    }
}

#[cfg(test)]
mod tests;
