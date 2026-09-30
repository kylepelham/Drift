use std::path::Path;
use std::time::SystemTime;

use globset::GlobBuilder;
use serde_json::{json, Value};

use super::{display, required_str, Ask, Context, Output, RunFuture, Tool, ToolError};
use crate::llm::ToolSpec;

const MAX_RESULTS: usize = 100;

pub struct Glob;

impl Tool for Glob {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "glob".into(),
            description: include_str!("prompts/glob.txt").trim().into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob such as `src/**/*.ts` or `**/Cargo.toml`, relative to `path`." },
                    "path": { "type": "string", "description": "Directory to search. Default: the workspace." }
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
            let workspace = ctx.workspace.clone();
            let (mut found, truncated) = tokio::task::spawn_blocking(move || find(&root, &pattern)).await.map_err(|e| ToolError(e.to_string()))??;
            found.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
            let mut lines: Vec<String> = found.iter().map(|(path, _)| display(path, &workspace)).collect();
            if truncated {
                lines.push(format!("(first {MAX_RESULTS} matches, newest first; narrow the pattern to see more)"));
            }
            let output = if lines.is_empty() { "No files matched".to_string() } else { lines.join("\n") };
            Ok(Output { title: input["pattern"].as_str().unwrap_or_default().into(), output, metadata: json!({ "count": found.len(), "truncated": truncated }) })
        })
    }
}

type Found = Vec<(std::path::PathBuf, SystemTime)>;

fn find(root: &Path, pattern: &str) -> Result<(Found, bool), ToolError> {
    let glob = GlobBuilder::new(pattern.trim_start_matches("./"))
        .literal_separator(true)
        .build()
        .map_err(|e| ToolError(format!("invalid glob: {e}")))?
        .compile_matcher();
    let mut found = Vec::new();
    for entry in super::walk(root).flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let relative = entry.path().strip_prefix(root).unwrap_or(entry.path());
        if !glob.is_match(relative) {
            continue;
        }
        let modified = entry.metadata().ok().and_then(|m| m.modified().ok()).unwrap_or(SystemTime::UNIX_EPOCH);
        found.push((entry.into_path(), modified));
        if found.len() > MAX_RESULTS {
            found.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
            found.truncate(MAX_RESULTS);
            return Ok((found, true));
        }
    }
    Ok((found, false))
}

#[cfg(test)]
mod tests {
    use super::super::tests::Sandbox;
    use super::*;

    #[tokio::test]
    async fn matches_recursively_and_respects_gitignore() {
        let sandbox = Sandbox::new("glob");
        sandbox.file("src/a.rs", "");
        sandbox.file("src/deep/b.rs", "");
        sandbox.file("src/c.txt", "");
        sandbox.file("target/ignored.rs", "");
        sandbox.file(".gitignore", "target/\n");
        let out = Glob.run(&sandbox.ctx, json!({ "pattern": "**/*.rs" })).await.unwrap();
        let mut lines: Vec<&str> = out.output.lines().collect();
        lines.sort();
        assert_eq!(lines, ["src/a.rs", "src/deep/b.rs"]);
        let none = Glob.run(&sandbox.ctx, json!({ "pattern": "*.py" })).await.unwrap();
        assert_eq!(none.output, "No files matched");
    }

    #[tokio::test]
    async fn never_lists_version_control_internals() {
        let sandbox = Sandbox::new("glob-git");
        sandbox.file(".git/HEAD", "ref: refs/heads/main\n");
        sandbox.file(".git/hooks/pre-commit", "");
        sandbox.file(".github/workflows/ci.yml", "");
        sandbox.file(".env", "");
        let out = Glob.run(&sandbox.ctx, json!({ "pattern": "**/*" })).await.unwrap();
        let mut lines: Vec<&str> = out.output.lines().collect();
        lines.sort();
        assert_eq!(lines, [".env", ".github/workflows/ci.yml"], "names are listed; .git is not");
    }
}
