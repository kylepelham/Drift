//! Exact search and replace. No fuzzy cascade: a miss reports the nearest region so the model re-reads.

use serde_json::{json, Value};
use similar::TextDiff;

use super::{display, required_str, Ask, Context, Output, RunFuture, Tool, ToolError};
use crate::llm::ToolSpec;

/// Lines of context shown around the closest region on a miss.
const NEAR_CONTEXT: usize = 3;

pub struct Edit;

impl Tool for Edit {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit".into(),
            description: include_str!("prompts/edit.txt").trim().into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File to edit." },
                    "old_string": { "type": "string", "description": "Exact text to replace, copied from a read." },
                    "new_string": { "type": "string", "description": "Replacement text." },
                    "replace_all": { "type": "boolean", "description": "Replace every occurrence instead of requiring exactly one. Default false." }
                },
                "required": ["path", "old_string", "new_string"]
            }),
        }
    }

    fn ask(&self, ctx: &Context, input: &Value) -> Option<Ask> {
        let path = ctx.resolve(input["path"].as_str()?);
        Some(Ask::path("edit", &path, &ctx.workspace, format!("Edit {}", display(&path, &ctx.workspace))))
    }

    fn mutates(&self) -> bool {
        true
    }

    fn touches(&self, ctx: &Context, input: &Value) -> Option<Vec<std::path::PathBuf>> {
        Some(vec![ctx.resolve(input["path"].as_str()?)])
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let path = ctx.resolve(required_str(&input, "path")?);
            let old = required_str(&input, "old_string")?;
            let new = input["new_string"].as_str().ok_or(ToolError("`new_string` is required".into()))?;
            let replace_all = input["replace_all"].as_bool().unwrap_or(false);
            let name = display(&path, &ctx.workspace);
            if old == new {
                return Err(ToolError("old_string and new_string are identical".into()));
            }
            if !ctx.files.was_read(&path) {
                return Err(ToolError(format!("{name} has not been read this session; read it before editing")));
            }
            let raw = tokio::fs::read_to_string(&path).await.map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => ToolError(format!("{name} does not exist")),
                _ => ToolError(format!("{name} could not be read as text ({error})")),
            })?;
            let ending = LineEnding::detect(&raw);
            let content = ending.normalise(&raw);
            let (updated, replacements) = replace(&content, &ending.normalise(old), &ending.normalise(new), replace_all)?;
            let written = ending.apply(&updated);
            super::fits_history(&name, written.len())?;
            super::stage::replace(&ctx.engine.store, &path, written.as_bytes()).await?;
            Ok(Output {
                title: name.clone(),
                output: diff(&name, &content, &updated),
                metadata: json!({ "replacements": replacements, "files": [path.to_string_lossy()] }),
            })
        })
    }
}

/// The new content and how many places changed; both sides already share the file's line endings.
fn replace(content: &str, old: &str, new: &str, replace_all: bool) -> Result<(String, usize), ToolError> {
    let count = content.matches(old).count();
    match (count, replace_all) {
        (0, _) => Err(ToolError(miss(content, old))),
        (1, _) | (_, true) => Ok((content.replace(old, new), count)),
        (n, false) => Err(ToolError(format!(
            "old_string matches {n} places; include more surrounding lines to make it unique, or set replace_all"
        ))),
    }
}

/// The window of the file whose lines best overlap the search text, so the model can re-read it.
fn miss(content: &str, old: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let wanted: Vec<&str> = old.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.is_empty() || wanted.is_empty() {
        return "old_string was not found in the file".into();
    }
    let window = wanted.len().min(lines.len());
    let score = |start: usize| {
        lines[start..start + window]
            .iter()
            .filter(|line| wanted.contains(&line.trim()))
            .count()
    };
    let (best, hits) = (0..=lines.len() - window)
        .map(|start| (start, score(start)))
        .max_by_key(|(start, hits)| (*hits, usize::MAX - *start))
        .unwrap_or((0, 0));
    if hits == 0 {
        return "old_string was not found in the file; read the file again and copy the text exactly".into();
    }
    let from = best.saturating_sub(NEAR_CONTEXT);
    let to = (best + window + NEAR_CONTEXT).min(lines.len());
    let region: Vec<String> = (from..to).map(|i| format!("{}: {}", i + 1, lines[i])).collect();
    format!(
        "old_string was not found. The closest region is lines {}-{}; copy it exactly:\n{}",
        from + 1,
        to,
        region.join("\n")
    )
}

pub fn diff(name: &str, before: &str, after: &str) -> String {
    TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{name}"), &format!("b/{name}"))
        .to_string()
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn detect(text: &str) -> Self {
        if text.contains("\r\n") {
            Self::CrLf
        } else {
            Self::Lf
        }
    }

    pub fn normalise(self, text: &str) -> String {
        text.replace("\r\n", "\n")
    }

    pub fn apply(self, text: &str) -> String {
        match self {
            Self::Lf => text.to_string(),
            Self::CrLf => text.replace("\r\n", "\n").replace('\n', "\r\n"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::Sandbox;
    use super::*;

    async fn edit(sandbox: &Sandbox, input: Value) -> Result<Output, ToolError> {
        Edit.run(&sandbox.ctx, input).await
    }

    #[tokio::test]
    async fn replaces_a_unique_match_and_returns_a_diff() {
        let sandbox = Sandbox::new("edit");
        let path = sandbox.file("a.rs", "fn a() {}\nfn b() {}\n");
        sandbox.ctx.files.mark_read(&path);
        let out = edit(&sandbox, json!({ "path": "a.rs", "old_string": "fn b() {}", "new_string": "fn c() {}" })).await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fn a() {}\nfn c() {}\n");
        assert!(out.output.contains("-fn b() {}\n+fn c() {}"));
    }

    #[tokio::test]
    async fn crlf_files_match_lf_search_strings_and_stay_crlf() {
        let sandbox = Sandbox::new("edit-crlf");
        let path = sandbox.file("w.txt", "one\r\ntwo\r\nthree\r\n");
        sandbox.ctx.files.mark_read(&path);
        edit(&sandbox, json!({ "path": "w.txt", "old_string": "one\ntwo", "new_string": "uno\ndos" })).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"uno\r\ndos\r\nthree\r\n");
    }

    #[tokio::test]
    async fn ambiguous_matches_need_replace_all() {
        let sandbox = Sandbox::new("edit-multi");
        let path = sandbox.file("m.txt", "x\ny\nx\n");
        sandbox.ctx.files.mark_read(&path);
        let err = edit(&sandbox, json!({ "path": "m.txt", "old_string": "x", "new_string": "z" })).await.unwrap_err();
        assert!(err.0.contains("matches 2 places"));
        let out = edit(&sandbox, json!({ "path": "m.txt", "old_string": "x", "new_string": "z", "replace_all": true })).await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "z\ny\nz\n");
        assert_eq!(out.metadata["replacements"], 2);
    }

    #[tokio::test]
    async fn a_crlf_search_string_counts_the_places_it_changed() {
        let sandbox = Sandbox::new("edit-crlf-count");
        let path = sandbox.file("c.txt", "x\r\ny\r\nx\r\ny\r\n");
        sandbox.ctx.files.mark_read(&path);
        let out = edit(&sandbox, json!({ "path": "c.txt", "old_string": "x\r\ny", "new_string": "z", "replace_all": true })).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"z\r\nz\r\n");
        assert_eq!(out.metadata["replacements"], 2);
    }

    #[tokio::test]
    async fn a_miss_points_at_the_closest_region() {
        let sandbox = Sandbox::new("edit-miss");
        let path = sandbox.file("f.rs", "a\nb\nfn target() {\n    let x = 1;\n}\nc\nd\ne\nf\n");
        sandbox.ctx.files.mark_read(&path);
        let err = edit(&sandbox, json!({ "path": "f.rs", "old_string": "fn target() {\n    let x = 2;\n}", "new_string": "" })).await.unwrap_err();
        assert!(err.0.contains("closest region is lines 1-8"), "{}", err.0);
        assert!(err.0.contains("4:     let x = 1;"));
    }

    #[tokio::test]
    async fn unread_files_are_refused() {
        let sandbox = Sandbox::new("edit-unread");
        sandbox.file("u.txt", "a\n");
        let err = edit(&sandbox, json!({ "path": "u.txt", "old_string": "a", "new_string": "b" })).await.unwrap_err();
        assert!(err.0.contains("has not been read"));
    }

    #[tokio::test]
    async fn a_write_that_fails_once_begun_leaves_the_file_whole() {
        use crate::tool::stage::tests::{inject, leftovers, Fault};
        let sandbox = Sandbox::new("edit-fails");
        let path = sandbox.file("w.txt", "one\r\ntwo\r\n");
        sandbox.ctx.files.mark_read(&path);
        inject(Fault::AfterStaging, &path);
        let err = edit(&sandbox, json!({ "path": "w.txt", "old_string": "two", "new_string": "TWO" })).await.unwrap_err();
        assert!(err.0.contains("injected"), "{}", err.0);
        assert_eq!(std::fs::read(&path).unwrap(), b"one\r\ntwo\r\n", "not cut short");
        assert!(leftovers(&sandbox.ctx.workspace).is_empty());
        edit(&sandbox, json!({ "path": "w.txt", "old_string": "two", "new_string": "TWO" })).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"one\r\nTWO\r\n", "line endings kept through the staged write");
    }

    #[test]
    fn no_overlap_gives_a_plain_miss() {
        assert!(miss("a\nb\n", "zzz").contains("read the file again"));
    }
}
