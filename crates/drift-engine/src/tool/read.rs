use serde_json::{json, Value};

use super::{display, required_str, Ask, Context, Output, RunFuture, Tool, ToolError};
use crate::llm::ToolSpec;

const MAX_LINES: usize = 2000;
const MAX_LINE_CHARS: usize = 2000;
/// Past this a file is not loaded whole: its page is read line by line and the rest left on disk.
const WHOLE_BYTES: u64 = 10 * 1024 * 1024;
/// One page stays under the shared result bound, leaving room for the continuation note.
const PAGE_BYTES: usize = super::spool::MAX_RESULT_BYTES - 1024;
/// Subdirectory instructions take at most this much of a read's result; the page has the rest.
const REMINDER_BYTES: usize = PAGE_BYTES / 2;
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

    fn starts_early(&self) -> bool {
        true
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
            if meta.len() > WHOLE_BYTES {
                return read_large(ctx, &path, offset, limit).await;
            }
            let bytes = tokio::fs::read(&path).await?;
            if let Some(mime) = super::image::sniff(&bytes).or(super::image::is_pdf(&bytes).then_some(super::image::PDF)) {
                ctx.files.mark_read(&path);
                return attached(ctx, &path, mime, &bytes);
            }
            if bytes.iter().take(8000).any(|b| *b == 0) {
                return Err(ToolError(format!("{} is binary", display(&path, &ctx.workspace))));
            }
            let text = String::from_utf8_lossy(&bytes);
            let total = text.lines().count();
            // The reminders come first in the budget, so the page fits beside them within one result.
            let reminders = reminders(ctx, &path);
            let body = page(&text, offset, limit, PAGE_BYTES.saturating_sub(reminders.len()));
            let shown = body.len();
            let mut output = body.join("\n");
            if offset - 1 + shown < total {
                output.push_str(&format!("\n\n({} more lines; read with offset {})", total - (offset - 1 + shown), offset + shown));
            }
            output.push_str(&reminders);
            ctx.files.mark_read(&path);
            Ok(Output {
                title: display(&path, &ctx.workspace),
                output,
                metadata: json!({ "lines": total, "shown": shown }),
            })
        })
    }
}

/// Reads a bounded page from a large text file, without scanning the rest for a line count.
async fn read_large(ctx: &Context, path: &std::path::Path, offset: usize, limit: usize) -> Result<Output, ToolError> {
    let name = display(path, &ctx.workspace);
    let reminders = reminders(ctx, path);
    let budget = PAGE_BYTES.saturating_sub(reminders.len());
    let (file, stop) = (path.to_path_buf(), ctx.abort.clone());
    let read = tokio::task::spawn_blocking(move || large_page(&file, offset, limit, budget, &stop)).await.map_err(|e| ToolError(e.to_string()))??;
    if ctx.abort.is_cancelled() {
        return Err(ToolError("stopped".into()));
    }
    let Large { lines, more, binary } = read;
    if binary {
        return Err(ToolError(format!("{name} is binary")));
    }
    let shown = lines.len();
    if shown == 0 {
        return Err(ToolError(format!("{name} has fewer than {offset} lines")));
    }
    let mut output = lines.join("\n");
    if more {
        output.push_str(&format!("\n\n(more lines follow; read with offset {})", offset + shown));
    }
    output.push_str(&reminders);
    ctx.files.mark_read(path);
    Ok(Output { title: name, output, metadata: json!({ "lines": null, "shown": shown, "large": true }) })
}

struct Large {
    lines: Vec<String>,
    more: bool,
    binary: bool,
}

/// The most of one line kept in memory: enough for [`MAX_LINE_CHARS`] characters of any width.
const LINE_BYTES: usize = MAX_LINE_CHARS * 4 + 4;

/// Lines `offset..` of `path`, numbered as [`page`] does, read a buffer at a time: no line is held past [`LINE_BYTES`], and a Stop is seen between buffers.
fn large_page(path: &std::path::Path, offset: usize, limit: usize, budget: usize, stop: &tokio_util::sync::CancellationToken) -> std::io::Result<Large> {
    use std::io::BufRead;
    let mut reader = std::io::BufReader::with_capacity(1 << 16, std::fs::File::open(path)?);
    if reader.fill_buf()?.iter().take(8000).any(|b| *b == 0) {
        return Ok(Large { lines: Vec::new(), more: false, binary: true });
    }
    let mut page = Page { lines: Vec::new(), used: 0, limit, budget };
    let (mut number, mut line, mut started) = (1, Vec::new(), false);
    while !stop.is_cancelled() {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            let more = started && number >= offset && !page.push(number, &line);
            return Ok(Large { lines: page.lines, more, binary: false });
        }
        let (take, ended) = chunk.iter().position(|b| *b == b'\n').map_or((chunk.len(), false), |at| (at + 1, true));
        if number >= offset {
            let room = LINE_BYTES.saturating_sub(line.len());
            line.extend_from_slice(&chunk[..take.min(room)]);
        }
        reader.consume(take);
        started = !ended;
        if !ended {
            continue;
        }
        if number >= offset && !page.push(number, &line) {
            return Ok(Large { lines: page.lines, more: true, binary: false });
        }
        line.clear();
        number += 1;
    }
    Ok(Large { lines: page.lines, more: false, binary: false })
}

/// A page being filled: numbered lines within `limit` and `budget` bytes, at least one.
struct Page {
    lines: Vec<String>,
    used: usize,
    limit: usize,
    budget: usize,
}

impl Page {
    /// Adds the line, or says `false` when the page is full; it never takes the line then.
    fn push(&mut self, number: usize, raw: &[u8]) -> bool {
        let text = String::from_utf8_lossy(raw);
        let numbered = format!("{number}: {}", truncate(text.trim_end_matches(['\n', '\r'])));
        self.used += numbered.len() + 1;
        if self.lines.len() == self.limit || (self.used > self.budget && !self.lines.is_empty()) {
            return false;
        }
        self.lines.push(numbered);
        true
    }
}
/// An image or PDF comes back for the model to look at, not as text.
fn attached(ctx: &Context, path: &std::path::Path, mime: &str, bytes: &[u8]) -> Result<Output, ToolError> {
    let name = display(path, &ctx.workspace);
    let (kind, limit) = if mime == super::image::PDF { ("a PDF", super::image::MAX_PDF_BYTES) } else { ("an image", super::image::MAX_IMAGE_BYTES) };
    if bytes.len() > limit {
        return Err(ToolError(format!("{name} is {kind} of {} bytes; too large to look at (the limit is {} MB)", bytes.len(), limit / 1024 / 1024)));
    }
    let file = super::image::Image::from_bytes(mime, bytes);
    Ok(Output {
        title: name.clone(),
        output: format!("{name} is {kind} ({mime}, {} KB); it follows this result.", bytes.len().div_ceil(1024)),
        metadata: json!({ "images": super::image::metadata(&[file]) }),
    })
}

/// Subdirectory instruction files not yet shown this session, within half a result; one that does
/// not fit is named so the model can read it.
fn reminders(ctx: &Context, path: &std::path::Path) -> String {
    let mut out = String::new();
    for (file, text) in crate::config::nested_instructions(&ctx.workspace, path) {
        if file == path || !ctx.files.first_showing(&file) {
            continue;
        }
        let name = display(&file, &ctx.workspace);
        let reminder = format!("\n\n<system-reminder>\nInstructions from {name}, for files under it:\n{text}\n</system-reminder>");
        if out.len() + reminder.len() <= REMINDER_BYTES {
            out.push_str(&reminder);
        } else {
            out.push_str(&format!("\n\n<system-reminder>\n{name} holds instructions for files under it; read it before working there.\n</system-reminder>"));
        }
    }
    out
}

/// Numbered lines from `offset`, at most `limit` of them and within `budget` bytes; always at least
/// one line, so every read makes progress.
fn page(text: &str, offset: usize, limit: usize, budget: usize) -> Vec<String> {
    let mut used = 0;
    let mut lines = Vec::new();
    for (index, line) in text.lines().enumerate().skip(offset - 1).take(limit) {
        let numbered = format!("{}: {}", index + 1, truncate(line));
        used += numbered.len() + 1;
        if used > budget && !lines.is_empty() {
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
        sandbox.ctx.files.forget_shown();
        assert!(Read.run(&sandbox.ctx, json!({ "path": "pkg/web/b.ts" })).await.unwrap().output.contains("web rules"), "shown again once forgotten");
    }

    #[tokio::test]
    async fn a_full_page_and_its_reminder_fit_in_one_result() {
        let sandbox = Sandbox::new("read-nested-full");
        sandbox.file("pkg/AGENTS.md", &"rule line\n".repeat(1_000));
        sandbox.file("pkg/big.txt", &format!("{}\n", "w".repeat(1_500)).repeat(1_000));
        let out = Read.run(&sandbox.ctx, json!({ "path": "pkg/big.txt" })).await.unwrap();
        assert!(out.output.len() <= super::super::spool::MAX_RESULT_BYTES, "{}", out.output.len());
        assert!(out.output.ends_with("</system-reminder>"), "the reminder is whole, not cut in the middle");
        assert!(out.output.contains("read with offset"), "the page says where to go on");
    }

    #[tokio::test]
    async fn a_file_too_large_to_load_is_read_a_page_at_a_time() {
        let sandbox = Sandbox::new("read-large");
        let line = format!("{}\n", "log entry ".repeat(10));
        let lines = (WHOLE_BYTES as usize / line.len()) + 5_000;
        sandbox.file("big.log", &line.repeat(lines));
        let first = Read.run(&sandbox.ctx, json!({ "path": "big.log", "limit": 3 })).await.unwrap();
        assert!(first.output.starts_with("1: log entry") && first.output.ends_with("(more lines follow; read with offset 4)"), "{}", &first.output);
        let deep = Read.run(&sandbox.ctx, json!({ "path": "big.log", "offset": lines, "limit": 10 })).await.unwrap();
        assert!(deep.output.starts_with(&format!("{lines}: log entry")) && !deep.output.contains("more lines"), "the last line, with nothing after it");
        assert!(Read.run(&sandbox.ctx, json!({ "path": "big.log", "offset": lines + 1 })).await.unwrap_err().0.contains("fewer than"));
        assert!(sandbox.ctx.files.was_read(&sandbox.ctx.workspace.join("big.log")));

        sandbox.file("one-line.log", &"x".repeat(WHOLE_BYTES as usize + 1024));
        let long = Read.run(&sandbox.ctx, json!({ "path": "one-line.log" })).await.unwrap();
        assert!(long.output.starts_with("1: xxx") && long.output.len() < 3 * MAX_LINE_CHARS && !long.output.contains("more lines"), "one huge line, cut as it is read");
        sandbox.file("tail.log", &format!("{}last", line.repeat(lines)));
        let tail = Read.run(&sandbox.ctx, json!({ "path": "tail.log", "offset": lines + 1 })).await.unwrap();
        assert_eq!(tail.output, format!("{}: last", lines + 1), "a last line without a newline still counts");
        sandbox.ctx.abort.cancel();
        assert_eq!(Read.run(&sandbox.ctx, json!({ "path": "big.log", "offset": lines })).await.unwrap_err().0, "stopped");
    }

    #[tokio::test]
    async fn a_pdf_comes_back_to_look_at() {
        let sandbox = Sandbox::new("read-pdf");
        sandbox.file("spec.pdf", "%PDF-1.7\n1 0 obj\n");
        let out = Read.run(&sandbox.ctx, json!({ "path": "spec.pdf" })).await.unwrap();
        assert_eq!(super::super::image::returned(&out.metadata)[0].mime, "application/pdf");
        assert!(out.output.contains("is a PDF"));
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
