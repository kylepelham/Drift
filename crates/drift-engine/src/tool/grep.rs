use std::path::Path;

use globset::GlobBuilder;
use grep::regex::RegexMatcherBuilder;
use grep::searcher::sinks::UTF8;
use grep::searcher::SearcherBuilder;
use ignore::WalkBuilder;
use serde_json::{json, Value};

use super::{display, required_str, Ask, Context, Output, RunFuture, Tool, ToolError};
use crate::llm::ToolSpec;

const MAX_MATCHES: usize = 200;
const MAX_LINE_CHARS: usize = 300;

pub struct Grep;

impl Tool for Grep {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "grep".into(),
            description: include_str!("prompts/grep.txt").trim().into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regular expression (Rust/ripgrep syntax) to search for." },
                    "path": { "type": "string", "description": "Directory or file to search. Default: the workspace." },
                    "include": { "type": "string", "description": "Only search files matching this glob, such as `*.rs` or `src/**/*.ts`." }
                },
                "required": ["pattern"]
            }),
        }
    }

    fn ask(&self, ctx: &Context, input: &Value) -> Option<Ask> {
        ctx.ask_if_outside("read", &ctx.resolve(input["path"].as_str().unwrap_or(".")), "Search")
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let pattern = required_str(&input, "pattern")?.to_string();
            let root = ctx.resolve(input["path"].as_str().unwrap_or("."));
            let include = input["include"].as_str().map(str::to_string);
            let workspace = ctx.workspace.clone();
            let (matches, truncated) = tokio::task::spawn_blocking(move || search(&root, &pattern, include.as_deref(), &workspace))
                .await
                .map_err(|e| ToolError(e.to_string()))??;
            let mut output = if matches.is_empty() { "No matches".to_string() } else { matches.join("\n") };
            if truncated {
                output.push_str(&format!("\n(first {MAX_MATCHES} matches; narrow the pattern or path to see more)"));
            }
            Ok(Output { title: input["pattern"].as_str().unwrap_or_default().into(), output, metadata: json!({ "count": matches.len(), "truncated": truncated }) })
        })
    }
}

fn search(root: &Path, pattern: &str, include: Option<&str>, workspace: &Path) -> Result<(Vec<String>, bool), ToolError> {
    let matcher = RegexMatcherBuilder::new()
        .line_terminator(Some(b'\n'))
        .build(pattern)
        .map_err(|e| ToolError(format!("invalid regex: {e}")))?;
    let include = include
        .map(|glob| GlobBuilder::new(glob).literal_separator(false).build().map(|g| g.compile_matcher()))
        .transpose()
        .map_err(|e| ToolError(format!("invalid include glob: {e}")))?;
    let mut searcher = SearcherBuilder::new().line_number(true).build();
    let mut lines = Vec::new();
    for entry in WalkBuilder::new(root).hidden(false).require_git(false).build().flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let relative = entry.path().strip_prefix(root).unwrap_or(entry.path());
        if include.as_ref().is_some_and(|glob| !glob.is_match(relative) && !glob.is_match(entry.file_name())) {
            continue;
        }
        let name = display(entry.path(), workspace);
        let _ = searcher.search_path(
            &matcher,
            entry.path(),
            UTF8(|line_number, line| {
                lines.push(format!("{name}:{line_number}: {}", clip(line.trim_end())));
                Ok(lines.len() <= MAX_MATCHES)
            }),
        );
        if lines.len() > MAX_MATCHES {
            lines.truncate(MAX_MATCHES);
            return Ok((lines, true));
        }
    }
    Ok((lines, false))
}

fn clip(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        return line.to_string();
    }
    format!("{}...", line.chars().take(MAX_LINE_CHARS).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::super::tests::Sandbox;
    use super::*;

    #[tokio::test]
    async fn finds_lines_with_numbers_and_filters_by_include() {
        let sandbox = Sandbox::new("grep");
        sandbox.file("src/a.rs", "fn alpha() {}\nfn beta() {}\n");
        sandbox.file("notes.md", "alpha in prose\n");
        let out = Grep.run(&sandbox.ctx, json!({ "pattern": "alpha" })).await.unwrap();
        let mut lines: Vec<&str> = out.output.lines().collect();
        lines.sort();
        assert_eq!(lines, ["notes.md:1: alpha in prose", "src/a.rs:1: fn alpha() {}"]);
        let only = Grep.run(&sandbox.ctx, json!({ "pattern": "alpha", "include": "*.rs" })).await.unwrap();
        assert_eq!(only.output, "src/a.rs:1: fn alpha() {}");
        let none = Grep.run(&sandbox.ctx, json!({ "pattern": "gamma" })).await.unwrap();
        assert_eq!(none.output, "No matches");
    }

    #[tokio::test]
    async fn bad_regex_is_reported() {
        let sandbox = Sandbox::new("grep-bad");
        let err = Grep.run(&sandbox.ctx, json!({ "pattern": "(" })).await.unwrap_err();
        assert!(err.0.starts_with("invalid regex"));
    }
}
