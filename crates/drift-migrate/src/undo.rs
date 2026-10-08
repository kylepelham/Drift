//! Undo records for an import's recent edits. opencode kept a diff per edited file, not the versions
//! undo puts back, so each version is rebuilt from today's file by applying the diffs backwards,
//! newest edit first across every conversation imported in the run. A diff that no longer matches
//! (the file changed since) ends the rebuild for its files: that call and older ones on them get no
//! record, and undo names them instead of guessing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use drift_engine::session::snapshot::FileChange;
use serde_json::{Value, json};

use crate::map::Records;
use crate::source::{OcSession, Source};

/// Only edits in a conversation's newest messages, and only from the past week, get a record: older
/// ones almost never still match the files.
const RECENT_MESSAGES: usize = 30;
const RECENT_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const WRITERS: [&str; 3] = ["edit", "write", "apply_patch"];

/// Where rebuilt versions are kept: the workspace's undo history.
pub trait Blobs {
    /// Stores `bytes` in the history of workspace `owner` at `root`; its blob id, `None` when it cannot.
    fn store(&mut self, owner: &str, root: &Path, bytes: &[u8]) -> Option<String>;
}

/// A conversation this run imports, with where it lands.
pub(crate) struct Planned<'a> {
    pub session: &'a OcSession,
    pub owner: String,
    pub root: PathBuf,
}

struct Write<'a> {
    created: i64,
    part: String,
    data: Value,
    plan: &'a Planned<'a>,
}

/// Records by opencode part id, for the recent writing calls whose versions could be rebuilt.
pub(crate) fn records(
    source: &Source,
    planned: &[Planned],
    now: i64,
    blobs: &mut dyn Blobs,
) -> rusqlite::Result<Records> {
    let mut writes = Vec::new();
    for plan in planned {
        for message in source
            .newest_messages(&plan.session.id, RECENT_MESSAGES)?
            .into_iter()
            .filter(|message| message.created >= now - RECENT_MS)
        {
            for part in source.part_ids(&message.id)? {
                let Some(data) = source
                    .small_part(&part)?
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
                        part,
                        data,
                        plan,
                    });
                }
            }
        }
    }
    writes.sort_by(|a, b| (b.created, &b.part).cmp(&(a.created, &a.part)));
    let mut files = Files::default();
    let mut records = Records::new();
    for write in &writes {
        if let Some(record) = record(write, &mut files, blobs) {
            records.insert(write.part.clone(), record);
        }
    }
    Ok(records)
}

/// One version each file passed through, newest known first: what it holds just after the next older call.
#[derive(Default)]
struct Files {
    slots: HashMap<String, Option<Option<String>>>,
}

impl Files {
    /// The file's content after the next older call, read from disk the first time; `None` once its history is lost.
    fn current(&mut self, path: &Path) -> Option<Option<String>> {
        self.slots.entry(key(path)).or_insert_with(|| read(path)).clone()
    }

    fn set(&mut self, path: &Path, content: Option<Option<String>>) {
        self.slots.insert(key(path), content);
    }
}

fn key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn read(path: &Path) -> Option<Option<String>> {
    match std::fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes).ok().map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(None),
        Err(_) => None,
    }
}

/// One file a call changed, its content before and after.
struct Change {
    path: PathBuf,
    before: Option<String>,
    after: Option<String>,
}

/// The call's record, every file or none; a call that cannot be rebuilt loses the history of its files.
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

/// Inside the workspace a path is relative with `/`, as a native record's is; outside it stays absolute.
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
    if path.is_absolute()
        || path
            .to_string_lossy()
            .get(1..3)
            .is_some_and(|drive| drive == ":\\" || drive == ":/")
    {
        return path.to_path_buf();
    }
    Path::new(&write.plan.session.directory).join(path)
}

/// Every file the call names, so a call that cannot be rebuilt ends their histories.
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
    let before = reverse(diff["patch"].as_str().or(state["metadata"]["diff"].as_str())?, &after)?;
    // An edit with nothing to replace made the file.
    let made = state["input"]["oldString"] == "" && before.is_empty();
    Some(Change {
        path,
        before: (!made).then_some(before),
        after: Some(after),
    })
}

/// One file of a patch, two for a move: its new path appears and its old path goes.
fn patched(write: &Write, file: &Value, files: &mut Files) -> Option<Vec<Change>> {
    let path = resolve(file["filePath"].as_str()?, write);
    let kind = file["type"].as_str()?;
    let moved = file["movePath"]
        .as_str()
        .filter(|_| kind == "move")
        .map(|to| resolve(to, write));
    let now = files.current(moved.as_ref().unwrap_or(&path))?;
    let before = match (file["before"].as_str(), file["after"].as_str()) {
        (Some(before), Some(recorded)) if (kind == "delete" && now.is_none()) || now.as_deref() == Some(recorded) => {
            (kind != "add").then(|| before.to_string())
        }
        (Some(_), Some(_)) => return None,
        _ => original(file["patch"].as_str().or(file["diff"].as_str())?, kind, now.as_deref())?,
    };
    match moved {
        Some(to) => files.current(&path)?.is_none().then(|| {
            vec![
                Change {
                    path: to,
                    before: None,
                    after: now,
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
            after: now,
        }]),
    }
}

/// The file before a patch from what it holds now: absent before an add, absent after a delete.
fn original(diff: &str, kind: &str, now: Option<&str>) -> Option<Option<String>> {
    match kind {
        "add" => reverse(diff, now?)?.is_empty().then_some(None),
        "delete" if now.is_none() => reverse(diff, "").map(Some),
        "update" | "move" => reverse(diff, now?).map(Some),
        _ => None,
    }
}

struct Line {
    kind: u8,
    text: String,
    newline: bool,
}

struct Hunk {
    new_start: usize,
    new_len: usize,
    lines: Vec<Line>,
}

/// `current` with a unified diff undone: each hunk's new side must match exactly (line endings
/// aside) where it says, and is replaced by its old side in the file's own line ending.
fn reverse(diff: &str, current: &str) -> Option<String> {
    let hunks = parse(diff)?;
    let eol = if current.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = current.split_inclusive('\n').map(String::from).collect();
    for hunk in hunks.iter().rev() {
        let start = if hunk.new_len == 0 {
            hunk.new_start
        } else {
            hunk.new_start.checked_sub(1)?
        };
        let end = start.checked_add(hunk.new_len).filter(|end| *end <= lines.len())?;
        let mut at = start;
        let mut replacement = Vec::new();
        for line in &hunk.lines {
            if line.kind == b'-' {
                replacement.push(format!("{}{}", line.text, if line.newline { eol } else { "" }));
                continue;
            }
            if bare(&lines[at]) != line.text {
                return None;
            }
            if line.kind == b' ' {
                replacement.push(lines[at].clone());
            }
            at += 1;
        }
        lines.splice(start..end, replacement);
    }
    Some(lines.concat())
}

fn bare(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

/// The hunks of a one-file unified diff, read by their line counts so content that looks like a header stays content.
fn parse(diff: &str) -> Option<Vec<Hunk>> {
    let mut lines = diff.split('\n').peekable();
    let mut hunks = Vec::new();
    while let Some(line) = lines.next() {
        let Some(header) = line.strip_prefix("@@ -") else {
            continue;
        };
        let (old, rest) = header.split_once(" +")?;
        let (_, mut old_left) = range(old)?;
        let (new_start, new_len) = range(rest.split_once(" @@")?.0)?;
        let mut new_left = new_len;
        let mut body: Vec<Line> = Vec::new();
        while old_left > 0 || new_left > 0 {
            let line = lines.next()?;
            let kind = line.as_bytes().first().copied().unwrap_or(b' ');
            match kind {
                b' ' => (old_left, new_left) = (old_left.checked_sub(1)?, new_left.checked_sub(1)?),
                b'-' => old_left = old_left.checked_sub(1)?,
                b'+' => new_left = new_left.checked_sub(1)?,
                b'\\' => {
                    body.last_mut()?.newline = false;
                    continue;
                }
                _ => return None,
            }
            let text = line.get(1..).unwrap_or_default();
            body.push(Line {
                kind,
                text: text.strip_suffix('\r').unwrap_or(text).to_string(),
                newline: true,
            });
        }
        while lines.next_if(|line| line.starts_with('\\')).is_some() {
            if let Some(last) = body.last_mut() {
                last.newline = false;
            }
        }
        hunks.push(Hunk {
            new_start,
            new_len,
            lines: body,
        });
    }
    Some(hunks)
}

/// `start[,len]`, the length 1 when left out.
fn range(text: &str) -> Option<(usize, usize)> {
    match text.split_once(',') {
        Some((start, len)) => Some((start.parse().ok()?, len.parse().ok()?)),
        None => Some((text.parse().ok()?, 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "Index: a.rs\n===================================================================\n--- a.rs\n+++ a.rs\n@@ -1,4 +1,4 @@\n one\n-two\n+TWO\n three\n four\n@@ -8,2 +8,3 @@\n eight\n nine\n+ten\n";

    #[test]
    fn a_diff_is_undone_where_it_says_in_the_files_own_line_endings() {
        let now = "one\nTWO\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n";
        assert_eq!(
            reverse(DIFF, now).as_deref(),
            Some("one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\n")
        );
        let crlf = now.replace('\n', "\r\n");
        assert_eq!(
            reverse(DIFF, &crlf).as_deref(),
            Some("one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\n")
        );
    }

    #[test]
    fn a_file_changed_since_the_diff_is_not_undone() {
        assert_eq!(
            reverse(DIFF, "one\nTWO\nthree\nFOUR\nfive\nsix\nseven\neight\nnine\nten\n"),
            None,
            "a context line differs"
        );
        assert_eq!(reverse(DIFF, "one\nTWO\n"), None, "the file is shorter than the diff");
    }

    #[test]
    fn added_and_deleted_files_and_a_missing_final_newline_round_trip() {
        let added = "--- /dev/null\n+++ b.rs\n@@ -0,0 +1,2 @@\n+x\n+y\n\\ No newline at end of file\n";
        assert_eq!(reverse(added, "x\ny").as_deref(), Some(""));
        let deleted = "--- b.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-x\n-y\n\\ No newline at end of file\n";
        assert_eq!(reverse(deleted, "").as_deref(), Some("x\ny"));
        let tail = "@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+c\n";
        assert_eq!(
            reverse(tail, "a\nc\n").as_deref(),
            Some("a\nb"),
            "the old last line had no newline"
        );
    }

    #[test]
    fn lines_that_look_like_headers_are_read_as_content() {
        let diff = "@@ -1,2 +1,2 @@\n--- old dashes\n+++ new pluses\n keep\n";
        assert_eq!(
            reverse(diff, "++ new pluses\nkeep\n").as_deref(),
            Some("-- old dashes\nkeep\n")
        );
    }

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
