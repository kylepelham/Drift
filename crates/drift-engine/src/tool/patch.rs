//! The apply_patch format GPT models are trained on: a small envelope around per-file hunks.

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum ParseError {
    #[error("missing *** Begin Patch")]
    MissingBegin,
    #[error("missing *** End Patch")]
    MissingEnd,
    #[error("*** End Patch comes before *** Begin Patch")]
    ReversedEnvelope,
    #[error("unexpected line in patch: {0}")]
    UnexpectedLine(String),
    #[error("patch contains no operations")]
    NoOperations,
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum HunkError {
    #[error("hunk {hunk} did not match the file. {region}")]
    Nearby { hunk: usize, region: String },
    #[error(
        "hunk {hunk} did not match the file; read the file and copy the context lines exactly. Looking for:\n{wanted}"
    )]
    Missing { hunk: usize, wanted: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Add {
        path: String,
        content: String,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_to: Option<String>,
        chunks: Vec<Chunk>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    /// The `@@` line, if any: a line of the file to anchor the search near.
    pub context: Option<String>,
    pub old: Vec<String>,
    pub new: Vec<String>,
    pub end_of_file: bool,
}

impl Op {
    pub fn path(&self) -> &str {
        match self {
            Op::Add { path, .. } | Op::Delete { path } | Op::Update { path, .. } => path,
        }
    }
}

pub fn parse(text: &str) -> Result<Vec<Op>, ParseError> {
    let normalised = text.replace("\r\n", "\n");
    let lines: Vec<&str> = normalised.lines().collect();
    let begin = lines
        .iter()
        .position(|line| line.trim() == "*** Begin Patch")
        .ok_or(ParseError::MissingBegin)?;
    let end = lines
        .iter()
        .rposition(|line| line.trim() == "*** End Patch")
        .ok_or(ParseError::MissingEnd)?;
    if end < begin {
        return Err(ParseError::ReversedEnvelope);
    }

    let mut ops = Vec::new();
    let mut i = begin + 1;
    while i < end {
        let line = lines[i];
        if let Some(path) = line.strip_prefix("*** Add File:") {
            let (content, next) = add_content(&lines, i + 1, end);
            ops.push(Op::Add {
                path: path.trim().into(),
                content,
            });
            i = next;
        } else if let Some(path) = line.strip_prefix("*** Delete File:") {
            ops.push(Op::Delete {
                path: path.trim().into(),
            });
            i += 1;
        } else if let Some(path) = line.strip_prefix("*** Update File:") {
            let (op, next) = update(path, &lines, i + 1, end);
            ops.push(op);
            i = next;
        } else if line.trim().is_empty() {
            i += 1;
        } else {
            return Err(ParseError::UnexpectedLine(line.into()));
        }
    }

    if ops.is_empty() {
        return Err(ParseError::NoOperations);
    }
    Ok(ops)
}

/// An `*** Update File:` section whose body starts at line `i`, with the line after it.
fn update(path: &str, lines: &[&str], mut i: usize, end: usize) -> (Op, usize) {
    let move_to = lines
        .get(i)
        .and_then(|line| line.strip_prefix("*** Move to:"))
        .map(|to| to.trim().to_string());
    if move_to.is_some() {
        i += 1;
    }

    let (chunks, next) = chunks(lines, i, end);
    let op = Op::Update {
        path: path.trim().into(),
        move_to,
        chunks,
    };
    (op, next)
}

fn add_content(lines: &[&str], mut i: usize, end: usize) -> (String, usize) {
    let mut content = Vec::new();
    while i < end && !lines[i].starts_with("***") {
        content.push(lines[i].strip_prefix('+').unwrap_or(lines[i]));
        i += 1;
    }

    let mut text = content.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    (text, i)
}

fn chunks(lines: &[&str], mut i: usize, end: usize) -> (Vec<Chunk>, usize) {
    let mut chunks = Vec::new();
    let mut current: Option<Chunk> = None;
    while i < end && (lines[i] == "*** End of File" || !lines[i].starts_with("***")) {
        let line = lines[i];
        if let Some(context) = line.strip_prefix("@@") {
            chunks.extend(current.take());
            let context = context.trim();
            current = Some(Chunk {
                context: (!context.is_empty()).then(|| context.to_string()),
                old: vec![],
                new: vec![],
                end_of_file: false,
            });
        } else if line == "*** End of File" {
            if let Some(chunk) = &mut current {
                chunk.end_of_file = true;
            }
        } else {
            let chunk = current.get_or_insert_with(|| Chunk {
                context: None,
                old: vec![],
                new: vec![],
                end_of_file: false,
            });
            match line.chars().next() {
                Some('-') => chunk.old.push(line[1..].into()),
                Some('+') => chunk.new.push(line[1..].into()),
                _ => {
                    let kept = line.strip_prefix(' ').unwrap_or(line).to_string();
                    chunk.old.push(kept.clone());
                    chunk.new.push(kept);
                }
            }
        }
        i += 1;
    }

    chunks.extend(current);
    (chunks, i)
}

/// Applies every chunk in order; each search starts where the last one ended.
pub fn apply_chunks(content: &str, chunks: &[Chunk]) -> Result<String, HunkError> {
    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    let trailing_newline = content.ends_with('\n') || content.is_empty();
    let mut cursor = 0;

    for (index, chunk) in chunks.iter().enumerate() {
        let at = locate(&lines, chunk, cursor).ok_or_else(|| miss(content, index, chunk))?;
        lines.splice(at..at + chunk.old.len(), chunk.new.iter().cloned());
        cursor = at + chunk.new.len();
    }

    let mut out = lines.join("\n");
    if trailing_newline && !out.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

fn locate(lines: &[String], chunk: &Chunk, from: usize) -> Option<usize> {
    let start = match &chunk.context {
        Some(context) => seek(lines, std::slice::from_ref(context), from, false).map_or(from, |at| at + 1),
        None => from,
    };

    if chunk.old.is_empty() {
        return Some(if chunk.end_of_file {
            lines.len()
        } else {
            start.min(lines.len())
        });
    }
    seek(lines, &chunk.old, start, chunk.end_of_file)
}

/// How apply_patch finds a hunk, as Codex's own `seek_sequence` does, since the models it is offered to
/// (GPT and Codex) write patches that rely on it: exactly, then ignoring trailing whitespace, then
/// ignoring surrounding whitespace, then with Unicode dashes, quotes and spaces read as ASCII. The
/// first pass that matches wins. `edit` stays exact.
const PASSES: [fn(&str, &str) -> bool; 4] = [
    |a, b| a == b,
    |a, b| a.trim_end() == b.trim_end(),
    |a, b| a.trim() == b.trim(),
    |a, b| ascii_punctuation(a.trim()) == ascii_punctuation(b.trim()),
];

/// Where `pattern` starts at or after `from`, by the first pass that finds it; an end-of-file hunk
/// tries the file's end first.
fn seek(lines: &[String], pattern: &[String], from: usize, end_of_file: bool) -> Option<usize> {
    PASSES.iter().find_map(|same| {
        let at_end = lines
            .len()
            .checked_sub(pattern.len())
            .filter(|at| end_of_file && *at >= from);
        at_end.filter(|at| matches_at(lines, pattern, *at, *same)).or_else(|| {
            (from..=lines.len().saturating_sub(pattern.len())).find(|at| matches_at(lines, pattern, *at, *same))
        })
    })
}

fn matches_at(lines: &[String], pattern: &[String], at: usize, same: fn(&str, &str) -> bool) -> bool {
    lines.len() >= at + pattern.len()
        && lines[at..at + pattern.len()]
            .iter()
            .zip(pattern)
            .all(|(line, wanted)| same(line, wanted))
}

/// Codex's normalisation: typographic dashes, quotes and spaces as their ASCII forms.
fn ascii_punctuation(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            '\u{2018}'..='\u{201B}' => '\'',
            '\u{201C}'..='\u{201F}' => '"',
            '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

/// What a missed hunk says: the part of the file it most likely meant, as `edit` does, else what it looked for.
fn miss(content: &str, index: usize, chunk: &Chunk) -> HunkError {
    let wanted: Vec<&str> = chunk.old.iter().map(String::as_str).collect();
    let hunk = index + 1;

    match super::edit::closest_region(content, &wanted) {
        Some(region) => HunkError::Nearby { hunk, region },
        None => HunkError::Missing {
            hunk,
            wanted: wanted.iter().take(4).copied().collect::<Vec<_>>().join("\n"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "*** Begin Patch\n*** Add File: new.txt\n+hello\n+world\n*** Delete File: old.txt\n*** Update File: src/a.rs\n*** Move to: src/b.rs\n@@ fn main() {\n-    let x = 1;\n+    let x = 2;\n     println!(\"{x}\");\n*** End Patch\n";

    #[test]
    fn parses_every_operation_kind() {
        let ops = parse(PATCH).unwrap();
        assert_eq!(
            ops[0],
            Op::Add {
                path: "new.txt".into(),
                content: "hello\nworld\n".into()
            }
        );
        assert_eq!(ops[1], Op::Delete { path: "old.txt".into() });
        let Op::Update { path, move_to, chunks } = &ops[2] else {
            panic!()
        };
        assert_eq!(path, "src/a.rs");
        assert_eq!(move_to.as_deref(), Some("src/b.rs"));
        assert_eq!(chunks[0].context.as_deref(), Some("fn main() {"));
        assert_eq!(chunks[0].old, ["    let x = 1;", "    println!(\"{x}\");"]);
        assert_eq!(chunks[0].new, ["    let x = 2;", "    println!(\"{x}\");"]);
    }

    #[test]
    fn rejects_bad_envelopes() {
        assert!(parse("nothing").is_err());
        assert!(parse("*** Begin Patch\n*** End Patch\n").is_err());
        assert!(parse("*** Begin Patch\nrandom\n*** End Patch\n").is_err());
    }

    #[test]
    fn applies_chunks_in_order_with_context_anchoring() {
        let file = "a\nfn one() {\n    x\n}\nfn two() {\n    x\n}\n";
        let ops = parse("*** Begin Patch\n*** Update File: f\n@@ fn two() {\n-    x\n+    y\n*** End Patch\n").unwrap();
        let Op::Update { chunks, .. } = &ops[0] else { panic!() };
        assert_eq!(
            apply_chunks(file, chunks).unwrap(),
            "a\nfn one() {\n    x\n}\nfn two() {\n    y\n}\n"
        );
    }

    #[test]
    fn chunks_without_headers_and_end_of_file_work() {
        let ops =
            parse("*** Begin Patch\n*** Update File: f\n-b\n+B\n@@\n+z\n*** End of File\n*** End Patch\n").unwrap();
        let Op::Update { chunks, .. } = &ops[0] else { panic!() };
        assert_eq!(chunks.len(), 2);
        assert!(chunks[1].end_of_file);
        assert_eq!(apply_chunks("a\nb\nc\n", chunks).unwrap(), "a\nB\nc\nz\n");
    }

    #[test]
    fn hunks_match_as_codex_matches_them_and_exact_wins() {
        let patch = |old: &str, new: &str| {
            let ops = parse(&format!(
                "*** Begin Patch\n*** Update File: f\n-{old}\n+{new}\n*** End Patch\n"
            ))
            .unwrap();
            let Op::Update { chunks, .. } = ops.into_iter().next().unwrap() else {
                panic!()
            };
            chunks
        };
        assert_eq!(
            apply_chunks("let x = 1;   \nz\n", &patch("let x = 1;", "let x = 2;")).unwrap(),
            "let x = 2;\nz\n",
            "trailing whitespace"
        );
        assert_eq!(
            apply_chunks("    indented\n", &patch("indented", "done")).unwrap(),
            "done\n",
            "surrounding whitespace"
        );
        assert_eq!(
            apply_chunks(
                "say \u{201C}hi\u{201D} \u{2014} ok\n",
                &patch("say \"hi\" - ok", "said")
            )
            .unwrap(),
            "said\n",
            "typographic punctuation"
        );
        assert_eq!(
            apply_chunks("a \nb\na\n", &patch("a", "A")).unwrap(),
            "a \nb\nA\n",
            "an exact match anywhere beats a loose one earlier"
        );
        assert!(apply_chunks("something else\n", &patch("nothing like it", "x")).is_err());
    }

    #[test]
    fn a_miss_names_the_hunk() {
        let ops = parse("*** Begin Patch\n*** Update File: f\n-nope\n+x\n*** End Patch\n").unwrap();
        let Op::Update { chunks, .. } = &ops[0] else { panic!() };
        let err = apply_chunks("a\n", chunks).unwrap_err().to_string();
        assert!(err.starts_with("hunk 1 did not match"));
        let near = parse(
            "*** Begin Patch\n*** Update File: f\n fn two() {\n-    let x = 1;\n+    let x = 2;\n*** End Patch\n",
        )
        .unwrap();
        let Op::Update { chunks, .. } = &near[0] else { panic!() };
        let err = apply_chunks("a\nb\nfn two() {\n    let x = 3;\n}\n", chunks)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("closest region is lines") && err.contains("3: fn two() {"),
            "{err}"
        );
    }
}
