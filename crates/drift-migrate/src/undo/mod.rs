//! Rebuilds recent file versions by reversing opencode diffs against today's files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use drift_engine::session::snapshot::FileChange;
use serde_json::{Value, json};

use crate::map::Records;
use crate::source::{OcSession, Source};

mod diff;

use diff::reverse;

// Older edits rarely still match the files on disk.
const RECENT_MESSAGES: usize = 30;
const RECENT_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const WRITERS: [&str; 3] = ["edit", "write", "apply_patch"];

pub trait Blobs {
    /// Returns the stored blob ID, or None if the workspace history could not store the version.
    fn store(&mut self, owner: &str, root: &Path, bytes: &[u8]) -> Option<String>;
}

pub(crate) struct Planned<'a> {
    pub session: &'a OcSession,
    pub owner: String,
    pub root: PathBuf,
}

struct Write<'a> {
    created: i64,
    part_id: String,
    data: Value,
    plan: &'a Planned<'a>,
}

pub(crate) fn records(
    source: &Source,
    planned: &[Planned],
    now: i64,
    blobs: &mut dyn Blobs,
) -> rusqlite::Result<Records> {
    let mut writes = recent_writes(source, planned, now)?;
    writes.sort_by(|left, right| (right.created, &right.part_id).cmp(&(left.created, &left.part_id)));
    let mut files = Files::default();
    let mut records = Records::new();

    for write in &writes {
        if let Some(record) = record(write, &mut files, blobs) {
            records.insert(write.part_id.clone(), record);
        }
    }

    Ok(records)
}

fn recent_writes<'a>(source: &Source, planned: &'a [Planned<'a>], now: i64) -> rusqlite::Result<Vec<Write<'a>>> {
    let mut writes = Vec::new();

    for plan in planned {
        let messages = source.newest_messages(&plan.session.id, RECENT_MESSAGES)?;
        for message in messages
            .into_iter()
            .filter(|message| message.created >= now - RECENT_MS)
        {
            for part_id in source.part_ids(&message.id)? {
                let Some(data) = source
                    .small_part(&part_id)?
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                else {
                    continue;
                };

                if data["type"] == "tool"
                    && data["state"]["status"] == "completed"
                    && WRITERS.contains(&data["tool"].as_str().unwrap_or_default())
                {
                    writes.push(Write {
                        created: message.created,
                        part_id,
                        data,
                        plan,
                    });
                }
            }
        }
    }

    Ok(writes)
}

// Each file advances backwards through its versions as calls are rebuilt newest first.
#[derive(Default)]
struct Files {
    versions: HashMap<String, Option<Option<String>>>,
}

impl Files {
    // Outer None means lost history; inner None means a known absent file.
    fn current(&mut self, path: &Path) -> Option<Option<String>> {
        self.versions
            .entry(path_key(path))
            .or_insert_with(|| read(path))
            .clone()
    }

    fn set(&mut self, path: &Path, content: Option<Option<String>>) {
        self.versions.insert(path_key(path), content);
    }
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn read(path: &Path) -> Option<Option<String>> {
    match std::fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes).ok().map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(None),
        Err(_) => None,
    }
}

struct Change {
    path: PathBuf,
    before: Option<String>,
    after: Option<String>,
}

// A failed call ends history for all its files so older calls cannot invent an intermediate version.
fn record(write: &Write, files: &mut Files, blobs: &mut dyn Blobs) -> Option<Value> {
    let changes = changes(write, files);
    let stored = changes.as_ref().and_then(|changes| {
        changes
            .iter()
            .map(|change| stored(change, write.plan, blobs))
            .collect::<Option<Vec<_>>>()
    });
    let Some(stored) = stored else {
        for path in touched(write) {
            files.set(&path, None);
        }
        return None;
    };

    for change in changes.unwrap_or_default() {
        files.set(&change.path, Some(change.before));
    }

    Some(json!({ "changes": stored, "owner": write.plan.owner }))
}

fn stored(change: &Change, plan: &Planned, blobs: &mut dyn Blobs) -> Option<FileChange> {
    let mut blob = |content: &Option<String>| match content {
        Some(text) => blobs.store(&plan.owner, &plan.root, text.as_bytes()).map(Some),
        None => Some(None),
    };

    Some(FileChange {
        path: relative(&change.path, &plan.root),
        before: blob(&change.before)?,
        after: blob(&change.after)?,
        observed: false,
    })
}

// Native undo records use relative paths inside the workspace and absolute paths outside it.
fn relative(path: &Path, root: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    let root = root.to_string_lossy().replace('\\', "/");
    let root = root.trim_end_matches('/');

    match path.get(..root.len()) {
        Some(prefix) if prefix.eq_ignore_ascii_case(root) && path[root.len()..].starts_with('/') => {
            path[root.len() + 1..].to_string()
        }
        _ => path,
    }
}

fn resolve(path: &str, write: &Write) -> PathBuf {
    let path = Path::new(path);
    let windows_absolute = path
        .to_string_lossy()
        .get(1..3)
        .is_some_and(|drive| matches!(drive, ":\\" | ":/"));
    if path.is_absolute() || windows_absolute {
        return path.to_path_buf();
    }

    Path::new(&write.plan.session.directory).join(path)
}

fn touched(write: &Write) -> Vec<PathBuf> {
    let state = &write.data["state"];
    let mut names: Vec<&str> = state["input"]["filePath"].as_str().into_iter().collect();

    for file in state["metadata"]["files"].as_array().into_iter().flatten() {
        names.extend(["filePath", "movePath"].iter().filter_map(|key| file[*key].as_str()));
    }

    names.into_iter().map(|name| resolve(name, write)).collect()
}

fn changes(write: &Write, files: &mut Files) -> Option<Vec<Change>> {
    let state = &write.data["state"];

    match write.data["tool"].as_str()? {
        "edit" => edit(write, state, files).map(|change| vec![change]),
        "write" if state["metadata"]["exists"] == false => {
            let path = resolve(state["input"]["filePath"].as_str()?, write);
            let content = state["input"]["content"].as_str()?;

            (files.current(&path)?.as_deref() == Some(content)).then(|| {
                vec![Change {
                    path,
                    before: None,
                    after: Some(content.into()),
                }]
            })
        }
        "apply_patch" => state["metadata"]["files"]
            .as_array()?
            .iter()
            .map(|file| patched(write, file, files))
            .collect::<Option<Vec<_>>>()
            .map(|all| all.into_iter().flatten().collect()),
        _ => None,
    }
}

fn edit(write: &Write, state: &Value, files: &mut Files) -> Option<Change> {
    let path = resolve(state["input"]["filePath"].as_str()?, write);
    let diff = &state["metadata"]["filediff"];
    let after = files.current(&path)??;

    if let (Some(before), Some(recorded)) = (diff["before"].as_str(), diff["after"].as_str()) {
        return (recorded == after).then(|| Change {
            path,
            before: Some(before.into()),
            after: Some(after),
        });
    }

    let patch = diff["patch"].as_str().or(state["metadata"]["diff"].as_str())?;
    let before = reverse(patch, &after)?;
    // An empty oldString with no prior content means the edit created the file.
    let created = state["input"]["oldString"] == "" && before.is_empty();

    Some(Change {
        path,
        before: (!created).then_some(before),
        after: Some(after),
    })
}

// Moves need two records: creation at the destination and removal at the original path.
fn patched(write: &Write, file: &Value, files: &mut Files) -> Option<Vec<Change>> {
    let path = resolve(file["filePath"].as_str()?, write);
    let kind = file["type"].as_str()?;
    let destination = file["movePath"]
        .as_str()
        .filter(|_| kind == "move")
        .map(|destination| resolve(destination, write));
    let current = files.current(destination.as_ref().unwrap_or(&path))?;

    let before = match (file["before"].as_str(), file["after"].as_str()) {
        (Some(before), Some(recorded))
            if (kind == "delete" && current.is_none()) || current.as_deref() == Some(recorded) =>
        {
            (kind != "add").then(|| before.to_string())
        }
        (Some(_), Some(_)) => return None,
        _ => {
            let patch = file["patch"].as_str().or(file["diff"].as_str())?;
            original(patch, kind, current.as_deref())?
        }
    };

    match destination {
        Some(destination) => files.current(&path)?.is_none().then(|| {
            vec![
                Change {
                    path: destination,
                    before: None,
                    after: current,
                },
                Change {
                    path,
                    before,
                    after: None,
                },
            ]
        }),
        None => Some(vec![Change {
            path,
            before,
            after: current,
        }]),
    }
}

fn original(diff: &str, kind: &str, current: Option<&str>) -> Option<Option<String>> {
    match kind {
        "add" => reverse(diff, current?)?.is_empty().then_some(None),
        "delete" if current.is_none() => reverse(diff, "").map(Some),
        "update" | "move" => reverse(diff, current?).map(Some),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_inside_the_workspace_become_relative_ignoring_case_and_slashes() {
        assert_eq!(
            relative(Path::new("C:\\Repo\\src\\a.rs"), Path::new("c:/repo")),
            "src/a.rs"
        );
        assert_eq!(
            relative(Path::new("C:/Repo2/a.rs"), Path::new("C:/Repo")),
            "C:/Repo2/a.rs"
        );
    }
}
