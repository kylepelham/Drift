use std::fmt::Write;
use std::path::Path;

use grep::regex::RegexMatcherBuilder;
use grep::searcher::sinks::UTF8;
use grep::searcher::{BinaryDetection, SearcherBuilder};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::ToolMetadata;
use super::sensitive::is_sensitive;
use super::{Ask, Context, FileGlob, Output, RunFuture, Tool, ToolError, display, required_str};
use crate::llm::ToolSpec;

const MAX_MATCHES: usize = 200;
/// Past this many matches the search stops: a broad pattern in a large tree returns at once.
const MAX_COUNTED: usize = 2000;
const MAX_LINE_CHARS: usize = 300;

pub struct Grep;

impl Tool for Grep {
    fn permissions(&self) -> &'static [&'static str] {
        &["grep", "read"]
    }

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

    fn asks(&self, ctx: &Context, input: &Value) -> Vec<Ask> {
        self.ask(ctx, input)
            .into_iter()
            .chain(
                input["pattern"]
                    .as_str()
                    .map(|pattern| Ask::new("grep", pattern, format!("Search for {pattern}")).allow_by_default()),
            )
            .collect()
    }

    fn starts_early(&self) -> bool {
        true
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let pattern = required_str(&input, "pattern")?.to_string();
            let root = ctx.resolve(input["path"].as_str().unwrap_or("."));
            let include = input["include"].as_str().map(str::to_string);
            let (workspace, stop) = (ctx.workspace.clone(), ctx.abort.clone());
            let allowed = read_filter(ctx);

            let found = tokio::task::spawn_blocking(move || {
                search(Search {
                    root: &root,
                    pattern: &pattern,
                    include: include.as_deref(),
                    workspace: &workspace,
                    stop: &stop,
                    allowed: &allowed,
                })
            })
            .await
            .map_err(|error| ToolError(error.to_string()))??;

            let metadata = ToolMetadata {
                count: Some(found.lines.len()),
                total: Some(found.total.min(MAX_COUNTED)),
                capped: Some(found.total > MAX_COUNTED),
                truncated: Some(found.total > found.lines.len()),
                withheld: Some(found.withheld),
                restricted: Some(found.restricted),
                ..Default::default()
            };
            Ok(Output {
                title: input["pattern"].as_str().unwrap_or_default().into(),
                output: found.report(),
                metadata,
            })
        })
    }
}

/// Whether a file under the search may be read without asking, by the session's rules and approvals.
fn read_filter(ctx: &Context) -> impl Fn(&Path) -> bool + Send + Sync + 'static {
    let (engine, session, policy, read_root) = (
        ctx.engine.clone(),
        ctx.session_id.clone(),
        ctx.config.policy(),
        ctx.workspace.clone(),
    );
    let agent_policy = ctx.config.agent_policy(&ctx.agent);
    let rules = engine.permissions.compiled(&policy, &agent_policy);

    move |path: &Path| {
        super::read_ask(&read_root, path, "Search").is_none_or(|ask| {
            engine.permissions.covered_by_approval(
                &session,
                &rules,
                crate::permission::Policies {
                    workspace: &policy,
                    agent: &agent_policy,
                },
                &ask,
            )
        })
    }
}

struct Found {
    lines: Vec<String>,
    /// Every match, listed or not; past [`MAX_COUNTED`] the search stopped.
    total: usize,
    /// Files skipped because they may hold secrets.
    withheld: usize,
    restricted: usize,
}

impl Found {
    /// The listed matches, then a note for each limit or skip that hid any.
    fn report(&self) -> String {
        let mut output = if self.lines.is_empty() {
            "No matches".to_string()
        } else {
            self.lines.join("\n")
        };

        if self.total > MAX_COUNTED {
            write!(output, "\n(more than {MAX_COUNTED} matches, so the search stopped; these {MAX_MATCHES} are sorted from the files it reached and earlier files may be missing. Narrow the pattern, `path` or `include`)").expect("writing to a String cannot fail");
        } else if self.total > self.lines.len() {
            write!(output, "\n({} matches; these are the first {MAX_MATCHES} by file and line. Narrow the pattern, `path` or `include` to see the rest)", self.total).expect("writing to a String cannot fail");
        }
        if self.withheld > 0 {
            write!(
                output,
                "\n({} files that may hold secrets were not searched; read one directly and the user is asked)",
                self.withheld
            )
            .expect("writing to a String cannot fail");
        }
        if self.restricted > 0 {
            write!(
                output,
                "\n({} files were excluded by read policy or need read approval; use read on an allowed file)",
                self.restricted
            )
            .expect("writing to a String cannot fail");
        }

        output
    }
}

/// One matching line: the file as shown, its line number, and the line.
type Hit = (String, u64, String);

struct Search<'a> {
    root: &'a Path,
    pattern: &'a str,
    include: Option<&'a str>,
    workspace: &'a Path,
    stop: &'a CancellationToken,
    allowed: &'a (dyn Fn(&Path) -> bool + Sync),
}

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
/// line, with how many there were in all, up to [`MAX_COUNTED`], where it stops. Binary files end
/// their search at the first NUL; files that may hold secrets are skipped unless the search names
/// one directly, which has already asked. A Stop ends the walk and every file search in it.
fn search(search: Search<'_>) -> Result<Found, ToolError> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (root, pattern, include) = (search.root, search.pattern, search.include);
    let (workspace, stop, allowed) = (search.workspace, search.stop, search.allowed);

    let matcher = RegexMatcherBuilder::new()
        .line_terminator(Some(b'\n'))
        .build(pattern)
        .map_err(|error| ToolError(format!("invalid regex: {error}")))?;
    let base = if root.is_file() {
        root.parent().unwrap_or(root)
    } else {
        root
    };
    let include = include
        .map(|glob| FileGlob::new(base, glob))
        .transpose()
        .map_err(|error| ToolError(format!("invalid include glob: {error}")))?;

    let first = First::default();
    let (total, withheld) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let restricted = AtomicUsize::new(0);
    super::walker(root).build_parallel().run(|| {
        let mut searcher = SearcherBuilder::new()
            .line_number(true)
            .binary_detection(BinaryDetection::quit(0))
            .build();
        let (matcher, include, first, total, withheld, restricted) =
            (&matcher, &include, &first, &total, &withheld, &restricted);
        Box::new(move |entry| {
            use ignore::WalkState;

            // Skip what is not a file, outside the include glob, secret or not readable here.
            if stop.is_cancelled() || total.load(Ordering::Relaxed) > MAX_COUNTED {
                return WalkState::Quit;
            }
            let Ok(entry) = entry else { return WalkState::Continue };
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                return WalkState::Continue;
            }
            if include.as_ref().is_some_and(|glob| !glob.matches(entry.path())) {
                return WalkState::Continue;
            }
            if entry.path() != root && is_sensitive(entry.path()) {
                withheld.fetch_add(1, Ordering::Relaxed);
                return WalkState::Continue;
            }
            if entry.path() != root && !allowed(entry.path()) {
                restricted.fetch_add(1, Ordering::Relaxed);
                return WalkState::Continue;
            }

            let name = display(entry.path(), workspace);
            let mut found = Vec::new();
            let _ = searcher.search_path(
                matcher,
                entry.path(),
                UTF8(|line_number, line| {
                    let counted = total.fetch_add(1, Ordering::Relaxed) + 1;
                    // Later lines of this file sort after these, so past the limit only the count matters.
                    if found.len() < MAX_MATCHES {
                        found.push((name.clone(), line_number, clip(line.trim_end())));
                    }
                    Ok(counted <= MAX_COUNTED && !stop.is_cancelled())
                }),
            );
            first.add(found);
            WalkState::Continue
        })
    });
    if stop.is_cancelled() {
        return Err(ToolError("stopped".into()));
    }

    let lines = first
        .into_sorted()
        .into_iter()
        .map(|(name, line, text)| format!("{name}:{line}: {text}"))
        .collect();
    Ok(Found {
        lines,
        total: total.into_inner(),
        withheld: withheld.into_inner(),
        restricted: restricted.into_inner(),
    })
}

fn clip(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        return line.to_string();
    }
    format!("{}...", line.chars().take(MAX_LINE_CHARS).collect::<String>())
}

#[cfg(test)]
mod tests;
