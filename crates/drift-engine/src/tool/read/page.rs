use std::io::BufRead;
use std::path::Path;

use tokio_util::sync::CancellationToken;

pub(super) const MAX_LINE_CHARS: usize = 2000;
/// The most of one line kept in memory: enough for [`MAX_LINE_CHARS`] characters of any width.
const LINE_BYTES: usize = MAX_LINE_CHARS * 4 + 4;

pub(super) struct Large {
    pub(super) lines: Vec<String>,
    pub(super) more: bool,
    pub(super) binary: bool,
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
        let text = if number == 1 {
            text.strip_prefix('\u{feff}').unwrap_or(&text)
        } else {
            &text
        };
        let numbered = format!("{number}: {}", truncate(text.trim_end_matches(['\n', '\r'])));

        self.used += numbered.len() + 1;
        if self.lines.len() == self.limit || (self.used > self.budget && !self.lines.is_empty()) {
            return false;
        }

        self.lines.push(numbered);
        true
    }
}

/// Lines `offset..` of `path`, numbered as [`page`] does, read a buffer at a time: no line is held past [`LINE_BYTES`], and a Stop is seen between buffers.
pub(super) fn large_page(
    path: &Path,
    offset: usize,
    limit: usize,
    budget: usize,
    stop: &CancellationToken,
) -> std::io::Result<Large> {
    let mut reader = std::io::BufReader::with_capacity(1 << 16, std::fs::File::open(path)?);
    if reader.fill_buf()?.iter().take(8000).any(|byte| *byte == 0) {
        return Ok(Large {
            lines: Vec::new(),
            more: false,
            binary: true,
        });
    }

    let mut page = Page {
        lines: Vec::new(),
        used: 0,
        limit,
        budget,
    };
    let (mut number, mut line, mut started) = (1, Vec::new(), false);

    while !stop.is_cancelled() {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            let more = started && number >= offset && !page.push(number, &line);
            return Ok(Large {
                lines: page.lines,
                more,
                binary: false,
            });
        }

        let (take, ended) = chunk
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or((chunk.len(), false), |at| (at + 1, true));
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
            return Ok(Large {
                lines: page.lines,
                more: true,
                binary: false,
            });
        }

        line.clear();
        number += 1;
    }

    Ok(Large {
        lines: page.lines,
        more: false,
        binary: false,
    })
}

/// Numbered lines from `offset`, at most `limit` of them and within `budget` bytes; always at least
/// one line, so every read makes progress.
pub(super) fn page(text: &str, offset: usize, limit: usize, budget: usize) -> Vec<String> {
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
