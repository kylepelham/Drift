use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use grep::regex::RegexMatcherBuilder;
use serde_json::{Value, json};

use super::ToolMetadata;
use super::{Ask, Context, FileGlob, Output, RunFuture, Tool, ToolError, display, required_str};
use crate::llm::ToolSpec;

mod search;

use search::{FileHits, Search, Tally};

/// Entries shown when the call does not say.
const DEFAULT_LIMIT: usize = 250;
/// Entries a search collects at most, however many it is asked to show.
const MAX_COLLECTED: usize = 20_000;
const MAX_LINE_CHARS: usize = 300;
/// How long a search walks before it answers with what it has.
const BUDGET: Duration = Duration::from_secs(20);
/// Files larger than this are skipped unless the search names one.
const LARGE_FILE_BYTES: u64 = 10 * 1024 * 1024;

pub struct Grep;

impl Tool for Grep {
    fn permissions(&self) -> &'static [&'static str] {
        &["grep", "read"]
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "grep".into(),
            description: include_str!("prompts/grep.txt").trim().into(),
            input_schema: schema(),
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

    fn starts_early(&self, _input: &Value) -> bool {
        true
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let request = Request {
                pattern: required_str(&input, "pattern")?.to_string(),
                options: Options::from(&input)?,
                root: ctx.resolve(input["path"].as_str().unwrap_or(".")),
                include: input["include"].as_str().map(str::to_string),
            };
            let (base, stop) = (ctx.workdir().to_path_buf(), ctx.abort.clone());
            let allowed = read_filter(ctx);

            let (found, request) = tokio::task::spawn_blocking(move || {
                let allowed = allowed
                    .as_ref()
                    .map(|allowed| allowed as &(dyn Fn(&Path) -> bool + Sync));
                searched(&request, &stop, allowed).map(|found| (found, request))
            })
            .await
            .map_err(|error| ToolError(error.to_string()))??;

            Ok(found.output(&request.options, &base, &request.pattern))
        })
    }
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "pattern": { "type": "string", "description": "Regular expression (Rust/ripgrep syntax) to search for." },
            "path": { "type": "string", "description": "File or directory to search. Default: the working directory." },
            "include": { "type": "string", "description": "Only search files matching this glob, such as `*.rs` or `src/**/*.ts`." },
            "type": { "type": "string", "description": "Only search this file type, as ripgrep names them (rg --type): rust, js, ts, py, go, java, and so on." },
            "output_mode": { "type": "string", "enum": ["content", "files_with_matches", "count"], "description": "`content` shows matching lines (default), `files_with_matches` the files that match, `count` how many lines match in each file." },
            "-A": { "type": "integer", "description": "Lines to show after each match (rg -A). Content mode only." },
            "-B": { "type": "integer", "description": "Lines to show before each match (rg -B). Content mode only." },
            "-C": { "type": "integer", "description": "Alias for context." },
            "context": { "type": "integer", "description": "Lines to show before and after each match (rg -C). Content mode only." },
            "-n": { "type": "boolean", "description": "Show line numbers (rg -n). Content mode only. Default true." },
            "-i": { "type": "boolean", "description": "Case insensitive search (rg -i)." },
            "multiline": { "type": "boolean", "description": "`.` matches newlines and a pattern may span lines (rg -U --multiline-dotall). Default false." },
            "head_limit": { "type": "integer", "description": "Show at most this many lines or entries, like `| head -N`. Default 250; 0 shows all (use sparingly)." },
            "offset": { "type": "integer", "description": "Skip this many lines or entries first, like `| tail -n +N`. Default 0." },
            "include_ignored": { "type": "boolean", "description": "Also search what .gitignore and the built-in skip list leave out (node_modules, target, dist, build and the like). Default false." }
        },
        "required": ["pattern"]
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Content,
    Files,
    Count,
}

/// A call's options, as Claude Code's `Grep` names them.
struct Options {
    mode: Mode,
    before: usize,
    after: usize,
    line_numbers: bool,
    ignore_case: bool,
    multiline: bool,
    file_type: Option<String>,
    /// Entries to show; none shows all.
    limit: Option<usize>,
    offset: usize,
    include_ignored: bool,
}

impl Options {
    fn from(input: &Value) -> Result<Self, ToolError> {
        let mode = match input["output_mode"].as_str().unwrap_or("content") {
            "content" => Mode::Content,
            "files_with_matches" => Mode::Files,
            "count" => Mode::Count,
            other => return Err(ToolError(format!("unknown output_mode `{other}`"))),
        };
        let number = |key: &str| input[key].as_u64().map(|value| value as usize);
        let context = number("-C").or_else(|| number("context")).unwrap_or(0);
        let limit = number("head_limit").unwrap_or(DEFAULT_LIMIT);
        let flag = |key: &str, default: bool| input[key].as_bool().unwrap_or(default);

        Ok(Self {
            mode,
            before: number("-B").unwrap_or(context),
            after: number("-A").unwrap_or(context),
            line_numbers: flag("-n", true),
            ignore_case: flag("-i", false),
            multiline: flag("multiline", false),
            file_type: input["type"]
                .as_str()
                .filter(|kind| !kind.is_empty())
                .map(str::to_string),
            limit: (limit > 0).then_some(limit),
            offset: number("offset").unwrap_or(0),
            include_ignored: flag("include_ignored", false),
        })
    }

    /// Entries worth collecting before stopping early: one past the page shows more exist. A count needs every file,
    /// and a later page needs the whole search in order.
    fn enough(&self) -> Option<usize> {
        let page = self.limit.filter(|_| self.mode != Mode::Count && self.offset == 0)?;

        Some(page + 1)
    }
}

/// Whether a file under the search may be read without asking, by the session's rules and approvals; none when no
/// rule could refuse a read, so the search checks nothing per file.
fn read_filter(ctx: &Context) -> Option<impl Fn(&Path) -> bool + Send + Sync + 'static> {
    let (engine, session, policy, read_root) = (
        ctx.engine.clone(),
        ctx.session_id.clone(),
        ctx.config.policy(),
        ctx.workspace.clone(),
    );
    let agent_policy = ctx.config.agent_policy(&ctx.agent);
    let rules = engine.permissions.compiled(&policy, &agent_policy);
    if !rules.restricts("read") {
        return None;
    }

    Some(move |path: &Path| {
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
    })
}

/// What a call asks for.
struct Request {
    pattern: String,
    options: Options,
    root: std::path::PathBuf,
    include: Option<String>,
}

/// Searches under the request's root within [`BUDGET`]; a Stop fails it.
fn searched(
    request: &Request,
    stop: &tokio_util::sync::CancellationToken,
    allowed: Option<&(dyn Fn(&Path) -> bool + Sync)>,
) -> Result<Found, ToolError> {
    let (root, options) = (request.root.as_path(), &request.options);
    let matcher = matcher(&request.pattern, options)?;
    let include = request.include.as_deref();
    let base = if root.is_file() {
        root.parent().unwrap_or(root)
    } else {
        root
    };
    let include = include
        .map(|glob| FileGlob::new(base, glob))
        .transpose()
        .map_err(|error| ToolError(format!("invalid include glob: {error}")))?;
    let skipped = Arc::new(super::Skipped::default());
    let mut walk = super::search_walker(root, options.include_ignored, &skipped);
    if let Some(kind) = &options.file_type {
        walk.types(file_type(kind)?);
    }

    let tally = Tally::default();
    let search = Search {
        root,
        matcher: &matcher,
        include: include.as_ref(),
        options,
        stop,
        allowed,
        deadline: Instant::now() + BUDGET,
        enough: Some(options.enough().unwrap_or(MAX_COLLECTED).min(MAX_COLLECTED)),
    };
    search.run(&walk, &tally);
    if stop.is_cancelled() {
        return Err(ToolError("stopped".into()));
    }

    Ok(Found::from(tally, skipped.names()))
}

fn matcher(pattern: &str, options: &Options) -> Result<grep::regex::RegexMatcher, ToolError> {
    let mut builder = RegexMatcherBuilder::new();
    builder.case_insensitive(options.ignore_case);
    if options.multiline {
        builder.multi_line(true).dot_matches_new_line(true);
    } else {
        builder.line_terminator(Some(b'\n'));
    }

    builder
        .build(pattern)
        .map_err(|error| ToolError(format!("invalid regex: {error}")))
}

fn file_type(kind: &str) -> Result<ignore::types::Types, ToolError> {
    let mut types = ignore::types::TypesBuilder::new();
    types.add_defaults();
    types.select(kind);

    types
        .build()
        .ok()
        .filter(|types| types.definitions().iter().any(|known| known.name() == kind))
        .ok_or_else(|| {
            ToolError(format!(
                "unknown file type `{kind}`; use ripgrep's names, such as rust, js, ts, py"
            ))
        })
}

/// What the search collected, sorted by path, and everything it left out.
struct Found {
    files: Vec<FileHits>,
    withheld: usize,
    restricted: usize,
    unreadable: usize,
    large: usize,
    out_of_time: bool,
    /// It stopped once it had enough to show (or hit [`MAX_COLLECTED`]), so more may exist.
    stopped_early: bool,
    skipped: Vec<String>,
}

impl Found {
    fn from(tally: Tally, skipped: Vec<String>) -> Self {
        let mut files = tally.hits.into_inner().unwrap();
        files.sort_by(|a, b| a.path.cmp(&b.path));

        Self {
            files,
            withheld: tally.withheld.into_inner(),
            restricted: tally.restricted.into_inner(),
            unreadable: tally.unreadable.into_inner(),
            large: tally.large.into_inner(),
            out_of_time: tally.out_of_time.into_inner(),
            stopped_early: tally.enough.into_inner(),
            skipped,
        }
    }

    /// Every entry in order, as text, each saying whether a `--` goes before it.
    fn entries(&self, options: &Options, base: &Path) -> Vec<(bool, String)> {
        let mut entries = Vec::new();
        let grouped = options.mode == Mode::Content && options.before + options.after > 0;
        for (index, file) in self.files.iter().enumerate() {
            let name = display(&file.path, base);
            match options.mode {
                Mode::Files => entries.push((false, name)),
                Mode::Count => entries.push((false, format!("{name}:{}", file.count))),
                Mode::Content => {
                    for (at, line) in file.lines.iter().enumerate() {
                        let apart = grouped && ((at == 0 && index > 0) || line.gap);
                        entries.push((apart, content_line(&name, line, options.line_numbers)));
                    }
                }
            }
        }

        entries
    }

    fn output(&self, options: &Options, base: &Path, title: &str) -> Output {
        let entries = self.entries(options, base);
        let total = entries.len();
        let page: Vec<&(bool, String)> = entries
            .iter()
            .skip(options.offset)
            .take(options.limit.unwrap_or(usize::MAX))
            .collect();
        let more = total > options.offset + page.len();
        let mut text = String::new();
        for (index, (apart, line)) in page.iter().enumerate() {
            if *apart && index > 0 {
                text.push_str("--\n");
            }
            text.push_str(line);
            text.push('\n');
        }
        let mut text = text.trim_end().to_string();
        if text.is_empty() {
            text = self.nothing(options.offset, total);
        }
        self.notes(&mut text, options, page.len(), total);

        Output {
            title: title.into(),
            output: text,
            metadata: ToolMetadata {
                count: Some(page.len()),
                total: Some(total),
                truncated: Some(more || self.stopped_early),
                capped: Some(self.stopped_early || self.out_of_time),
                withheld: Some(self.withheld),
                restricted: Some(self.restricted),
                ..Default::default()
            },
        }
    }

    fn nothing(&self, offset: usize, total: usize) -> String {
        if offset > 0 && total > 0 {
            return format!("Nothing past offset {offset}; there are {total} in all");
        }

        "No matches".into()
    }

    /// A note for each limit or skip that hid anything.
    fn notes(&self, text: &mut String, options: &Options, shown: usize, total: usize) {
        let mut note = |line: String| {
            text.push_str("\n(");
            text.push_str(&line);
            text.push(')');
        };
        if self.out_of_time {
            note(format!(
                "the search stopped after {} s, so results may be incomplete; narrow `path`",
                BUDGET.as_secs()
            ));
        }
        if self.stopped_early {
            note("more matches exist; the search stopped once it had enough to show, so these are the first it found, not the first by path. Narrow the pattern, `path`, `include` or `type`".into());
        } else if total > options.offset + shown {
            let next = options.offset + shown;
            note(format!(
                "{} to {next} of {total}; pass `offset: {next}` for more, or narrow the search",
                options.offset + 1
            ));
        }
        self.skip_notes(note);
    }

    fn skip_notes(&self, mut note: impl FnMut(String)) {
        if !self.skipped.is_empty() {
            note(format!(
                "not searched: {}; name one in `path`, or pass `include_ignored: true`",
                self.skipped.join(", ")
            ));
        }
        let counted = [
            (
                self.large,
                "files over 10 MB were not searched; name one in `path` to search it",
            ),
            (
                self.withheld,
                "files that may hold secrets were not searched; read one directly and the user is asked",
            ),
            (
                self.restricted,
                "files were excluded by read policy or need read approval; use read on an allowed file",
            ),
            (
                self.unreadable,
                "files or folders could not be read, so any matches in them are missing",
            ),
        ];
        for (count, why) in counted.into_iter().filter(|(count, _)| *count > 0) {
            note(format!("{count} {why}"));
        }
    }
}

fn content_line(name: &str, line: &search::Line, numbers: bool) -> String {
    let mark = if line.matched { ':' } else { '-' };
    if numbers {
        format!("{name}{mark}{}{mark} {}", line.number, line.text)
    } else {
        format!("{name}{mark} {}", line.text)
    }
}

fn clip(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        return line.to_string();
    }
    format!("{}...", line.chars().take(MAX_LINE_CHARS).collect::<String>())
}

#[cfg(test)]
mod tests;
