//! The apply_patch format GPT models are trained on: a small envelope around per-file hunks.

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Add { path: String, content: String },
    Delete { path: String },
    Update { path: String, move_to: Option<String>, chunks: Vec<Chunk> },
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

pub fn parse(text: &str) -> Result<Vec<Op>, String> {
    let normalised = text.replace("\r\n", "\n");
    let lines: Vec<&str> = normalised.lines().collect();
    let begin = lines.iter().position(|l| l.trim() == "*** Begin Patch").ok_or("missing *** Begin Patch")?;
    let end = lines.iter().rposition(|l| l.trim() == "*** End Patch").ok_or("missing *** End Patch")?;
    if end < begin {
        return Err("*** End Patch comes before *** Begin Patch".into());
    }
    let mut ops = Vec::new();
    let mut i = begin + 1;
    while i < end {
        let line = lines[i];
        if let Some(path) = line.strip_prefix("*** Add File:") {
            let (content, next) = add_content(&lines, i + 1, end);
            ops.push(Op::Add { path: path.trim().into(), content });
            i = next;
        } else if let Some(path) = line.strip_prefix("*** Delete File:") {
            ops.push(Op::Delete { path: path.trim().into() });
            i += 1;
        } else if let Some(path) = line.strip_prefix("*** Update File:") {
            let mut next = i + 1;
            let move_to = lines.get(next).and_then(|l| l.strip_prefix("*** Move to:")).map(|p| p.trim().to_string());
            if move_to.is_some() {
                next += 1;
            }
            let (chunks, after) = chunks(&lines, next, end);
            ops.push(Op::Update { path: path.trim().into(), move_to, chunks });
            i = after;
        } else if line.trim().is_empty() {
            i += 1;
        } else {
            return Err(format!("unexpected line in patch: {line}"));
        }
    }
    if ops.is_empty() {
        return Err("patch contains no operations".into());
    }
    Ok(ops)
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
            current = Some(Chunk { context: (!context.is_empty()).then(|| context.to_string()), old: vec![], new: vec![], end_of_file: false });
        } else if line == "*** End of File" {
            if let Some(chunk) = &mut current {
                chunk.end_of_file = true;
            }
        } else {
            let chunk = current.get_or_insert_with(|| Chunk { context: None, old: vec![], new: vec![], end_of_file: false });
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
pub fn apply_chunks(content: &str, chunks: &[Chunk]) -> Result<String, String> {
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
    if chunk.end_of_file {
        let at = lines.len().checked_sub(chunk.old.len())?;
        return matches_at(lines, &chunk.old, at).then_some(at);
    }
    let start = match &chunk.context {
        Some(context) => (from..lines.len()).find(|i| lines[*i].trim() == context.trim()).map_or(from, |i| i + 1),
        None => from,
    };
    if chunk.old.is_empty() {
        return Some(start.min(lines.len()));
    }
    (start..=lines.len().saturating_sub(chunk.old.len())).find(|at| matches_at(lines, &chunk.old, *at))
}

fn matches_at(lines: &[String], old: &[String], at: usize) -> bool {
    lines.len() >= at + old.len() && lines[at..at + old.len()].iter().zip(old).all(|(a, b)| a == b)
}

/// What a missed hunk says: the part of the file it most likely meant, as `edit` does, else what it looked for.
fn miss(content: &str, index: usize, chunk: &Chunk) -> String {
    let wanted: Vec<&str> = chunk.old.iter().map(String::as_str).collect();
    match super::edit::closest_region(content, &wanted) {
        Some(region) => format!("hunk {} did not match the file. {region}", index + 1),
        None => format!("hunk {} did not match the file; read the file and copy the context lines exactly. Looking for:\n{}", index + 1, wanted.iter().take(4).copied().collect::<Vec<_>>().join("\n")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "*** Begin Patch\n*** Add File: new.txt\n+hello\n+world\n*** Delete File: old.txt\n*** Update File: src/a.rs\n*** Move to: src/b.rs\n@@ fn main() {\n-    let x = 1;\n+    let x = 2;\n     println!(\"{x}\");\n*** End Patch\n";

    #[test]
    fn parses_every_operation_kind() {
        let ops = parse(PATCH).unwrap();
        assert_eq!(ops[0], Op::Add { path: "new.txt".into(), content: "hello\nworld\n".into() });
        assert_eq!(ops[1], Op::Delete { path: "old.txt".into() });
        let Op::Update { path, move_to, chunks } = &ops[2] else { panic!() };
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
        assert_eq!(apply_chunks(file, chunks).unwrap(), "a\nfn one() {\n    x\n}\nfn two() {\n    y\n}\n");
    }

    #[test]
    fn chunks_without_headers_and_end_of_file_work() {
        let ops = parse("*** Begin Patch\n*** Update File: f\n-b\n+B\n@@\n+z\n*** End of File\n*** End Patch\n").unwrap();
        let Op::Update { chunks, .. } = &ops[0] else { panic!() };
        assert_eq!(chunks.len(), 2);
        assert!(chunks[1].end_of_file);
        assert_eq!(apply_chunks("a\nb\nc\n", chunks).unwrap(), "a\nB\nc\nz\n");
    }

    #[test]
    fn a_miss_names_the_hunk() {
        let ops = parse("*** Begin Patch\n*** Update File: f\n-nope\n+x\n*** End Patch\n").unwrap();
        let Op::Update { chunks, .. } = &ops[0] else { panic!() };
        let err = apply_chunks("a\n", chunks).unwrap_err();
        assert!(err.starts_with("hunk 1 did not match"));
        let near = parse("*** Begin Patch\n*** Update File: f\n fn two() {\n-    let x = 1;\n+    let x = 2;\n*** End Patch\n").unwrap();
        let Op::Update { chunks, .. } = &near[0] else { panic!() };
        let err = apply_chunks("a\nb\nfn two() {\n    let x = 3;\n}\n", chunks).unwrap_err();
        assert!(err.contains("closest region is lines") && err.contains("3: fn two() {"), "{err}");
    }
}
