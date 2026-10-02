use std::path::Path;

use globset::GlobBuilder;
use grep::regex::RegexMatcherBuilder;
use grep::searcher::sinks::UTF8;
use grep::searcher::{BinaryDetection, SearcherBuilder};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::sensitive::is_sensitive;
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

    /// Searching one named file that may hold secrets asks, as reading it would.
    fn ask(&self, ctx: &Context, input: &Value) -> Option<Ask> {
        ctx.ask_to_read(&ctx.resolve(input["path"].as_str().unwrap_or(".")), "Search")
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let pattern = required_str(&input, "pattern")?.to_string();
            let root = ctx.resolve(input["path"].as_str().unwrap_or("."));
            let include = input["include"].as_str().map(str::to_string);
            let (workspace, stop) = (ctx.workspace.clone(), ctx.abort.clone());
            let found = tokio::task::spawn_blocking(move || search(&root, &pattern, include.as_deref(), &workspace, &stop))
                .await
                .map_err(|e| ToolError(e.to_string()))??;
            let mut output = if found.lines.is_empty() { "No matches".to_string() } else { found.lines.join("\n") };
            if found.total > found.lines.len() {
                output.push_str(&format!("\n({} matches; these are the first {MAX_MATCHES} by file and line. Narrow the pattern or path to see the rest)", found.total));
            }
            if found.withheld > 0 {
                output.push_str(&format!("\n({} files that may hold secrets were not searched; read one directly and the user is asked)", found.withheld));
            }
            let metadata = json!({ "count": found.lines.len(), "total": found.total, "truncated": found.total > found.lines.len(), "withheld": found.withheld });
            Ok(Output { title: input["pattern"].as_str().unwrap_or_default().into(), output, metadata })
        })
    }
}

struct Found {
    lines: Vec<String>,
    /// Every match, listed or not.
    total: usize,
    /// Files skipped because they may hold secrets.
    withheld: usize,
}

/// One matching line: the file as shown, its line number, and the line.
type Hit = (String, u64, String);

/// The first [`MAX_MATCHES`] hits by file then line, whatever order the threads find them in.
#[derive(Default)]
struct First(std::sync::Mutex<Vec<Hit>>);

impl First {
    fn add(&self, found: Vec<Hit>) {
        let mut hits = self.0.lock().unwrap();
        hits.extend(found);
        if hits.len() > 2 * MAX_MATCHES {
            hits.sort();
            hits.truncate(MAX_MATCHES);
        }
    }

    fn into_sorted(self) -> Vec<Hit> {
        let mut hits = self.0.into_inner().unwrap();
        hits.sort();
        hits.truncate(MAX_MATCHES);
        hits
    }
}

/// Searches files on several threads, as ripgrep does, and lists the first matches by file then
/// line, with how many there were in all. Binary files end their search at the first NUL; files
/// that may hold secrets are skipped unless the search names one directly, which has already asked.
/// A Stop ends the walk and every file search in it.
fn search(root: &Path, pattern: &str, include: Option<&str>, workspace: &Path, stop: &CancellationToken) -> Result<Found, ToolError> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let matcher = RegexMatcherBuilder::new()
        .line_terminator(Some(b'\n'))
        .build(pattern)
        .map_err(|e| ToolError(format!("invalid regex: {e}")))?;
    let include = include
        .map(|glob| GlobBuilder::new(glob).literal_separator(false).build().map(|g| g.compile_matcher()))
        .transpose()
        .map_err(|e| ToolError(format!("invalid include glob: {e}")))?;
    let first = First::default();
    let (total, withheld) = (AtomicUsize::new(0), AtomicUsize::new(0));
    super::walker(root).build_parallel().run(|| {
        let mut searcher = SearcherBuilder::new().line_number(true).binary_detection(BinaryDetection::quit(0)).build();
        let (matcher, include, first, total, withheld) = (&matcher, &include, &first, &total, &withheld);
        Box::new(move |entry| {
            use ignore::WalkState;
            if stop.is_cancelled() {
                return WalkState::Quit;
            }
            let Ok(entry) = entry else { return WalkState::Continue };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                return WalkState::Continue;
            }
            let relative = entry.path().strip_prefix(root).unwrap_or(entry.path());
            if include.as_ref().is_some_and(|glob| !glob.is_match(relative) && !glob.is_match(entry.file_name())) {
                return WalkState::Continue;
            }
            if entry.path() != root && is_sensitive(entry.path()) {
                withheld.fetch_add(1, Ordering::Relaxed);
                return WalkState::Continue;
            }
            let name = display(entry.path(), workspace);
            let mut found = Vec::new();
            let _ = searcher.search_path(
                matcher,
                entry.path(),
                UTF8(|line_number, line| {
                    total.fetch_add(1, Ordering::Relaxed);
                    // Later lines of this file sort after these, so past the limit only the count matters.
                    if found.len() < MAX_MATCHES {
                        found.push((name.clone(), line_number, clip(line.trim_end())));
                    }
                    Ok(!stop.is_cancelled())
                }),
            );
            first.add(found);
            WalkState::Continue
        })
    });
    if stop.is_cancelled() {
        return Err(ToolError("stopped".into()));
    }
    let lines = first.into_sorted().into_iter().map(|(name, line, text)| format!("{name}:{line}: {text}")).collect();
    Ok(Found { lines, total: total.into_inner(), withheld: withheld.into_inner() })
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
    async fn skips_secrets_git_internals_and_binaries() {
        let sandbox = Sandbox::new("grep-skip");
        sandbox.file("src/a.rs", "token = 1\n");
        sandbox.file(".env", "token = hunter2\n");
        sandbox.file(".env.example", "token = changeme\n");
        sandbox.file(".git/config", "token = internal\n");
        std::fs::write(sandbox.ctx.workspace.join("blob.bin"), b"token = 1\0\x01\x02").unwrap();
        let out = Grep.run(&sandbox.ctx, json!({ "pattern": "token" })).await.unwrap();
        let mut lines: Vec<&str> = out.output.lines().filter(|line| !line.starts_with('(')).collect();
        lines.sort();
        assert_eq!(lines, [".env.example:1: token = changeme", "src/a.rs:1: token = 1"]);
        assert!(out.output.contains("1 files that may hold secrets were not searched"), "{}", out.output);
        assert!(!out.output.contains("hunter2") && !out.output.contains("internal") && !out.output.contains("blob.bin"), "{}", out.output);
        assert_eq!(out.metadata["withheld"], 1);

        let named = json!({ "pattern": "token", "path": ".env" });
        assert!(Grep.ask(&sandbox.ctx, &named).is_some_and(|ask| ask.title.contains("may hold secrets")));
        let direct = Grep.run(&sandbox.ctx, named).await.unwrap();
        assert_eq!(direct.output, ".env:1: token = hunter2", "a secret named directly is searched once approved");
    }

    #[tokio::test]
    async fn many_files_searched_together_list_in_order_and_stop_past_the_limit() {
        let sandbox = Sandbox::new("grep-many");
        for i in 0..60 {
            sandbox.file(&format!("f{i:02}.txt"), "hit\nmiss\nhit\n");
        }
        let few = Grep.run(&sandbox.ctx, json!({ "pattern": "hit", "include": "f0*.txt" })).await.unwrap();
        let lines: Vec<&str> = few.output.lines().collect();
        assert_eq!(lines.len(), 20);
        assert!(lines.windows(2).all(|pair| pair[0] < pair[1]) && lines[0] == "f00.txt:1: hit", "by file then line: {lines:?}");
        let all = Grep.run(&sandbox.ctx, json!({ "pattern": "hit" })).await.unwrap();
        assert_eq!((all.metadata["count"].as_u64(), all.metadata["truncated"].as_bool()), (Some(120), Some(false)), "120 matches fit");
        for i in 60..110 {
            sandbox.file(&format!("g{i}.txt"), "hit\nhit\n");
        }
        let past = Grep.run(&sandbox.ctx, json!({ "pattern": "hit" })).await.unwrap();
        assert_eq!((past.metadata["count"].as_u64(), past.metadata["total"].as_u64(), past.metadata["truncated"].as_bool()), (Some(MAX_MATCHES as u64), Some(220), Some(true)));
        let lines: Vec<&str> = past.output.lines().collect();
        assert_eq!((lines[0], lines[MAX_MATCHES - 1]), ("f00.txt:1: hit", "g89.txt:2: hit"), "the first by file and line, not the first found");
        assert!(past.output.contains("220 matches; these are the first 200 by file and line"), "{}", past.output);
    }

    #[tokio::test]
    async fn a_stop_ends_the_search() {
        let sandbox = Sandbox::new("grep-stop");
        sandbox.file("a.txt", "hit\n");
        sandbox.ctx.abort.cancel();
        let err = Grep.run(&sandbox.ctx, json!({ "pattern": "hit" })).await.unwrap_err();
        assert_eq!(err.0, "stopped");
    }

    #[tokio::test]
    async fn bad_regex_is_reported() {
        let sandbox = Sandbox::new("grep-bad");
        let err = Grep.run(&sandbox.ctx, json!({ "pattern": "(" })).await.unwrap_err();
        assert!(err.0.starts_with("invalid regex"));
    }
}
