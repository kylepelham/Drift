//! Undo and redo hide history back to a prompt and restore files changed by the session and its subagents.
//! Files changed by another writer are kept and reported; unrelated workspace files are untouched.
//! Hidden history is deleted only when a new prompt commits the undo (see `Store::admit_prompt`).

use std::future::Future;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;

use super::snapshot::FileChange;
use super::types::{MessageWithParts, Part, Revert, Role, Session};
use crate::Engine;
use crate::event::Event;

/// How long an undo waits for the turn it stopped to finish its stop, a tool's cleanup included.
#[cfg(not(test))]
const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(15);
#[cfg(test)]
const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

#[derive(Debug)]
pub enum RevertError {
    NoSession,
    /// Only a prompt the user sent can be the point to go back to.
    NotAPrompt,
    /// A job that would not stop holds the session; its files and history are in motion.
    Busy,
    Stopped,
    Files(String),
    Store(rusqlite::Error),
}

impl From<rusqlite::Error> for RevertError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

#[derive(Debug, thiserror::Error)]
enum MarkError {
    #[error("{0}")]
    Store(#[from] rusqlite::Error),
    #[error("the session is gone")]
    Gone,
}

#[derive(Debug, thiserror::Error)]
enum ShiftError {
    #[error("{0}")]
    Current(#[from] super::snapshot::Error),
    #[error("{path}: {error}")]
    Put {
        path: String,
        error: super::snapshot::Error,
    },
    #[error("could not save the conversation's undo point ({0})")]
    Mark(MarkError),
}

/// The session after an undo or redo, and the files it left alone.
#[derive(Debug, Serialize, ToSchema)]
pub struct Undone {
    pub session: Session,
    /// Changed by someone else since the session last wrote them.
    pub kept: Vec<String>,
    /// Seen changing while a command ran, which does not show who changed them.
    pub unattributed: Vec<String>,
    /// Written by a call with no undo record (an imported conversation's older or unmatched edits); left as they are.
    pub unrecorded: Vec<String>,
}

/// A physical file's chronological change chain, with independent snapshot owners for its endpoints.
struct Net {
    file: Option<PathBuf>,
    key: Option<PathBuf>,
    path: String,
    before: Endpoint,
    after: Endpoint,
    observed: bool,
    broken: bool,
}

struct Endpoint {
    owner: String,
    blob: Option<String>,
}

/// A recorded call sorted by its completed writes, with its stable part id as the tie-breaker.
struct RecordedCall {
    stamp: String,
    part_id: String,
    owner: String,
    changes: Vec<FileChange>,
}

impl Net {
    fn new(owner: String, change: FileChange, file: Option<PathBuf>) -> Self {
        Self {
            key: file.as_deref().map(crate::tool::lock::path_key),
            file,
            path: change.path,
            before: Endpoint {
                owner: owner.clone(),
                blob: change.before,
            },
            after: Endpoint {
                owner,
                blob: change.after,
            },
            observed: change.observed,
            broken: false,
        }
    }

    fn names(&self, key: Option<&Path>, owner: &str, path: &str) -> bool {
        match key {
            Some(key) => self.key.as_deref() == Some(key),
            None => self.file.is_none() && self.before.owner == owner && self.path == path,
        }
    }

    fn extend(&mut self, owner: String, change: FileChange) {
        self.broken |= self.after.blob != change.before;

        self.after = Endpoint {
            owner,
            blob: change.after,
        };
        self.observed |= change.observed;
    }
}

#[derive(Default)]
struct Shifted {
    kept: Vec<String>,
    unattributed: Vec<String>,
    unrecorded: Vec<String>,
    /// What the shift changed, oldest first, kept until the marker is saved so a failed save can put it back.
    applied: Vec<Applied>,
    /// The turns of every file the shift may change, held until the marker is saved or the files put back.
    _turns: Option<crate::tool::lock::Held>,
}

/// A path a shift changed, and the content it had before, to restore if a later path fails.
struct Applied {
    workspace: PathBuf,
    path: String,
    previous: Option<String>,
}

enum Direction {
    /// Put each file back to how it was before the first change in the range.
    Back,
    /// Put each file forward to how the last change in the range left it.
    Forward,
}

impl Engine {
    /// Hides `message_id`, a prompt, with everything after it, and undoes what those turns changed.
    /// Called again while undone it moves the point either way, and the files follow.
    pub async fn revert(&self, session_id: &str, message_id: &str) -> Result<Undone, RevertError> {
        self.exclusively(session_id, self.revert_claimed(session_id, message_id, false))
            .await
    }

    /// As [`Self::revert`], but only the conversation moves: every file stays as it is now.
    pub async fn revert_keeping_files(&self, session_id: &str, message_id: &str) -> Result<Undone, RevertError> {
        self.exclusively(session_id, self.revert_claimed(session_id, message_id, true))
            .await
    }

    /// Brings back everything an undo hid, and redoes what those turns changed.
    pub async fn unrevert(&self, session_id: &str) -> Result<Undone, RevertError> {
        self.exclusively(session_id, self.unrevert_claimed(session_id)).await
    }

    /// Runs `work` holding the session, so no turn starts while its files and history change
    /// underneath. A running turn is stopped first: asking to undo is asking for it to end.
    async fn exclusively<T>(
        &self,
        session_id: &str,
        work: impl Future<Output = Result<T, RevertError>>,
    ) -> Result<T, RevertError> {
        let deadline = tokio::time::Instant::now() + STOP_WAIT;
        while !self.turns.claim(session_id, &CancellationToken::new()) {
            self.abort(session_id);
            let until = CancellationToken::new();
            if tokio::time::timeout_at(deadline, self.turns.wait_idle(session_id, &until))
                .await
                .is_err()
            {
                return Err(RevertError::Busy);
            }
        }

        let result = work.await;
        self.turns.release(session_id);
        result
    }

    async fn revert_claimed(
        &self,
        session_id: &str,
        message_id: &str,
        keep_files: bool,
    ) -> Result<Undone, RevertError> {
        let session = self.store.session(session_id)?.ok_or(RevertError::NoSession)?;

        if !self
            .store
            .transcript(session_id)?
            .iter()
            .any(|m| m.info.id == message_id && is_prompt(m))
        {
            return Err(RevertError::NotAPrompt);
        }
        // A previous keep-files undo can leave files at a different point from the conversation.
        let files = session.revert.as_ref().and_then(Revert::files_from);
        if keep_files {
            return self
                .mark_or_put_back(
                    session_id,
                    Some(&Revert::new(message_id, Vec::new(), files)),
                    Shifted::default(),
                )
                .await;
        }

        let shifted = match files {
            None => self.shift(&session, message_id, None, Direction::Back).await?,
            Some(current) if message_id < current => {
                self.shift(&session, message_id, Some(current), Direction::Back).await?
            }
            Some(current) if message_id > current => {
                self.shift(&session, current, Some(message_id), Direction::Forward)
                    .await?
            }
            Some(_) => Shifted::default(),
        };

        let revert = Revert::new(message_id, shifted.kept.clone(), Some(message_id));
        self.mark_or_put_back(session_id, Some(&revert), shifted).await
    }

    async fn unrevert_claimed(&self, session_id: &str) -> Result<Undone, RevertError> {
        let session = self.store.session(session_id)?.ok_or(RevertError::NoSession)?;
        let Some(revert) = &session.revert else {
            return Ok(Undone {
                session,
                kept: Vec::new(),
                unattributed: Vec::new(),
                unrecorded: Vec::new(),
            });
        };

        let shifted = match revert.files_from() {
            Some(from) => self.shift(&session, from, None, Direction::Forward).await?,
            None => Shifted::default(),
        };
        self.mark_or_put_back(session_id, None, shifted).await
    }

    /// Saves the undo marker after shifting files, rolling back the files if saving the marker fails.
    async fn mark_or_put_back(
        &self,
        session_id: &str,
        revert: Option<&Revert>,
        shifted: Shifted,
    ) -> Result<Undone, RevertError> {
        match self.mark(session_id, revert) {
            Ok(session) => Ok(Undone {
                session,
                kept: shifted.kept,
                unattributed: shifted.unattributed,
                unrecorded: shifted.unrecorded,
            }),
            Err(error) => Err(self.put_back(shifted.applied, ShiftError::Mark(error)).await),
        }
    }

    /// Applies recorded changes in `[from, to)`, leaving externally edited or merely observed files alone.
    /// Each endpoint's original snapshot owner supplies its bytes, even after workspace moves.
    /// File reservations are held until the marker is saved or rollback completes.
    async fn shift(
        &self,
        session: &Session,
        from: &str,
        to: Option<&str>,
        direction: Direction,
    ) -> Result<Shifted, RevertError> {
        let (nets, unrecorded) = self.net_changes(session, from, to)?;
        let abort = self.turns.cancellation(&session.id);
        let turns = tokio::select! {
            biased;
            () = abort.cancelled() => return Err(RevertError::Stopped),
            held = self.turns_for(&nets) => held,
        };
        if abort.is_cancelled() {
            return Err(RevertError::Stopped);
        }

        let mut shifted = Shifted {
            unrecorded,
            _turns: Some(turns),
            ..Shifted::default()
        };
        for net in nets {
            match self.shift_one(net, &direction, &mut shifted).await {
                Ok(Some(done)) => shifted.applied.push(done),
                Ok(None) => {}
                // On failure, files this shift already changed are restored, so a shift is all or nothing.
                Err(error) => return Err(self.put_back(shifted.applied, error).await),
            }
        }

        Ok(shifted)
    }

    /// Reserves the complete canonical path set once, including files named by multiple historical workspaces.
    async fn turns_for(&self, nets: &[Net]) -> crate::tool::lock::Held {
        let paths: Vec<PathBuf> = nets
            .iter()
            .filter(|net| !net.observed && !net.broken)
            .filter_map(|net| net.file.clone())
            .collect();

        crate::tool::lock::files(&paths).await
    }

    /// Applies one path's change, or records why it is left alone; `Some` names what to put back if a later path fails.
    async fn shift_one(
        &self,
        net: Net,
        direction: &Direction,
        shifted: &mut Shifted,
    ) -> Result<Option<Applied>, ShiftError> {
        if net.observed {
            shifted.unattributed.push(net.path);
            return Ok(None);
        }

        let Some(file) = net.file.as_ref().filter(|_| !net.broken) else {
            shifted.kept.push(net.path);
            return Ok(None);
        };

        let (expected, target) = match direction {
            Direction::Back => (&net.after, &net.before),
            Direction::Forward => (&net.before, &net.after),
        };
        let Some((previous_workspace, target_workspace)) =
            self.root_of(&expected.owner).zip(self.root_of(&target.owner))
        else {
            shifted.kept.push(net.path);
            return Ok(None);
        };

        let path = file.to_string_lossy().into_owned();
        let current = self.snapshots.current(&previous_workspace, &path).await?;
        if current != expected.blob {
            shifted.kept.push(net.path);
            return Ok(None);
        }

        self.snapshots
            .put(&self.store, &target_workspace, &path, target.blob.as_deref())
            .await
            .map_err(|error| ShiftError::Put {
                path: net.path.clone(),
                error,
            })?;

        Ok(Some(Applied {
            workspace: previous_workspace,
            path,
            previous: expected.blob.clone(),
        }))
    }

    /// Restores already shifted paths newest first, while the failed shift still holds their reservations.
    async fn put_back(&self, applied: Vec<Applied>, error: ShiftError) -> RevertError {
        let mut stuck = Vec::new();
        for Applied {
            workspace,
            path,
            previous,
        } in applied.into_iter().rev()
        {
            if let Err(failure) = self
                .snapshots
                .put(&self.store, &workspace, &path, previous.as_deref())
                .await
            {
                stuck.push(format!("{path} ({failure})"));
            }
        }

        let state = if stuck.is_empty() {
            "no file was changed".to_string()
        } else {
            format!("these could not be put back: {}", stuck.join("; "))
        };

        RevertError::Files(format!("{error}; {state}"))
    }

    /// Groups chronological changes by canonical physical path, retaining each endpoint's snapshot
    /// owner; also names the files finished writing calls changed without leaving a record.
    fn net_changes(
        &self,
        session: &Session,
        from: &str,
        to: Option<&str>,
    ) -> Result<(Vec<Net>, Vec<String>), RevertError> {
        let mut calls = Vec::new();
        let mut unrecorded: Vec<String> = Vec::new();

        for member in self.store.session_tree(&session.id)? {
            for message in
                self.store.transcript(&member)?.iter().filter(|message| {
                    message.info.id.as_str() >= from && to.is_none_or(|to| message.info.id.as_str() < to)
                })
            {
                for row in &message.parts {
                    if let Some(record) = recorded_changes(&row.part) {
                        // Legacy changes predate snapshot-owner fields and belong to the session's workspace.
                        let owner = record.owner.unwrap_or_else(|| session.workspace_id.clone());
                        let at = record.at.unwrap_or_else(|| message.info.id.clone());
                        calls.push(RecordedCall {
                            stamp: stamp(&at).to_string(),
                            part_id: row.id.clone(),
                            owner,
                            changes: record.changes,
                        });
                    } else {
                        unrecorded.extend(
                            written_without_record(&row.part)
                                .into_iter()
                                .filter(|path| !unrecorded.contains(path))
                                .collect::<Vec<_>>(),
                        );
                    }
                }
            }
        }

        calls.sort_by(|left, right| (&left.stamp, &left.part_id).cmp(&(&right.stamp, &right.part_id)));
        let net = self.fold_recorded_calls(calls);
        unrecorded.dedup();

        Ok((net, unrecorded))
    }

    fn fold_recorded_calls(&self, calls: Vec<RecordedCall>) -> Vec<Net> {
        let mut net: Vec<Net> = Vec::new();
        for (owner, change) in calls
            .into_iter()
            .flat_map(|call| call.changes.into_iter().map(move |change| (call.owner.clone(), change)))
        {
            let file = self
                .root_of(&owner)
                .map(|root| crate::tool::canonical(&root.join(&change.path)));
            let key = file.as_deref().map(crate::tool::lock::path_key);
            match net
                .iter_mut()
                .find(|existing| existing.names(key.as_deref(), &owner, &change.path))
            {
                Some(existing) => existing.extend(owner, change),
                None => net.push(Net::new(owner, change, file)),
            }
        }
        net.retain(|change| change.broken || change.before.blob != change.after.blob);

        net
    }

    fn mark(&self, session_id: &str, revert: Option<&Revert>) -> Result<Session, MarkError> {
        let session = self.store.set_revert(session_id, revert)?.ok_or(MarkError::Gone)?;
        // Undone messages may hold a check's full output, so the next report must not rely on it.
        self.turns.forget_checked(session_id);
        self.hub.publish(Event::SessionUpdated {
            session: session.clone(),
        });

        Ok(session)
    }

    /// Where a workspace is now, bound to its history; `None` once the workspace is gone.
    fn root_of(&self, owner: &str) -> Option<PathBuf> {
        let workspace = self.store.workspace(owner).ok()??;
        let root = crate::tool::canonical(Path::new(&workspace.path));
        self.snapshots.bind(owner, &root);

        Some(root)
    }
}

fn is_prompt(message: &MessageWithParts) -> bool {
    message.info.role == Role::User
        && !message
            .parts
            .iter()
            .any(|row| matches!(row.part, Part::Compaction { .. }))
}

/// A call's recorded changes, with their snapshot owner and write-completion stamp when available.
struct Record {
    owner: Option<String>,
    at: Option<String>,
    changes: Vec<FileChange>,
}

fn recorded_changes(part: &Part) -> Option<Record> {
    let Part::ToolCall {
        metadata: Some(metadata),
        ..
    } = part
    else {
        return None;
    };

    let changes = metadata
        .changes
        .as_ref()?
        .iter()
        .map(super::types::HistoryChange::snapshot)
        .collect();

    Some(Record {
        owner: metadata.owner.clone(),
        at: metadata.at.clone(),
        changes,
    })
}

/// The files a finished `edit`, `write` or `apply_patch` call wrote when it left no undo record: the
/// paths it was given and, for a patch, every file opencode listed it touching.
fn written_without_record(part: &Part) -> Vec<String> {
    let Part::ToolCall {
        name,
        input,
        status: super::types::ToolStatus::Done,
        metadata,
        ..
    } = part
    else {
        return Vec::new();
    };
    if !matches!(name.as_str(), "edit" | "write" | "apply_patch") {
        return Vec::new();
    }

    let mut paths: Vec<String> = ["filePath", "path"]
        .iter()
        .filter_map(|key| input[*key].as_str().map(str::to_string))
        .collect();
    for file in metadata
        .as_ref()
        .and_then(|metadata| metadata.files.as_ref())
        .into_iter()
        .flatten()
    {
        if let super::types::MetadataFile::Imported(file) = file {
            paths.extend(
                ["filePath", "movePath"]
                    .iter()
                    .filter_map(|key| file.get(*key)?.as_str().map(str::to_string)),
            );
        }
    }
    paths.dedup();

    paths
}

/// An id's time-ordered part without its prefix, so a change stamp and a message id compare by time.
fn stamp(id: &str) -> &str {
    id.split_once('_').map_or(id, |(_, rest)| rest)
}

#[cfg(test)]
mod tests;
