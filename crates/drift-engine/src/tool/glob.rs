use std::path::Path;
use std::time::SystemTime;

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::ToolMetadata;
use super::{Ask, Context, FileGlob, Output, RunFuture, Tool, ToolError, display, required_str};
use crate::llm::ToolSpec;

const MAX_RESULTS: usize = 100;
/// How long a walk goes on before it answers with what it has.
const BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

pub struct Glob;

impl Tool for Glob {
    fn permissions(&self) -> &'static [&'static str] {
        &["glob", "read"]
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "glob".into(),
            description: include_str!("prompts/glob.txt").trim().into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob such as `src/**/*.ts` or `**/Cargo.toml`, relative to `path`." },
                    "path": { "type": "string", "description": "Directory to search. Default: the working directory." },
                    "include_ignored": { "type": "boolean", "description": "Also list what .gitignore and the built-in skip list leave out (node_modules, target, dist, build and the like). Default false." }
                },
                "required": ["pattern"]
            }),
        }
    }

    fn ask(&self, ctx: &Context, input: &Value) -> Option<Ask> {
        ctx.ask_if_outside("read", &ctx.resolve(input["path"].as_str().unwrap_or(".")), "Search")
    }

    fn asks(&self, ctx: &Context, input: &Value) -> Vec<Ask> {
        self.ask(ctx, input)
            .into_iter()
            .chain(
                input["pattern"]
                    .as_str()
                    .map(|pattern| Ask::new("glob", pattern, format!("Find {pattern}")).allow_by_default()),
            )
            .collect()
    }

    fn starts_early(&self, _input: &Value) -> bool {
        true
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let pattern = required_str(&input, "pattern")?.to_string();
            let root = ctx.resolve(input["path"].as_str().unwrap_or("."));
            let (base, stop) = (ctx.workdir().to_path_buf(), ctx.abort.clone());
            let everything = input["include_ignored"].as_bool().unwrap_or(false);
            let walked = tokio::task::spawn_blocking(move || find(&root, &pattern, everything, &stop))
                .await
                .map_err(|error| ToolError(error.to_string()))??;
            let (found, total) = (&walked.found, walked.total);

            let truncated = total > found.len();
            let mut lines: Vec<String> = found.iter().map(|(path, _)| display(path, &base)).collect();
            if lines.is_empty() {
                lines.push("No files matched".into());
            }
            lines.extend(walked.notes());
            let output = lines.join("\n");

            Ok(Output {
                title: input["pattern"].as_str().unwrap_or_default().into(),
                output,
                metadata: ToolMetadata {
                    count: Some(found.len()),
                    total: Some(total),
                    truncated: Some(truncated),
                    ..Default::default()
                },
            })
        })
    }
}

type Found = Vec<(std::path::PathBuf, SystemTime)>;

/// The newest matches, how many matched in all, and what the walk left out.
struct Walked {
    found: Found,
    total: usize,
    out_of_time: bool,
    skipped: Vec<String>,
}

impl Walked {
    fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if self.total > self.found.len() {
            notes.push(format!(
                "(the {MAX_RESULTS} newest of {} matches; narrow the pattern to see the rest)",
                self.total
            ));
        }
        if self.out_of_time {
            notes.push(format!(
                "(the walk stopped after {} s, so results may be incomplete; narrow `path`)",
                BUDGET.as_secs()
            ));
        }
        if !self.skipped.is_empty() {
            notes.push(format!(
                "(not searched: {}; name one in `path`, or pass `include_ignored: true`)",
                self.skipped.join(", ")
            ));
        }

        notes
    }
}

/// The newest [`MAX_RESULTS`] matches of the whole walk, newest first, and how many matched in all. The walk runs on
/// several threads, as grep's does, within [`BUDGET`]; a Stop ends it.
fn find(root: &Path, pattern: &str, everything: bool, stop: &CancellationToken) -> Result<Walked, ToolError> {
    use ignore::WalkState;
    use std::sync::atomic::{AtomicBool, Ordering};

    let glob = FileGlob::new(root, pattern).map_err(|error| ToolError(format!("invalid glob: {error}")))?;
    let matched = std::sync::Mutex::new(Vec::new());
    let skipped = std::sync::Arc::new(super::Skipped::default());
    let deadline = std::time::Instant::now() + BUDGET;
    let out_of_time = AtomicBool::new(false);

    super::search_walker(root, everything, &skipped)
        .build_parallel()
        .run(|| {
            let (glob, matched, out_of_time) = (&glob, &matched, &out_of_time);
            Box::new(move |entry| {
                if std::time::Instant::now() >= deadline {
                    out_of_time.store(true, Ordering::Relaxed);
                }
                if stop.is_cancelled() || out_of_time.load(Ordering::Relaxed) {
                    return WalkState::Quit;
                }
                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                if !entry.file_type().is_some_and(|kind| kind.is_file()) || !glob.matches(entry.path()) {
                    return WalkState::Continue;
                }

                let modified = entry
                    .metadata()
                    .ok()
                    .and_then(|meta| meta.modified().ok())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                matched.lock().unwrap().push((entry.into_path(), modified));
                WalkState::Continue
            })
        });
    if stop.is_cancelled() {
        return Err(ToolError("stopped".into()));
    }

    let mut found: Found = matched.into_inner().unwrap();
    let total = found.len();
    // Threads meet files in no fixed order, so equal times fall back to the path.
    found.sort_by(|(path, modified), (other, other_modified)| {
        other_modified.cmp(modified).then_with(|| path.cmp(other))
    });
    found.truncate(MAX_RESULTS);

    Ok(Walked {
        found,
        total,
        out_of_time: out_of_time.into_inner(),
        skipped: skipped.names(),
    })
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
            let relative = entry
                .path()
                .strip_prefix(root)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            let shown = if entry.file_type().is_some_and(|kind| kind.is_dir()) {
                format!("{relative}/")
            } else {
                relative
            };
            Some((rank(&shown, &query)?, shown))
        })
        .collect();

    ranked.sort_by(|(score, path), (other_score, other)| {
        (score, path.len(), path).cmp(&(other_score, other.len(), other))
    });
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
    query
        .chars()
        .all(|wanted| letters.any(|letter| letter == wanted))
        .then_some(3)
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
    async fn patterns_read_as_ripgreps_glob_reads_them_in_both_tools() {
        let sandbox = Sandbox::new("glob-rg");
        sandbox.file("top.ts", "needle");
        sandbox.file("src/a.ts", "needle");
        sandbox.file("src/deep/b.ts", "needle");
        sandbox.file("src/c.rs", "needle");
        let listed = |pattern: &'static str| {
            let ctx = &sandbox.ctx;
            async move {
                let mut lines: Vec<String> = Glob
                    .run(ctx, json!({ "pattern": pattern }))
                    .await
                    .unwrap()
                    .output
                    .lines()
                    .map(String::from)
                    .collect();
                lines.sort();
                lines
            }
        };
        let searched = |include: &'static str| {
            let ctx = &sandbox.ctx;
            async move {
                let out = super::super::grep::Grep
                    .run(ctx, json!({ "pattern": "needle", "include": include }))
                    .await
                    .unwrap()
                    .output;
                let mut files: Vec<String> = out
                    .lines()
                    .filter_map(|line| line.split_once(':').map(|(file, _)| file.to_string()))
                    .collect();
                files.sort();
                files
            }
        };
        assert_eq!(
            listed("*.ts").await,
            ["src/a.ts", "src/deep/b.ts", "top.ts"],
            "a name pattern matches at any depth"
        );
        assert_eq!(
            listed("src/*.ts").await,
            ["src/a.ts"],
            "a pattern with a slash is anchored and * stays in one folder"
        );
        assert_eq!(listed("src/**/*.ts").await, ["src/a.ts", "src/deep/b.ts"]);
        for pattern in ["*.ts", "src/*.ts", "src/**/*.ts"] {
            assert_eq!(
                searched(pattern).await,
                listed(pattern).await,
                "grep's include agrees with glob for {pattern}"
            );
        }
    }

    #[test]
    fn a_stop_ends_the_walk() {
        let sandbox = Sandbox::new("glob-stop");
        sandbox.file("a.txt", "");
        let stop = CancellationToken::new();
        stop.cancel();
        assert!(find(&sandbox.ctx.workspace, "*.txt", false, &stop).is_err());
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
        assert_eq!(
            found[..3],
            [
                "src/composer.tsx",
                "tests/composer.test.ts",
                "src/ui/composer-mentions.ts"
            ],
            "{found:?}"
        );
        assert!(
            !found
                .iter()
                .any(|path| path.starts_with("dist/") || path.starts_with(".git/")),
            "{found:?}"
        );
        assert_eq!(
            search_names(&sandbox.ctx.workspace, "src", 1),
            ["src/"],
            "directories are offered too"
        );
        assert!(
            search_names(&sandbox.ctx.workspace, "scmp", 10).contains(&"src/composer.tsx".to_string()),
            "letters in order still match"
        );
        assert!(search_names(&sandbox.ctx.workspace, "zzz", 10).is_empty());
    }

    #[test]
    fn past_the_limit_it_keeps_the_newest_of_the_whole_walk() {
        let sandbox = Sandbox::new("glob-newest");
        let old = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        for i in 0..MAX_RESULTS + 20 {
            let path = sandbox.file(&format!("f{i:03}.txt"), "");
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        let newest = sandbox.file("zzz/newest.txt", "");
        std::fs::File::options()
            .write(true)
            .open(&newest)
            .unwrap()
            .set_modified(old + std::time::Duration::from_secs(60))
            .unwrap();
        let walked = find(&sandbox.ctx.workspace, "**/*.txt", false, &CancellationToken::new()).unwrap();
        assert_eq!((walked.found.len(), walked.total), (MAX_RESULTS, MAX_RESULTS + 21));
        assert_eq!(walked.found[0].0, newest, "the newest file wherever the walk meets it");
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
        assert_eq!(
            lines,
            [".env", ".github/workflows/ci.yml"],
            "names are listed; .git is not"
        );
    }
}
