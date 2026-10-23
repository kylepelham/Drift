//! The files a worker changed, totalled per file over its finished file-tool calls, as a turn's card lists them.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::TaskRecord;
use crate::Engine;
use crate::store::EditCall;

/// One file a worker changed, with the lines its calls added and removed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkerEdit {
    pub path: String,
    pub additions: usize,
    pub deletions: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Recorded {
    file_path: String,
    additions: usize,
    deletions: usize,
}

impl Engine {
    /// A record as the API shows it: an ended worker carries the files it changed.
    pub fn with_edits(&self, task: TaskRecord) -> rusqlite::Result<TaskRecord> {
        let mut tasks = self.edited(vec![task])?;

        Ok(tasks.remove(0))
    }

    /// A session's workers as its list shows them: without their results, each ended one with the files it changed.
    pub fn listed_tasks(&self, parent: &str) -> rusqlite::Result<Vec<TaskRecord>> {
        let tasks = self.edited(self.store.tasks_of(parent)?)?;

        Ok(tasks
            .into_iter()
            .map(|task| TaskRecord { result: None, ..task })
            .collect())
    }

    /// Reads each ended worker's files from its transcript the first time, once per transcript, and keeps
    /// them all in one write.
    fn edited(&self, mut tasks: Vec<TaskRecord>) -> rusqlite::Result<Vec<TaskRecord>> {
        let mut read: HashMap<String, Vec<WorkerEdit>> = HashMap::new();
        let mut kept = Vec::new();
        for task in &mut tasks {
            if task.edits.is_some() || !task.state.is_terminal() {
                continue;
            }
            if !read.contains_key(&task.session_id) {
                let edits = totals(&self.store.edit_calls(&task.session_id)?);
                read.insert(task.session_id.clone(), edits);
            }
            task.edits = read.get(&task.session_id).cloned();
            kept.push(task.clone());
        }
        if !kept.is_empty() {
            self.store.save_task_edits(&kept)?;
        }

        Ok(tasks)
    }
}

/// One row per file, in the order the files were first changed; a path differing only in case or slashes is the same file.
pub(super) fn totals(calls: &[EditCall]) -> Vec<WorkerEdit> {
    let mut edits: Vec<WorkerEdit> = Vec::new();
    let mut rows: HashMap<String, usize> = HashMap::new();
    for change in calls.iter().flat_map(changes) {
        let key = change.path.replace('\\', "/").to_lowercase();
        match rows.get(&key) {
            Some(&row) => {
                edits[row].additions += change.additions;
                edits[row].deletions += change.deletions;
            }
            None => {
                rows.insert(key, edits.len());
                edits.push(change);
            }
        }
    }

    edits
}

/// A call's per-file records, else the file it names with its diff's counts (older calls kept only that).
fn changes(call: &EditCall) -> Vec<WorkerEdit> {
    let recorded = [&call.file_changes, &call.files]
        .into_iter()
        .flatten()
        .map(|json| records(json))
        .find(|records| !records.is_empty());
    if let Some(recorded) = recorded {
        return recorded;
    }

    match (&call.path, &call.diff) {
        (Some(path), Some(diff)) if !diff.trim().is_empty() => {
            let (additions, deletions) = counted(diff);
            vec![WorkerEdit {
                path: path.clone(),
                additions,
                deletions,
            }]
        }
        _ => Vec::new(),
    }
}

/// The entries of a JSON array that are per-file records; a bare path among them is skipped.
fn records(json: &str) -> Vec<WorkerEdit> {
    let values: Vec<serde_json::Value> = serde_json::from_str(json).unwrap_or_default();

    values
        .into_iter()
        .filter_map(|value| serde_json::from_value::<Recorded>(value).ok())
        .map(|record| WorkerEdit {
            path: record.file_path,
            additions: record.additions,
            deletions: record.deletions,
        })
        .collect()
}

/// Lines added and removed inside a unified diff's hunks, each hunk read only as far as its header says.
fn counted(diff: &str) -> (usize, usize) {
    let (mut additions, mut deletions) = (0, 0);
    let (mut old, mut new) = (0_i64, 0_i64);
    for line in diff.lines() {
        if let Some(lengths) = hunk(line) {
            (old, new) = lengths;
            continue;
        }
        if old <= 0 && new <= 0 {
            continue;
        }
        match line.as_bytes().first() {
            Some(b'+') => (additions, new) = (additions + 1, new - 1),
            Some(b'-') => (deletions, old) = (deletions + 1, old - 1),
            Some(b' ') => (old, new) = (old - 1, new - 1),
            _ => {}
        }
    }

    (additions, deletions)
}

/// The old and new lengths a `@@ -a,b +c,d @@` header gives; a length left out is one line.
fn hunk(line: &str) -> Option<(i64, i64)> {
    let (old, rest) = line.strip_prefix("@@ -")?.split_once(" +")?;
    let (new, _) = rest.split_once(" @@")?;

    Some((length(old)?, length(new)?))
}

fn length(range: &str) -> Option<i64> {
    let (start, length) = range.split_once(',').unwrap_or((range, "1"));
    start.parse::<i64>().ok()?;

    length.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(path: Option<&str>, files: Option<&str>, file_changes: Option<&str>, diff: Option<&str>) -> EditCall {
        EditCall {
            path: path.map(Into::into),
            files: files.map(Into::into),
            file_changes: file_changes.map(Into::into),
            diff: diff.map(Into::into),
        }
    }

    fn edit(path: &str, additions: usize, deletions: usize) -> WorkerEdit {
        WorkerEdit {
            path: path.into(),
            additions,
            deletions,
        }
    }

    #[test]
    fn calls_total_per_file_in_first_change_order_from_records_or_an_older_calls_diff() {
        let diff =
            "--- a/b.rs\n+++ b/b.rs\n@@ -1,2 +1,3 @@\n keep\n-old\n+new\n+more\n\\ No newline at end of file\n--- x\n";
        let calls = [
            call(
                Some("C:\\w\\a.rs"),
                Some(r#"["C:\\w\\a.rs"]"#),
                Some(r#"[{"filePath":"C:\\w\\a.rs","relativePath":"a.rs","patch":"","additions":2,"deletions":1}]"#),
                None,
            ),
            call(Some("C:/w/b.rs"), None, None, Some(diff)),
            call(
                None,
                Some(r#"[{"filePath":"c:/W/A.rs","additions":1,"deletions":0},"bare"]"#),
                Some("[]"),
                None,
            ),
            call(Some("C:/w/c.rs"), Some(r#"["C:/w/c.rs"]"#), None, Some("  ")),
            call(None, None, None, Some(diff)),
        ];

        assert_eq!(totals(&calls), [edit("C:\\w\\a.rs", 3, 1), edit("C:/w/b.rs", 2, 1)]);
    }

    #[test]
    fn a_hunk_without_lengths_is_one_line_each_side() {
        assert_eq!(counted("@@ -3 +3 @@\n-a\n+b\n+c\n"), (1, 1));
        assert_eq!(counted("@@ -0,0 +1,2 @@\n+a\n+b\n"), (2, 0));
        assert_eq!(hunk("@@ -x +1 @@"), None);
    }
}
