use std::iter::Peekable;
use std::str::Split;

type Lines<'a> = Peekable<Split<'a, char>>;

struct Line {
    kind: u8,
    text: String,
    newline: bool,
}

struct Hunk {
    new_start: usize,
    new_len: usize,
    lines: Vec<Line>,
}

pub(super) fn reverse(diff: &str, current: &str) -> Option<String> {
    let hunks = parse(diff)?;
    let line_ending = if current.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = current.split_inclusive('\n').map(String::from).collect();

    // Reverse from the end so replacing a hunk cannot move an earlier hunk's position.
    for hunk in hunks.iter().rev() {
        reverse_hunk(hunk, &mut lines, line_ending)?;
    }

    Some(lines.concat())
}

fn reverse_hunk(hunk: &Hunk, lines: &mut Vec<String>, line_ending: &str) -> Option<()> {
    let start = if hunk.new_len == 0 {
        hunk.new_start
    } else {
        hunk.new_start.checked_sub(1)?
    };
    let end = start.checked_add(hunk.new_len).filter(|end| *end <= lines.len())?;
    let mut index = start;
    let mut replacement = Vec::new();

    for line in &hunk.lines {
        if line.kind == b'-' {
            let suffix = if line.newline { line_ending } else { "" };
            replacement.push(format!("{}{suffix}", line.text));
            continue;
        }

        // Every new-side line must match; guessing would undo someone else's changes.
        if bare(&lines[index]) != line.text {
            return None;
        }

        if line.kind == b' ' {
            replacement.push(lines[index].clone());
        }
        index += 1;
    }

    lines.splice(start..end, replacement);
    Some(())
}

fn bare(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

fn parse(diff: &str) -> Option<Vec<Hunk>> {
    let mut lines = diff.split('\n').peekable();
    let mut hunks = Vec::new();

    while let Some(line) = lines.next() {
        if let Some(header) = line.strip_prefix("@@ -") {
            hunks.push(parse_hunk(header, &mut lines)?);
        }
    }

    Some(hunks)
}

fn parse_hunk(header: &str, lines: &mut Lines<'_>) -> Option<Hunk> {
    let (old, rest) = header.split_once(" +")?;
    let (_, old_len) = range(old)?;
    let (new_start, new_len) = range(rest.split_once(" @@")?.0)?;
    let body = parse_body(lines, old_len, new_len)?;

    Some(Hunk {
        new_start,
        new_len,
        lines: body,
    })
}

fn parse_body(lines: &mut Lines<'_>, mut old_left: usize, mut new_left: usize) -> Option<Vec<Line>> {
    let mut body: Vec<Line> = Vec::new();

    // Counts delimit the body because file content can look like a diff header.
    while old_left > 0 || new_left > 0 {
        let line = lines.next()?;
        let kind = line.as_bytes().first().copied().unwrap_or(b' ');
        match kind {
            b' ' => (old_left, new_left) = (old_left.checked_sub(1)?, new_left.checked_sub(1)?),
            b'-' => old_left = old_left.checked_sub(1)?,
            b'+' => new_left = new_left.checked_sub(1)?,
            b'\\' => {
                body.last_mut()?.newline = false;
                continue;
            }
            _ => return None,
        }

        let text = line.get(1..).unwrap_or_default();
        body.push(Line {
            kind,
            text: text.strip_suffix('\r').unwrap_or(text).to_string(),
            newline: true,
        });
    }

    while lines.next_if(|line| line.starts_with('\\')).is_some() {
        if let Some(last) = body.last_mut() {
            last.newline = false;
        }
    }

    Some(body)
}

fn range(text: &str) -> Option<(usize, usize)> {
    match text.split_once(',') {
        Some((start, len)) => Some((start.parse().ok()?, len.parse().ok()?)),
        None => Some((text.parse().ok()?, 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = concat!(
        "Index: a.rs\n===================================================================\n--- a.rs\n+++ a.rs\n",
        "@@ -1,4 +1,4 @@\n one\n-two\n+TWO\n three\n four\n@@ -8,2 +8,3 @@\n eight\n nine\n+ten\n",
    );

    #[test]
    fn a_diff_is_undone_where_it_says_in_the_files_own_line_endings() {
        let current = "one\nTWO\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n";

        assert_eq!(
            reverse(DIFF, current).as_deref(),
            Some("one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\n")
        );

        let crlf = current.replace('\n', "\r\n");

        assert_eq!(
            reverse(DIFF, &crlf).as_deref(),
            Some("one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\n")
        );
    }

    #[test]
    fn a_file_changed_since_the_diff_is_not_undone() {
        assert_eq!(
            reverse(DIFF, "one\nTWO\nthree\nFOUR\nfive\nsix\nseven\neight\nnine\nten\n"),
            None,
            "a context line differs"
        );
        assert_eq!(reverse(DIFF, "one\nTWO\n"), None, "the file is shorter than the diff");
    }

    #[test]
    fn added_and_deleted_files_and_a_missing_final_newline_round_trip() {
        let added = "--- /dev/null\n+++ b.rs\n@@ -0,0 +1,2 @@\n+x\n+y\n\\ No newline at end of file\n";
        assert_eq!(reverse(added, "x\ny").as_deref(), Some(""));

        let deleted = "--- b.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-x\n-y\n\\ No newline at end of file\n";
        assert_eq!(reverse(deleted, "").as_deref(), Some("x\ny"));

        let tail = "@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+c\n";
        assert_eq!(
            reverse(tail, "a\nc\n").as_deref(),
            Some("a\nb"),
            "the old last line had no newline"
        );
    }

    #[test]
    fn lines_that_look_like_headers_are_read_as_content() {
        let diff = "@@ -1,2 +1,2 @@\n--- old dashes\n+++ new pluses\n keep\n";

        assert_eq!(
            reverse(diff, "++ new pluses\nkeep\n").as_deref(),
            Some("-- old dashes\nkeep\n")
        );
    }

    #[test]
    fn malformed_ranges_and_incomplete_hunks_refuse_to_rebuild_a_version() {
        for diff in [
            "@@ -a +1 @@\n-x\n+y\n",
            "@@ -1,1 +1,1 @@\n+unexpected\n",
            "@@ -1 +0 @@\n-x\n+y\n",
            "@@ -0,0 +1,1 @@\n-x\n+y\n",
        ] {
            assert_eq!(reverse(diff, "y\n"), None, "{diff}");
        }
    }
}
