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

    fn asks(&self, ctx: &Context, input: &Value) -> Vec<Ask> {
        self.ask(ctx, input).into_iter().chain(input["pattern"].as_str().map(|pattern| Ask::new("glob", pattern, format!("Find {pattern}")).allow_by_default())).collect()
    }

    fn starts_early(&self) -> bool {
        true
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let pattern = required_str(&input, "pattern")?.to_string();
            let root = ctx.resolve(input["path"].as_str().unwrap_or("."));
            let workspace = ctx.workspace.clone();
            let (found, total) = tokio::task::spawn_blocking(move || find(&root, &pattern)).await.map_err(|e| ToolError(e.to_string()))??;
            let truncated = total > found.len();
            let mut lines: Vec<String> = found.iter().map(|(path, _)| display(path, &workspace)).collect();
            if truncated {
                lines.push(format!("(the {MAX_RESULTS} newest of {total} matches; narrow the pattern to see the rest)"));
            }
            let output = if lines.is_empty() { "No files matched".to_string() } else { lines.join("\n") };
            Ok(Output { title: input["pattern"].as_str().unwrap_or_default().into(), output, metadata: json!({ "count": found.len(), "total": total, "truncated": truncated }) })
        })
    }
}

type Found = Vec<(std::path::PathBuf, SystemTime)>;

/// The newest [`MAX_RESULTS`] matches of the whole walk, newest first, and how many matched in all.
fn find(root: &Path, pattern: &str) -> Result<(Found, usize), ToolError> {
    let glob = GlobBuilder::new(pattern.trim_start_matches("./"))
        .literal_separator(true)
        .build()
        .map_err(|e| ToolError(format!("invalid glob: {e}")))?
        .compile_matcher();
    let mut found: Found = Vec::new();
    let mut total = 0;
    for entry in super::walk(root).flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let relative = entry.path().strip_prefix(root).unwrap_or(entry.path());
        if !glob.is_match(relative) {
            continue;
        }
        total += 1;
        let modified = entry.metadata().ok().and_then(|m| m.modified().ok()).unwrap_or(SystemTime::UNIX_EPOCH);
        let at = found.partition_point(|(_, kept)| *kept >= modified);
        if at < MAX_RESULTS {
            found.insert(at, (entry.into_path(), modified));
            found.truncate(MAX_RESULTS);
        }
    }
    Ok((found, total))
}

/// Paths the walk looks at before settling for what it has: a mention search must answer while typing.
const MAX_SCANNED: usize = 50_000;

/// Workspace paths (directories end in `/`) matching `query` for an @ mention, best first: a name
/// that starts with it, a name that contains it, a path that contains it, then its letters in order.
pub fn search_names(root: &Path, query: &str, limit: usize) -> Vec<String> {
    let query = query.trim().to_lowercase().replace('\\', "/");
    let mut ranked: Vec<(u8, String)> = super::walk(root)
        .flatten()
        .take(MAX_SCANNED)
        .filter(|entry| entry.depth() > 0)
        .filter_map(|entry| {
            let relative = entry.path().strip_prefix(root).ok()?.to_string_lossy().replace('\\', "/");
            let shown = if entry.file_type().is_some_and(|t| t.is_dir()) { format!("{relative}/") } else { relative };
            Some((rank(&shown, &query)?, shown))
        })
        .collect();
    ranked.sort_by(|a, b| (a.0, a.1.len(), &a.1).cmp(&(b.0, b.1.len(), &b.1)));
    ranked.into_iter().take(limit).map(|(_, path)| path).collect()
}

fn rank(path: &str, query: &str) -> Option<u8> {
    let lower = path.to_lowercase();
    let name = lower.trim_end_matches('/').rsplit('/').next().unwrap_or(&lower);
    if name.starts_with(query) {
        return Some(0);
    }
    if name.contains(query) {
        return Some(1);
    }
    if lower.contains(query) {
        return Some(2);
    }
    let mut letters = lower.chars();
    query.chars().all(|wanted| letters.any(|c| c == wanted)).then_some(3)
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

    #[test]
    fn mention_search_ranks_names_before_paths_and_skips_ignored_and_git() {
        let sandbox = Sandbox::new("mention-search");
        sandbox.file("src/composer.tsx", "");
        sandbox.file("src/ui/composer-mentions.ts", "");
        sandbox.file("docs/compose.md", "");
        sandbox.file("tests/composer.test.ts", "");
        sandbox.file("dist/composer.js", "");
        sandbox.file(".gitignore", "dist/\n");
        sandbox.file(".git/composer", "");
        let found = search_names(&sandbox.ctx.workspace, "composer", 10);
        assert_eq!(found[..3], ["src/composer.tsx", "tests/composer.test.ts", "src/ui/composer-mentions.ts"], "{found:?}");
        assert!(!found.iter().any(|p| p.starts_with("dist/") || p.starts_with(".git/")), "{found:?}");
        assert_eq!(search_names(&sandbox.ctx.workspace, "src", 1), ["src/"], "directories are offered too");
        assert!(search_names(&sandbox.ctx.workspace, "scmp", 10).contains(&"src/composer.tsx".to_string()), "letters in order still match");
        assert!(search_names(&sandbox.ctx.workspace, "zzz", 10).is_empty());
    }

    #[test]
    fn past_the_limit_it_keeps_the_newest_of_the_whole_walk() {
        let sandbox = Sandbox::new("glob-newest");
        let old = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        for i in 0..MAX_RESULTS + 20 {
            let path = sandbox.file(&format!("f{i:03}.txt"), "");
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(old).unwrap();
        }
        let newest = sandbox.file("zzz/newest.txt", "");
        std::fs::File::options().write(true).open(&newest).unwrap().set_modified(old + std::time::Duration::from_secs(60)).unwrap();
        let (found, total) = find(&sandbox.ctx.workspace, "**/*.txt").unwrap();
        assert_eq!((found.len(), total), (MAX_RESULTS, MAX_RESULTS + 21));
        assert_eq!(found[0].0, newest, "the newest file wherever the walk meets it");
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
