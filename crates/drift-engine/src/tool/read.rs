use std::fmt::Write;

use serde_json::{Value, json};

use super::ToolMetadata;
use super::{Ask, Context, Output, RunFuture, Tool, ToolError, display, required_str};
use crate::llm::ToolSpec;

mod output;
mod page;
mod pdf;

use output::{attached, list_dir, missing, reminders};
use page::{Large, count_lines, large_page, page};

const MAX_LINES: usize = 2000;
/// Past this a file is not loaded whole: its page is read line by line and the rest left on disk.
const WHOLE_BYTES: u64 = 10 * 1024 * 1024;
/// One page stays under the shared result bound, leaving room for the continuation note.
const PAGE_BYTES: usize = super::spool::MAX_RESULT_BYTES - 1024;
/// Subdirectory instructions take at most this much of a read's result; the page has the rest.
const REMINDER_BYTES: usize = PAGE_BYTES / 2;
/// Entries a directory listing shows before saying how many more there are.
const MAX_ENTRIES: usize = 1000;
/// The largest PDF `pages` reads a part of.
const MAX_SPLIT_BYTES: u64 = 100 * 1024 * 1024;

pub struct Read;

impl Tool for Read {
    fn permissions(&self) -> &'static [&'static str] {
        &["read"]
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read".into(),
            description: include_str!("prompts/read.txt").trim().into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File to read. Relative paths resolve against the workspace." },
                    "offset": { "type": "integer", "description": "1-based line to start from; a negative number reads the last that many lines. Default 1." },
                    "limit": { "type": "integer", "description": "Maximum lines to return. Default 2000." },
                    "pages": { "type": "string", "description": "For a PDF: the pages to read, such as \"3\" or \"1-5\", at most 20. Needed for a PDF over 10 pages." }
                },
                "required": ["path"]
            }),
        }
    }

    fn ask(&self, ctx: &Context, input: &Value) -> Option<Ask> {
        ctx.ask_to_read(&ctx.resolve(input["path"].as_str()?), "Read")
    }

    fn starts_early(&self, _input: &Value) -> bool {
        true
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let path = ctx.resolve(required_str(&input, "path")?);
            let span = Span {
                from: From::of(&input),
                limit: input["limit"]
                    .as_u64()
                    .map_or(MAX_LINES, |limit| limit as usize)
                    .clamp(1, MAX_LINES),
            };
            let meta = tokio::fs::metadata(&path).await.map_err(|_| missing(ctx, &path))?;
            if meta.is_dir() {
                return list_dir(ctx, &path).await;
            }
            let pages = input["pages"].as_str().filter(|pages| !pages.trim().is_empty());
            if pages.is_some() || meta.len() <= WHOLE_BYTES {
                let bytes = whole(ctx, &path, meta.len(), pages.is_some()).await?;
                return read_bytes(ctx, &path, &bytes, pages, span);
            }
            if starts_pdf(&path) {
                let name = display(&path, ctx.workdir());
                return Err(ToolError(format!(
                    "{name} is a PDF over 10 MB; read a part of it with `pages`"
                )));
            }

            read_large(ctx, &path, span).await
        })
    }
}

/// The lines a read asks for: where it starts and how many.
#[derive(Clone, Copy)]
struct Span {
    from: From,
    limit: usize,
}

/// Where a read starts: a line, or so many lines from the end.
#[derive(Clone, Copy)]
enum From {
    Line(usize),
    Last(usize),
}

impl From {
    fn of(input: &Value) -> Self {
        match input["offset"].as_i64() {
            Some(back) if back < 0 => Self::Last(back.unsigned_abs() as usize),
            Some(line) => Self::Line((line as usize).max(1)),
            None => Self::Line(1),
        }
    }

    fn line(self, total: usize) -> usize {
        match self {
            Self::Line(line) => line,
            Self::Last(count) => total.saturating_sub(count) + 1,
        }
    }
}

/// The file's bytes, up to 10 MB, or for a part of a PDF up to [`MAX_SPLIT_BYTES`].
async fn whole(ctx: &Context, path: &std::path::Path, size: u64, paged: bool) -> Result<Vec<u8>, ToolError> {
    if paged && size > MAX_SPLIT_BYTES {
        let name = display(path, ctx.workdir());
        return Err(ToolError(format!(
            "{name} is over {} MB",
            MAX_SPLIT_BYTES / 1024 / 1024
        )));
    }

    Ok(tokio::fs::read(path).await?)
}

fn starts_pdf(path: &std::path::Path) -> bool {
    let mut head = [0u8; 5];
    std::fs::File::open(path)
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut head))
        .is_ok_and(|()| super::image::is_pdf(&head))
}

/// An image or PDF to look at, or text by page; `pages` is for PDFs only.
fn read_bytes(
    ctx: &Context,
    path: &std::path::Path,
    bytes: &[u8],
    pages: Option<&str>,
    span: Span,
) -> Result<Output, ToolError> {
    let name = display(path, ctx.workdir());
    if super::image::is_pdf(bytes) {
        let selected = pdf::select(bytes, pages, &name)?;
        ctx.files.mark_read(path);
        return part_attached(ctx, path, &selected);
    }
    if pages.is_some() {
        return Err(ToolError(format!("`pages` is for PDFs, and {name} is not one")));
    }
    if let Some(mime) = super::image::sniff(bytes) {
        ctx.files.mark_read(path);
        return attached(ctx, path, mime, bytes);
    }
    if bytes.iter().take(8000).any(|byte| *byte == 0) {
        return Err(ToolError(format!("{name} is binary")));
    }

    read_text(ctx, path, bytes, span)
}

fn part_attached(ctx: &Context, path: &std::path::Path, selected: &pdf::Selected) -> Result<Output, ToolError> {
    let mut output = attached(ctx, path, super::image::PDF, &selected.bytes)?;
    if let Some((first, last, count)) = selected.part {
        let name = display(path, ctx.workdir());
        output.output = format!(
            "Pages {first} to {last} of {name}'s {count} ({} KB) follow this result.",
            selected.bytes.len().div_ceil(1024)
        );
    }

    Ok(output)
}

fn read_text(ctx: &Context, path: &std::path::Path, bytes: &[u8], span: Span) -> Result<Output, ToolError> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let total = text.lines().count();
    let name = display(path, ctx.workdir());
    let offset = span.from.line(total);
    if total > 0 && offset > total {
        return Err(ToolError(format!(
            "{name} has {total} lines, so offset {offset} is past its end"
        )));
    }
    warm_servers(ctx, path);
    // The reminders come first in the budget, so the page fits beside them within one result.
    let reminders = reminders(ctx, path);
    let body = page(text, offset, span.limit, PAGE_BYTES.saturating_sub(reminders.len()));
    let shown = body.len();
    let mut output = if total == 0 {
        "(the file is empty)".to_string()
    } else {
        body.join("\n")
    };
    if offset - 1 + shown < total {
        write!(
            output,
            "\n\n({} more lines; read with offset {})",
            total - (offset - 1 + shown),
            offset + shown
        )
        .expect("writing to a String cannot fail");
    }
    output.push_str(&reminders);
    ctx.files.mark_read(path);
    Ok(Output {
        title: name,
        output,
        metadata: ToolMetadata {
            lines: Some(Some(total)),
            shown: Some(shown),
            ..Default::default()
        },
    })
}

/// Starts the language servers for a workspace file being read, so they are ready by its first edit, as opencode does.
fn warm_servers(ctx: &Context, path: &std::path::Path) {
    if !path.starts_with(&ctx.workspace) {
        return;
    }
    let (engine, workspace, file, config) = (
        ctx.engine.clone(),
        ctx.workspace.clone(),
        path.to_path_buf(),
        ctx.config.clone(),
    );
    tokio::spawn(async move {
        let servers = engine.hooks.lsp_config(&config.lsp);
        engine.lsp.warm(&workspace, &file, &servers).await;
    });
}

/// Reads a bounded page from a large text file, without scanning the rest for a line count unless it reads from the end.
async fn read_large(ctx: &Context, path: &std::path::Path, span: Span) -> Result<Output, ToolError> {
    let name = display(path, ctx.workdir());
    let reminders = reminders(ctx, path);
    let budget = PAGE_BYTES.saturating_sub(reminders.len());
    let (file, stop) = (path.to_path_buf(), ctx.abort.clone());
    let (read, offset) = tokio::task::spawn_blocking(move || {
        let offset = match span.from {
            From::Line(line) => line,
            From::Last(_) => span.from.line(count_lines(&file, &stop)?),
        };
        large_page(&file, offset, span.limit, budget, &stop).map(|read| (read, offset))
    })
    .await
    .map_err(|error| ToolError(error.to_string()))??;
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
        write!(output, "\n\n(more lines follow; read with offset {})", offset + shown)
            .expect("writing to a String cannot fail");
    }
    output.push_str(&reminders);
    ctx.files.mark_read(path);
    Ok(Output {
        title: name,
        output,
        metadata: ToolMetadata {
            lines: Some(None),
            shown: Some(shown),
            large: Some(true),
            ..Default::default()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::super::tests::Sandbox;
    use super::page::MAX_LINE_CHARS;
    use super::*;

    #[tokio::test]
    async fn numbers_lines_and_pages() {
        let sandbox = Sandbox::new("read");
        sandbox.file("a.txt", "one\ntwo\nthree\nfour\n");
        let out = Read
            .run(&sandbox.ctx, json!({ "path": "a.txt", "offset": 2, "limit": 2 }))
            .await
            .unwrap();
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
        let near = Read.run(&sandbox.ctx, json!({ "path": "src/main" })).await.unwrap_err();
        assert_eq!(
            near.0,
            "src/main does not exist. Did you mean one of these?\nsrc/main.rs"
        );
        sandbox.file("a", "");
        let short = Read
            .run(&sandbox.ctx, json!({ "path": "banana.txt" }))
            .await
            .unwrap_err();
        assert_eq!(
            short.0, "banana.txt does not exist",
            "a one-letter name is not a suggestion"
        );
        let cased = Read.run(&sandbox.ctx, json!({ "path": "readme.MD" })).await;
        assert!(
            cased.is_ok() || cased.unwrap_err().0.ends_with("README.md"),
            "a name in another case is suggested where the file system is case-sensitive"
        );
    }

    #[test]
    fn paths_inside_the_workspace_need_no_permission() {
        let sandbox = Sandbox::new("read-ask");
        assert!(
            Read.ask(&sandbox.ctx, &json!({ "path": "a.txt" }))
                .unwrap()
                .default_allow
        );
        let outside = Read.ask(&sandbox.ctx, &json!({ "path": "C:/Windows/hosts" })).unwrap();
        assert_eq!(outside.kind, "read");
    }

    #[tokio::test]
    async fn pages_stay_within_the_result_bound_and_say_where_to_go_on() {
        let sandbox = Sandbox::new("read-page");
        let wide = "w".repeat(1_500);
        sandbox.file("wide.txt", &format!("{wide}\n").repeat(1_000));
        let first = Read.run(&sandbox.ctx, json!({ "path": "wide.txt" })).await.unwrap();
        assert!(
            first.output.len() <= super::super::spool::MAX_RESULT_BYTES,
            "{}",
            first.output.len()
        );
        let shown = first.metadata.shown.unwrap();
        assert!(shown > 10 && shown < 1_000);
        assert!(
            first.output.ends_with(&format!("read with offset {})", shown + 1)),
            "{}",
            &first.output[first.output.len() - 80..]
        );
        let next = Read
            .run(&sandbox.ctx, json!({ "path": "wide.txt", "offset": shown + 1 }))
            .await
            .unwrap();
        assert!(next.output.starts_with(&format!("{}: w", shown + 1)));

        let huge_line = "h".repeat(super::super::spool::MAX_RESULT_BYTES * 2);
        sandbox.file("one.txt", &huge_line);
        let one = Read.run(&sandbox.ctx, json!({ "path": "one.txt" })).await.unwrap();
        assert!(
            one.output.starts_with("1: hhh") && one.output.len() < 3_000,
            "a single line is cut, not skipped"
        );
    }

    #[tokio::test]
    async fn a_subdirectory_s_instructions_come_with_the_first_read_under_it() {
        let sandbox = Sandbox::new("read-nested");
        sandbox.file("AGENTS.md", "root rules, already in the system prompt");
        sandbox.file("pkg/AGENTS.md", "pkg rules");
        sandbox.file("pkg/web/CLAUDE.md", "web rules");
        sandbox.file("pkg/web/a.ts", "a");
        sandbox.file("pkg/web/b.ts", "b");
        let first = Read.run(&sandbox.ctx, json!({ "path": "pkg/web/a.ts" })).await.unwrap();
        let pkg = first.output.find("pkg rules").expect("pkg/AGENTS.md is shown");
        assert!(
            pkg < first.output.find("web rules").expect("pkg/web/CLAUDE.md too"),
            "outermost first"
        );
        assert!(
            !first.output.contains("root rules"),
            "the workspace's own file is not repeated"
        );
        let second = Read.run(&sandbox.ctx, json!({ "path": "pkg/web/b.ts" })).await.unwrap();
        assert_eq!(second.output, "1: b", "shown once per session");
        let itself = Read
            .run(&sandbox.ctx, json!({ "path": "pkg/AGENTS.md" }))
            .await
            .unwrap();
        assert_eq!(itself.output, "1: pkg rules");
        sandbox.ctx.files.forget_shown();
        assert!(
            Read.run(&sandbox.ctx, json!({ "path": "pkg/web/b.ts" }))
                .await
                .unwrap()
                .output
                .contains("web rules"),
            "shown again once forgotten"
        );
    }

    #[tokio::test]
    async fn a_full_page_and_its_reminder_fit_in_one_result() {
        let sandbox = Sandbox::new("read-nested-full");
        sandbox.file("pkg/AGENTS.md", &"rule line\n".repeat(1_000));
        sandbox.file("pkg/big.txt", &format!("{}\n", "w".repeat(1_500)).repeat(1_000));
        let out = Read.run(&sandbox.ctx, json!({ "path": "pkg/big.txt" })).await.unwrap();
        assert!(
            out.output.len() <= super::super::spool::MAX_RESULT_BYTES,
            "{}",
            out.output.len()
        );
        assert!(
            out.output.ends_with("</system-reminder>"),
            "the reminder is whole, not cut in the middle"
        );
        assert!(out.output.contains("read with offset"), "the page says where to go on");
    }

    #[tokio::test]
    async fn a_file_too_large_to_load_is_read_a_page_at_a_time() {
        let sandbox = Sandbox::new("read-large");
        let line = format!("{}\n", "log entry ".repeat(10));
        let lines = (WHOLE_BYTES as usize / line.len()) + 5_000;
        sandbox.file("big.log", &line.repeat(lines));
        let first = Read
            .run(&sandbox.ctx, json!({ "path": "big.log", "limit": 3 }))
            .await
            .unwrap();
        assert!(
            first.output.starts_with("1: log entry")
                && first.output.ends_with("(more lines follow; read with offset 4)"),
            "{}",
            &first.output
        );
        let deep = Read
            .run(&sandbox.ctx, json!({ "path": "big.log", "offset": lines, "limit": 10 }))
            .await
            .unwrap();
        assert!(
            deep.output.starts_with(&format!("{lines}: log entry")) && !deep.output.contains("more lines"),
            "the last line, with nothing after it"
        );
        assert!(
            Read.run(&sandbox.ctx, json!({ "path": "big.log", "offset": lines + 1 }))
                .await
                .unwrap_err()
                .0
                .contains("fewer than")
        );
        assert!(sandbox.ctx.files.was_read(&sandbox.ctx.workspace.join("big.log")));

        sandbox.file("one-line.log", &"x".repeat(WHOLE_BYTES as usize + 1024));
        let long = Read.run(&sandbox.ctx, json!({ "path": "one-line.log" })).await.unwrap();
        assert!(
            long.output.starts_with("1: xxx")
                && long.output.len() < 3 * MAX_LINE_CHARS
                && !long.output.contains("more lines"),
            "one huge line, cut as it is read"
        );
        sandbox.file("tail.log", &format!("{}last", line.repeat(lines)));
        let tail = Read
            .run(&sandbox.ctx, json!({ "path": "tail.log", "offset": lines + 1 }))
            .await
            .unwrap();
        assert_eq!(
            tail.output,
            format!("{}: last", lines + 1),
            "a last line without a newline still counts"
        );
        let end = Read
            .run(&sandbox.ctx, json!({ "path": "tail.log", "offset": -2 }))
            .await
            .unwrap();
        assert!(
            end.output.starts_with(&format!("{lines}: log entry"))
                && end.output.ends_with(&format!("{}: last", lines + 1)),
            "a negative offset reads the end: {}",
            end.output
        );
        sandbox.ctx.abort.cancel();
        assert_eq!(
            Read.run(&sandbox.ctx, json!({ "path": "big.log", "offset": lines }))
                .await
                .unwrap_err()
                .0,
            "stopped"
        );
    }

    #[tokio::test]
    async fn a_negative_offset_reads_the_last_lines() {
        let sandbox = Sandbox::new("read-tail");
        sandbox.file("a.txt", "one\ntwo\nthree\nfour\n");
        let out = Read
            .run(&sandbox.ctx, json!({ "path": "a.txt", "offset": -2 }))
            .await
            .unwrap();
        assert_eq!(out.output, "3: three\n4: four");
        let all = Read
            .run(&sandbox.ctx, json!({ "path": "a.txt", "offset": -9 }))
            .await
            .unwrap();
        assert!(all.output.starts_with("1: one"), "more than the file has reads it all");
    }

    #[tokio::test]
    async fn a_long_pdf_is_read_by_pages() {
        let sandbox = Sandbox::new("read-pages");
        std::fs::write(sandbox.ctx.workspace.join("long.pdf"), pdf::sample(12)).unwrap();
        let whole = Read.run(&sandbox.ctx, json!({ "path": "long.pdf" })).await.unwrap_err();
        assert!(whole.0.contains("has 12 pages; read it in parts"), "{}", whole.0);
        let part = Read
            .run(&sandbox.ctx, json!({ "path": "long.pdf", "pages": "11-12" }))
            .await
            .unwrap();
        assert!(
            part.output.starts_with("Pages 11 to 12 of long.pdf's 12"),
            "{}",
            part.output
        );
        assert_eq!(super::super::image::returned(&part.metadata)[0].mime, "application/pdf");
        sandbox.file("a.txt", "text");
        let text = Read
            .run(&sandbox.ctx, json!({ "path": "a.txt", "pages": "1" }))
            .await
            .unwrap_err();
        assert!(text.0.contains("for PDFs"), "{}", text.0);
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
        assert_eq!(
            out.output.lines().filter(|line| line.starts_with('f')).count(),
            MAX_ENTRIES
        );
        assert!(
            out.output
                .ends_with("(200 more entries; use glob with a pattern to narrow it)")
        );
    }

    #[tokio::test]
    async fn an_empty_file_says_so_and_an_offset_past_the_end_is_an_error() {
        let sandbox = Sandbox::new("read-ends");
        sandbox.file("empty.txt", "");
        sandbox.file("three.txt", "a\nb\nc\n");
        let empty = Read.run(&sandbox.ctx, json!({ "path": "empty.txt" })).await.unwrap();
        assert_eq!(empty.output, "(the file is empty)");
        let past = Read
            .run(&sandbox.ctx, json!({ "path": "three.txt", "offset": 9 }))
            .await
            .unwrap_err();
        assert_eq!(past.0, "three.txt has 3 lines, so offset 9 is past its end");
        assert_eq!(
            Read.run(&sandbox.ctx, json!({ "path": "three.txt", "offset": 3 }))
                .await
                .unwrap()
                .output,
            "3: c",
            "the last line is still in range"
        );
    }

    #[test]
    fn this_sessions_own_spilled_output_reads_without_asking_and_nothing_else_of_the_data_dir_does() {
        let sandbox = Sandbox::new("read-own-output");
        let data = &sandbox.ctx.engine.data_dir;
        let own = data
            .join("tool-output")
            .join(&sandbox.ctx.session_id)
            .join("call_1.log");
        let other = data.join("tool-output").join("ses_someone_else").join("call_1.log");
        for file in [&own, &other] {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "output").unwrap();
        }
        let ask = |path: &std::path::Path| Read.ask(&sandbox.ctx, &json!({ "path": path.to_string_lossy() }));
        assert!(ask(&own).unwrap().default_allow, "its own output");
        assert!(ask(&other).is_some(), "another session's output");
        assert!(ask(&data.join("drift.db")).is_some(), "the rest of the data dir");
        let escape = data
            .join("tool-output")
            .join(&sandbox.ctx.session_id)
            .join("..")
            .join("ses_someone_else")
            .join("call_1.log");
        assert!(ask(&escape).is_some(), "resolved before it is judged");
    }

    #[test]
    fn secrets_inside_the_workspace_ask_and_their_examples_do_not() {
        let sandbox = Sandbox::new("read-secret");
        let ask = Read
            .ask(&sandbox.ctx, &json!({ "path": "config/.env.local" }))
            .expect("a secret must ask");
        assert_eq!(ask.kind, "read");
        assert_eq!(
            std::path::PathBuf::from(&ask.pattern),
            sandbox.ctx.workspace.join("config/.env.local")
        );
        assert!(ask.title.contains("may hold secrets"), "{}", ask.title);
        assert!(
            Read.ask(&sandbox.ctx, &json!({ "path": "config/.env.example" }))
                .unwrap()
                .default_allow
        );
        assert!(
            Read.ask(&sandbox.ctx, &json!({ "path": "config" }))
                .unwrap()
                .default_allow,
            "listing a directory shows names, not contents"
        );
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
        let ask = Read
            .ask(&sandbox.ctx, &json!({ "path": "../outside.txt" }))
            .expect("traversal must ask");
        assert_eq!(std::path::PathBuf::from(&ask.pattern), outside);
        assert!(
            Read.ask(&sandbox.ctx, &json!({ "path": "sub/../a.txt" }))
                .unwrap()
                .default_allow
        );
        assert!(
            Glob.ask(&sandbox.ctx, &json!({ "pattern": "*", "path": ".." }))
                .is_some()
        );
        assert!(
            Grep.ask(
                &sandbox.ctx,
                &json!({ "pattern": "x", "path": &outside.parent().unwrap().to_string_lossy() })
            )
            .is_some()
        );

        let link = sandbox.ctx.workspace.join("link");
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(outside.parent().unwrap(), &link).is_ok();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(outside.parent().unwrap(), &link).is_ok();
        if made {
            let ask = Read
                .ask(&sandbox.ctx, &json!({ "path": "link/outside.txt" }))
                .expect("symlink escape must ask");
            assert_eq!(std::path::PathBuf::from(&ask.pattern), outside);
        }
        std::fs::remove_file(outside).ok();
    }

    use super::super::glob::Glob;
    use super::super::grep::Grep;
}
