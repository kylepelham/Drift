use serde_json::{json, Value};

use super::{display, required_str, Ask, Context, Output, RunFuture, Tool, ToolError};
use crate::llm::ToolSpec;

const MAX_LINES: usize = 2000;
const MAX_LINE_CHARS: usize = 2000;
const MAX_BYTES: u64 = 10 * 1024 * 1024;

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
        ctx.ask_if_outside("read", &ctx.resolve(input["path"].as_str()?), "Read")
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
            let body: Vec<String> = text
                .lines()
                .enumerate()
                .skip(offset - 1)
                .take(limit)
                .map(|(index, line)| format!("{}: {}", index + 1, truncate(line)))
                .collect();
            let shown = body.len();
            let mut output = body.join("\n");
            if offset - 1 + shown < total {
                output.push_str(&format!("\n\n({} more lines; read with offset {})", total - (offset - 1 + shown), offset + shown));
            }
            ctx.files.mark_read(&path);
            Ok(Output {
                title: display(&path, &ctx.workspace),
                output,
                metadata: json!({ "lines": total, "shown": shown }),
            })
        })
    }
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
    Ok(Output::new(display(path, &ctx.workspace), names.join("\n")))
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
