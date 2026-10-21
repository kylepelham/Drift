//! Where `old_string` sits in a file and what replaces it. The search runs on the LF view the model
//! read: exact first, then with typographic quotes, dashes and spaces folded, then ignoring each line's
//! trailing whitespace; a tier is tried only when the one before found nothing, and more than one match
//! is still ambiguous. Matches map back to the file's own bytes, and the replacement takes the line
//! endings of the region it replaces, so a mixed file keeps each region's endings.

use std::ops::Range;

use super::super::text::ascii;

/// Lines compared to keep a region's endings; past this the replacement pairs lines by position.
const MAX_ALIGN_CELLS: usize = 4_000_000;

pub(super) enum Refusal {
    Missing,
    Ambiguous(usize),
    /// The edit would write the same bytes back; `endings_only` when it only changed line endings.
    Unchanged {
        endings_only: bool,
    },
}

type Found = Vec<(Range<usize>, String)>;

/// The edited text and how many places matched. Strings that differ only in `\r` ask to change line
/// endings themselves, so they apply to the exact bytes.
pub(super) fn replace(source: &str, old: &str, new: &str, replace_all: bool) -> Result<(String, usize), Refusal> {
    let endings_only = old.replace('\r', "") == new.replace('\r', "");
    let found = if endings_only {
        occurrences(source, old)
            .into_iter()
            .map(|range| (range, new.to_string()))
            .collect()
    } else {
        in_view(source, old, new)
    };

    let count = found.len();
    match (count, replace_all) {
        (0, _) => return Err(Refusal::Missing),
        (1, _) | (_, true) => {}
        (places, false) => return Err(Refusal::Ambiguous(places)),
    }
    let mut text = source.to_string();
    for (range, with) in found.into_iter().rev() {
        text.replace_range(range, &with);
    }
    if text == source {
        return Err(Refusal::Unchanged { endings_only });
    }

    Ok((text, count))
}

/// Non-overlapping places `needle` appears.
fn occurrences(haystack: &str, needle: &str) -> Vec<Range<usize>> {
    haystack
        .match_indices(needle)
        .map(|(start, found)| start..start + found.len())
        .collect()
}

fn in_view(source: &str, old: &str, new: &str) -> Found {
    let view = source.replace("\r\n", "\n");
    // Where each removed `\r` sat in the view, so a view offset maps back to the file's.
    let removed: Vec<usize> = source
        .match_indices("\r\n")
        .enumerate()
        .map(|(index, (at, _))| at - index)
        .collect();
    let back = |offset: usize| offset + removed.partition_point(|at| *at < offset);
    let replacement = new.replace("\r\n", "\n");
    let file_ending = dominant(source, "\n");

    tiers(&view, &old.replace("\r\n", "\n"))
        .into_iter()
        .map(|range| {
            let (start, end) = (back(range.start), back(range.end));
            let region = &source[start..end];
            (
                start..end,
                with_region_endings(region, &replacement, dominant(region, file_ending)),
            )
        })
        .collect()
}

fn tiers(view: &str, search: &str) -> Vec<Range<usize>> {
    let exact = occurrences(view, search);
    if !exact.is_empty() {
        return exact;
    }
    let folded = folded_occurrences(view, search);
    if !folded.is_empty() {
        return folded;
    }

    line_occurrences(view, search)
}

/// Matches with typography folded; each folded character stands for one original, so offsets map back.
fn folded_occurrences(view: &str, search: &str) -> Vec<Range<usize>> {
    let mut folded = String::with_capacity(view.len());
    let mut origin = Vec::with_capacity(view.len() + 1);
    for (at, character) in view.char_indices() {
        let ascii = ascii(character);
        origin.extend(std::iter::repeat_n(at, ascii.len_utf8()));
        folded.push(ascii);
    }
    origin.push(view.len());
    let search: String = search.chars().map(ascii).collect();

    occurrences(&folded, &search)
        .into_iter()
        .map(|range| origin[range.start]..origin[range.end])
        .collect()
}

/// A line as the trailing-whitespace tier compares it.
fn key(line: &str) -> String {
    line.trim_end().chars().map(ascii).collect()
}

struct Line {
    start: usize,
    /// After its newline, when it has one.
    end: usize,
    /// Before its newline and any `\r` ahead of it.
    content_end: usize,
    newline: bool,
}

fn lines(view: &str) -> Vec<Line> {
    let mut found = Vec::new();
    let mut start = 0;
    for piece in view.split_inclusive('\n') {
        let newline = piece.ends_with('\n');
        let text = piece.strip_suffix('\n').unwrap_or(piece);
        let content_end = start + text.len() - usize::from(text.ends_with('\r'));
        found.push(Line {
            start,
            end: start + piece.len(),
            content_end,
            newline,
        });
        start += piece.len();
    }

    found
}

/// Whole lines that match ignoring trailing whitespace, earliest first, never overlapping.
fn line_occurrences(view: &str, search: &str) -> Vec<Range<usize>> {
    let trailing_newline = search.ends_with('\n');
    let wanted: Vec<String> = search
        .strip_suffix('\n')
        .unwrap_or(search)
        .split('\n')
        .map(key)
        .collect();
    let lines = lines(view);
    let keys: Vec<String> = lines
        .iter()
        .map(|line| key(&view[line.start..line.content_end]))
        .collect();

    let mut found: Vec<Range<usize>> = Vec::new();
    for index in 0..lines.len().saturating_sub(wanted.len() - 1) {
        let last = &lines[index + wanted.len() - 1];
        if keys[index..index + wanted.len()] != wanted[..] || (trailing_newline && !last.newline) {
            continue;
        }
        let range = lines[index].start..if trailing_newline { last.end } else { last.content_end };
        if found.last().is_none_or(|previous| previous.end <= range.start) {
            found.push(range);
        }
    }

    found
}

/// The line ending most of `text` uses, or `fallback` when neither has more.
fn dominant(text: &str, fallback: &'static str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;

    match crlf.cmp(&lf) {
        std::cmp::Ordering::Greater => "\r\n",
        std::cmp::Ordering::Less => "\n",
        std::cmp::Ordering::Equal => fallback,
    }
}

/// The replacement's lines joined with endings from the region it replaces: a line that keeps a region
/// line, or takes the place of one, reuses its ending; only lines beyond those take `fallback`.
fn with_region_endings(region: &str, replacement: &str, fallback: &str) -> String {
    let before: Vec<String> = region
        .split('\n')
        .map(|line| key(line.strip_suffix('\r').unwrap_or(line)))
        .collect();
    let endings: Vec<&str> = region
        .match_indices('\n')
        .map(|(at, _)| if region[..at].ends_with('\r') { "\r\n" } else { "\n" })
        .collect();
    let after: Vec<&str> = replacement.split('\n').collect();
    let keys: Vec<String> = after.iter().map(|line| key(line)).collect();
    let source = align(&before, &keys);

    let mut joined = String::with_capacity(replacement.len() + after.len());
    for (index, line) in after.iter().enumerate() {
        joined.push_str(line);
        if index + 1 < after.len() {
            let kept = source[index].and_then(|line| endings.get(line).copied());
            joined.push_str(kept.unwrap_or(fallback));
        }
    }

    joined
}

/// For each line of `after`, the `before` line it keeps (longest common subsequence) or, between kept
/// lines, the one it takes the place of; `None` for lines with no counterpart.
fn align(before: &[String], after: &[String]) -> Vec<Option<usize>> {
    let width = after.len() + 1;
    if (before.len() + 1) * width > MAX_ALIGN_CELLS {
        return (0..after.len())
            .map(|line| (line < before.len()).then_some(line))
            .collect();
    }

    let mut table = vec![0_u32; (before.len() + 1) * width];
    for i in (0..before.len()).rev() {
        for j in (0..after.len()).rev() {
            table[i * width + j] = if before[i] == after[j] {
                table[(i + 1) * width + j + 1] + 1
            } else {
                table[(i + 1) * width + j].max(table[i * width + j + 1])
            };
        }
    }

    let mut result = vec![None; after.len()];
    let mut gap = Gap::default();
    let (mut i, mut j) = (0, 0);
    while i < before.len() && j < after.len() {
        if before[i] == after[j] {
            gap.pair(&mut result);
            result[j] = Some(i);
            (i, j) = (i + 1, j + 1);
        } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
            gap.before.push(i);
            i += 1;
        } else {
            gap.after.push(j);
            j += 1;
        }
    }
    gap.before.extend(i..before.len());
    gap.after.extend(j..after.len());
    gap.pair(&mut result);

    result
}

/// Lines between two kept ones: each new line takes the place of the old line at the same position.
#[derive(Default)]
struct Gap {
    before: Vec<usize>,
    after: Vec<usize>,
}

impl Gap {
    fn pair(&mut self, result: &mut [Option<usize>]) {
        for (index, line) in self.after.drain(..).enumerate() {
            result[line] = self.before.get(index).copied();
        }
        self.before.clear();
    }
}
