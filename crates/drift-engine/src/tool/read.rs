use serde_json::{json, Value};

use super::{display, required_str, Ask, Context, Output, RunFuture, Tool, ToolError};
use crate::llm::ToolSpec;

const MAX_LINES: usize = 2000;
const MAX_LINE_CHARS: usize = 2000;
const MAX_BYTES: u64 = 10 * 1024 * 1024;
/// One page stays under the shared result bound, leaving room for the continuation note.
const PAGE_BYTES: usize = super::spool::MAX_RESULT_BYTES - 1024;
/// Entries a directory listing shows before saying how many more there are.
const MAX_ENTRIES: usize = 1000;

pub struct Read;

impl Tool for Read {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read".into(),
            description: include_str!("prompts/read.txt").trim().into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File to read. Relative paths resolve against the workspace." },
                    "offset": { "type": "integer", "description": "1-based line to start from. Default 1." },
                    "limit": { "type": "integer", "description": "Maximum lines to return. Default 2000." }
                },
                "required": ["path"]
            }),
        }
    }

    fn ask(&self, ctx: &Context, input: &Value) -> Option<Ask> {
        ctx.ask_to_read(&ctx.resolve(input["path"].as_str()?), "Read")
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let path = ctx.resolve(required_str(&input, "path")?);
            let offset = input["offset"].as_u64().unwrap_or(1).max(1) as usize;
            let limit = input["limit"].as_u64().map_or(MAX_LINES, |l| l as usize).clamp(1, MAX_LINES);
            let meta = tokio::fs::metadata(&path).await.map_err(|_| ToolError(format!("{} does not exist", display(&path, &ctx.workspace))))?;
            if meta.is_dir() {
                return list_dir(ctx, &path).await;
            }
            if meta.len() > MAX_BYTES {
                return Err(ToolError(format!("{} is {} bytes; too large to read", display(&path, &ctx.workspace), meta.len())));
            }
            let bytes = tokio::fs::read(&path).await?;
            if bytes.iter().take(8000).any(|b| *b == 0) {
                return Err(ToolError(format!("{} is binary", display(&path, &ctx.workspace))));
            }
            let text = String::from_utf8_lossy(&bytes);
            let total = text.lines().count();
            let body = page(&text, offset, limit);
            let shown = body.len();
            let mut output = body.join("\n");
            if offset - 1 + shown < total {
                output.push_str(&format!("\n\n({} more lines; read with offset {})", total - (offset - 1 + shown), offset + shown));
            }
            ctx.files.mark_read(&path);
            for (file, text) in crate::config::nested_instructions(&ctx.workspace, &path) {
                if file != path && ctx.files.first_showing(&file) {
                    output.push_str(&format!("\n\n<system-reminder>\nInstructions from {}, for files under it:\n{text}\n</system-reminder>", display(&file, &ctx.workspace)));
                }
            }
            Ok(Output {
                title: display(&path, &ctx.workspace),
                output,
                metadata: json!({ "lines": total, "shown": shown }),
            })
        })
    }
}

/// Numbered lines from `offset`, at most `limit` of them and within the page budget; always at least
/// one line, so every read makes progress.
fn page(text: &str, offset: usize, limit: usize) -> Vec<String> {
    let mut used = 0;
    let mut lines = Vec::new();
    for (index, line) in text.lines().enumerate().skip(offset - 1).take(limit) {
        let numbered = format!("{}: {}", index + 1, truncate(line));
        used += numbered.len() + 1;
        if used > PAGE_BYTES && !lines.is_empty() {
            break;
        }
        lines.push(numbered);
    }
    lines
}

fn truncate(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        return line.to_string();
    }
    let cut: String = line.chars().take(MAX_LINE_CHARS).collect();
    format!("{cut}...")
}

async fn list_dir(ctx: &Context, path: &std::path::Path) -> Result<Output, ToolError> {
    let mut entries = tokio::fs::read_dir(path).await?;
    let mut names = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let suffix = if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) { "/" } else { "" };
        names.push(format!("{}{suffix}", entry.file_name().to_string_lossy()));
    }
    names.sort();
    let total = names.len();
    names.truncate(MAX_ENTRIES);
    let mut output = names.join("\n");
    if total > MAX_ENTRIES {
        output.push_str(&format!("\n\n({} more entries; use glob with a pattern to narrow it)", total - MAX_ENTRIES));
    }
    Ok(Output::new(display(path, &ctx.workspace), output))
}

#[cfg(test)]
mod tests {
    use super::super::tests::Sandbox;
    use super::*;

    #[tokio::test]
    async fn numbers_lines_and_pages() {
        let sandbox = Sandbox::new("read");
        sandbox.file("a.txt", "one\ntwo\nthree\nfour\n");
        let out = Read.run(&sandbox.ctx, json!({ "path": "a.txt", "offset": 2, "limit": 2 })).await.unwrap();
        assert_eq!(out.output, "2: two\n3: three\n\n(1 more lines; read with offset 4)");
        assert_eq!(out.title, "a.txt");
        assert!(sandbox.ctx.files.was_read(&sandbox.ctx.workspace.join("a.txt")));
    }

    #[tokio::test]
    async fn directories_list_entries_and_missing_files_error() {
        let sandbox = Sandbox::new("read-dir");
        sandbox.file("src/main.rs", "");
        sandbox.file("README.md", "");
        let out = Read.run(&sandbox.ctx, json!({ "path": "." })).await.unwrap();
        assert_eq!(out.output, "README.md\nsrc/");
        let err = Read.run(&sandbox.ctx, json!({ "path": "nope.txt" })).await.unwrap_err();
        assert_eq!(err.0, "nope.txt does not exist");
    }

    #[test]
    fn paths_inside_the_workspace_need_no_permission() {
        let sandbox = Sandbox::new("read-ask");
        assert!(Read.ask(&sandbox.ctx, &json!({ "path": "a.txt" })).is_none());
        let outside = Read.ask(&sandbox.ctx, &json!({ "path": "C:/Windows/hosts" })).unwrap();
        assert_eq!(outside.kind, "read");
    }

    #[tokio::test]
    async fn pages_stay_within_the_result_bound_and_say_where_to_go_on() {
        let sandbox = Sandbox::new("read-page");
        let wide = "w".repeat(1_500);
        sandbox.file("wide.txt", &format!("{wide}\n").repeat(1_000));
        let first = Read.run(&sandbox.ctx, json!({ "path": "wide.txt" })).await.unwrap();
        assert!(first.output.len() <= super::super::spool::MAX_RESULT_BYTES, "{}", first.output.len());
        let shown = first.metadata["shown"].as_u64().unwrap() as usize;
        assert!(shown > 10 && shown < 1_000);
        assert!(first.output.ends_with(&format!("read with offset {})", shown + 1)), "{}", &first.output[first.output.len() - 80..]);
        let next = Read.run(&sandbox.ctx, json!({ "path": "wide.txt", "offset": shown + 1 })).await.unwrap();
        assert!(next.output.starts_with(&format!("{}: w", shown + 1)));

        let huge_line = "h".repeat(super::super::spool::MAX_RESULT_BYTES * 2);
        sandbox.file("one.txt", &huge_line);
        let one = Read.run(&sandbox.ctx, json!({ "path": "one.txt" })).await.unwrap();
        assert!(one.output.starts_with("1: hhh") && one.output.len() < 3_000, "a single line is cut, not skipped");
    }

    #[tokio::test]
    async fn a_subdirectorys_instructions_come_with_the_first_read_under_it() {
        let sandbox = Sandbox::new("read-nested");
        sandbox.file("AGENTS.md", "root rules, already in the system prompt");
        sandbox.file("pkg/AGENTS.md", "pkg rules");
        sandbox.file("pkg/web/CLAUDE.md", "web rules");
        sandbox.file("pkg/web/a.ts", "a");
        sandbox.file("pkg/web/b.ts", "b");
        let first = Read.run(&sandbox.ctx, json!({ "path": "pkg/web/a.ts" })).await.unwrap();
        let pkg = first.output.find("pkg rules").expect("pkg/AGENTS.md is shown");
        assert!(pkg < first.output.find("web rules").expect("pkg/web/CLAUDE.md too"), "outermost first");
        assert!(!first.output.contains("root rules"), "the workspace's own file is not repeated");
        let second = Read.run(&sandbox.ctx, json!({ "path": "pkg/web/b.ts" })).await.unwrap();
        assert_eq!(second.output, "1: b", "shown once per session");
        let itself = Read.run(&sandbox.ctx, json!({ "path": "pkg/AGENTS.md" })).await.unwrap();
        assert_eq!(itself.output, "1: pkg rules");
    }

    #[tokio::test]
    async fn a_large_directory_lists_its_first_entries_and_the_count_of_the_rest() {
        let sandbox = Sandbox::new("read-many");
        for i in 0..1_200 {
            sandbox.file(&format!("many/f{i:04}.txt"), "");
        }
        let out = Read.run(&sandbox.ctx, json!({ "path": "many" })).await.unwrap();
        assert_eq!(out.output.lines().filter(|l| l.starts_with('f')).count(), MAX_ENTRIES);
        assert!(out.output.ends_with("(200 more entries; use glob with a pattern to narrow it)"));
    }

    #[test]
    fn this_sessions_own_spilled_output_reads_without_asking_and_nothing_else_of_the_data_dir_does() {
        let sandbox = Sandbox::new("read-own-output");
        let data = &sandbox.ctx.engine.data_dir;
        let own = data.join("tool-output").join(&sandbox.ctx.session_id).join("call_1.log");
        let other = data.join("tool-output").join("ses_someone_else").join("call_1.log");
        for file in [&own, &other] {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "output").unwrap();
        }
        let ask = |path: &std::path::Path| Read.ask(&sandbox.ctx, &json!({ "path": path.to_string_lossy() }));
        assert!(ask(&own).is_none(), "its own output");
        assert!(ask(&other).is_some(), "another session's output");
        assert!(ask(&data.join("drift.db")).is_some(), "the rest of the data dir");
        let escape = data.join("tool-output").join(&sandbox.ctx.session_id).join("..").join("ses_someone_else").join("call_1.log");
        assert!(ask(&escape).is_some(), "resolved before it is judged");
    }

    #[test]
    fn secrets_inside_the_workspace_ask_and_their_examples_do_not() {
        let sandbox = Sandbox::new("read-secret");
        let ask = Read.ask(&sandbox.ctx, &json!({ "path": "config/.env.local" })).expect("a secret must ask");
        assert_eq!(ask.kind, "read");
        assert_eq!(std::path::PathBuf::from(&ask.pattern), sandbox.ctx.workspace.join("config/.env.local"));
        assert!(ask.title.contains("may hold secrets"), "{}", ask.title);
        assert!(Read.ask(&sandbox.ctx, &json!({ "path": "config/.env.example" })).is_none());
        assert!(Read.ask(&sandbox.ctx, &json!({ "path": "config" })).is_none(), "listing a directory shows names, not contents");
    }
}

#[cfg(test)]
mod escape_tests {
    use super::super::tests::Sandbox;
    use super::*;

    #[test]
    fn dotdot_and_symlinks_resolve_to_the_real_target_before_the_ask() {
        let sandbox = Sandbox::new("read-escape");
        let outside = sandbox.ctx.workspace.parent().unwrap().join("outside.txt");
        std::fs::write(&outside, "secret").unwrap();
        let ask = Read.ask(&sandbox.ctx, &json!({ "path": "../outside.txt" })).expect("traversal must ask");
        assert_eq!(std::path::PathBuf::from(&ask.pattern), outside);
        assert!(Read.ask(&sandbox.ctx, &json!({ "path": "sub/../a.txt" })).is_none());
        assert!(Glob.ask(&sandbox.ctx, &json!({ "pattern": "*", "path": ".." })).is_some());
        assert!(Grep.ask(&sandbox.ctx, &json!({ "pattern": "x", "path": &outside.parent().unwrap().to_string_lossy() })).is_some());

        let link = sandbox.ctx.workspace.join("link");
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(outside.parent().unwrap(), &link).is_ok();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(outside.parent().unwrap(), &link).is_ok();
        if made {
            let ask = Read.ask(&sandbox.ctx, &json!({ "path": "link/outside.txt" })).expect("symlink escape must ask");
            assert_eq!(std::path::PathBuf::from(&ask.pattern), outside);
        }
        std::fs::remove_file(outside).ok();
    }

    use super::super::glob::Glob;
    use super::super::grep::Grep;
}
