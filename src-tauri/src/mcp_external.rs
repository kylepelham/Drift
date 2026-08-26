//! Locating and rewriting MCP servers defined in the user's own OpenCode config files.
//!
//! Drift's registry materializes into its private engine-config directory, so servers the user
//! wrote into `opencode.json`, `opencode.jsonc`, or `config.json` reach Drift only as observed
//! fingerprints. This module finds the exact file member that produced an observed fingerprint and
//! edits or removes that byte span alone, leaving the rest of the file - comments included -
//! untouched. The fingerprint doubles as the staleness guard: a definition that no longer hashes
//! to what the user saw is never modified.

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// Config files larger than this are never parsed or rewritten. Matches the watcher's read bound.
const MAX_EXTERNAL_FILE_BYTES: u64 = 1_048_576;

/// One `mcp.<name>` member found in a config file, with the byte spans needed to rewrite it.
pub struct ExternalLocation {
    pub path: PathBuf,
    pub text: String,
    pub object: McpObjectSpans,
    pub index: usize,
    pub config: Value,
}

/// Byte spans of the top-level `"mcp"` object and each of its members, measured on the
/// comment-neutralized text. Neutralization is byte-for-byte, so they are valid in the original.
pub struct McpObjectSpans {
    pub members: Vec<McpMemberSpan>,
}

pub struct McpMemberSpan {
    pub name: String,
    pub key_start: usize,
    pub value_start: usize,
    pub value_end: usize,
}

/// Replaces comments with spaces and neutralizes trailing commas so the result parses as strict
/// JSON while every remaining byte keeps its original offset.
pub fn neutralize_jsonc(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = bytes.to_vec();
    let mut position = 0;
    let mut in_string = false;
    let mut escaped = false;
    while position < out.len() {
        let byte = out[position];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            position += 1;
            continue;
        }
        match byte {
            b'"' => {
                in_string = true;
                position += 1;
            }
            b'/' if position + 1 < out.len() && out[position + 1] == b'/' => {
                while position < out.len() && out[position] != b'\n' && out[position] != b'\r' {
                    out[position] = b' ';
                    position += 1;
                }
            }
            b'/' if position + 1 < out.len() && out[position + 1] == b'*' => {
                out[position] = b' ';
                out[position + 1] = b' ';
                position += 2;
                while position < out.len() {
                    if out[position] == b'*' && position + 1 < out.len() && out[position + 1] == b'/' {
                        out[position] = b' ';
                        out[position + 1] = b' ';
                        position += 2;
                        break;
                    }
                    if out[position] != b'\n' && out[position] != b'\r' {
                        out[position] = b' ';
                    }
                    position += 1;
                }
            }
            _ => position += 1,
        }
    }
    // Trailing commas: a comma whose next non-whitespace byte closes a container is JSONC-only.
    let mut position = 0;
    let mut in_string = false;
    let mut escaped = false;
    while position < out.len() {
        let byte = out[position];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else if byte == b'"' {
            in_string = true;
        } else if byte == b',' {
            let mut next = position + 1;
            while next < out.len() && out[next].is_ascii_whitespace() {
                next += 1;
            }
            if next < out.len() && (out[next] == b'}' || out[next] == b']') {
                out[position] = b' ';
            }
        }
        position += 1;
    }
    // Comment and comma bytes were replaced with ASCII spaces; multibyte string content is intact.
    String::from_utf8(out).unwrap_or_default()
}

struct Scanner<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Scanner<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            position: 0,
        }
    }

    fn skip_whitespace(&mut self) {
        while self.position < self.bytes.len() && self.bytes[self.position].is_ascii_whitespace() {
            self.position += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        if self.peek() != Some(byte) {
            return Err(format!(
                "expected '{}' at byte {}",
                byte as char, self.position
            ));
        }
        self.position += 1;
        Ok(())
    }

    /// Advances past one JSON string token, returning its byte span.
    fn parse_string(&mut self) -> Result<(usize, usize), String> {
        let start = self.position;
        self.expect(b'"')?;
        let mut escaped = false;
        while let Some(byte) = self.peek() {
            self.position += 1;
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                return Ok((start, self.position));
            }
        }
        Err("unterminated string".into())
    }

    /// Advances past one JSON value of any type.
    fn parse_value(&mut self) -> Result<(), String> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'"') => self.parse_string().map(|_| ()),
            Some(b'{') => self.parse_container(b'{', b'}'),
            Some(b'[') => self.parse_container(b'[', b']'),
            Some(_) => {
                // Numbers and literals: consume until a structural delimiter.
                while let Some(byte) = self.peek() {
                    if byte.is_ascii_whitespace() || matches!(byte, b',' | b'}' | b']') {
                        break;
                    }
                    self.position += 1;
                }
                Ok(())
            }
            None => Err("unexpected end of input".into()),
        }
    }

    fn parse_container(&mut self, open: u8, close: u8) -> Result<(), String> {
        self.expect(open)?;
        self.skip_whitespace();
        if self.peek() == Some(close) {
            self.position += 1;
            return Ok(());
        }
        loop {
            if open == b'{' {
                self.skip_whitespace();
                self.parse_string()?;
                self.skip_whitespace();
                self.expect(b':')?;
            }
            self.parse_value()?;
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.position += 1;
                }
                Some(byte) if byte == close => {
                    self.position += 1;
                    return Ok(());
                }
                _ => return Err(format!("malformed container at byte {}", self.position)),
            }
        }
    }
}

/// Finds the top-level `"mcp"` object and the spans of its members in neutralized JSON text.
pub fn mcp_object_spans(neutral: &str) -> Result<Option<McpObjectSpans>, String> {
    let mut scanner = Scanner::new(neutral);
    scanner.skip_whitespace();
    scanner.expect(b'{')?;
    loop {
        scanner.skip_whitespace();
        if scanner.peek() == Some(b'}') {
            return Ok(None);
        }
        let (key_start, key_end) = scanner.parse_string()?;
        let key: String = serde_json::from_str(&neutral[key_start..key_end])
            .map_err(|error| error.to_string())?;
        scanner.skip_whitespace();
        scanner.expect(b':')?;
        scanner.skip_whitespace();
        if key == "mcp" {
            if scanner.peek() != Some(b'{') {
                return Ok(None);
            }
            return member_spans(&mut scanner, neutral).map(Some);
        }
        scanner.parse_value()?;
        scanner.skip_whitespace();
        match scanner.peek() {
            Some(b',') => scanner.position += 1,
            Some(b'}') => return Ok(None),
            _ => return Err("malformed root object".into()),
        }
    }
}

fn member_spans(scanner: &mut Scanner, neutral: &str) -> Result<McpObjectSpans, String> {
    scanner.expect(b'{')?;
    let mut members = Vec::new();
    loop {
        scanner.skip_whitespace();
        if scanner.peek() == Some(b'}') {
            scanner.position += 1;
            return Ok(McpObjectSpans { members });
        }
        let (key_start, key_end) = scanner.parse_string()?;
        let name: String = serde_json::from_str(&neutral[key_start..key_end])
            .map_err(|error| error.to_string())?;
        scanner.skip_whitespace();
        scanner.expect(b':')?;
        scanner.skip_whitespace();
        let value_start = scanner.position;
        scanner.parse_value()?;
        members.push(McpMemberSpan {
            name,
            key_start,
            value_start,
            value_end: scanner.position,
        });
        scanner.skip_whitespace();
        match scanner.peek() {
            Some(b',') => scanner.position += 1,
            Some(b'}') => {
                scanner.position += 1;
                return Ok(McpObjectSpans { members });
            }
            _ => return Err("malformed mcp object".into()),
        }
    }
}

/// Canonical serialization matching the mcp-approval plugin: sorted keys in UTF-16 code-unit
/// order and `JSON.stringify`-compatible scalar formatting. `None` mirrors the plugin declining
/// to fingerprint (non-finite numbers cannot appear in parsed values).
fn canonical(value: &Value) -> Option<String> {
    match value {
        Value::Null | Value::Bool(_) | Value::String(_) => serde_json::to_string(value).ok(),
        Value::Number(number) => {
            // JSON.parse collapses whole floats like `30000.0` to integers before stringify.
            if let Some(float) = number.as_f64() {
                if float.fract() == 0.0 && float.abs() <= 9_007_199_254_740_991.0 {
                    return Some(format!("{}", float as i64));
                }
            }
            serde_json::to_string(number).ok()
        }
        Value::Array(items) => {
            let encoded: Option<Vec<String>> = items.iter().map(canonical).collect();
            Some(format!("[{}]", encoded?.join(",")))
        }
        Value::Object(entries) => {
            let mut sorted: Vec<(&String, &Value)> = entries.iter().collect();
            sorted.sort_by(|(left, _), (right, _)| {
                left.encode_utf16()
                    .collect::<Vec<_>>()
                    .cmp(&right.encode_utf16().collect::<Vec<_>>())
            });
            let mut parts = Vec::with_capacity(sorted.len());
            for (key, item) in sorted {
                let key = serde_json::to_string(key).ok()?;
                parts.push(format!("{key}:{}", canonical(item)?));
            }
            Some(format!("{{{}}}", parts.join(",")))
        }
    }
}

/// The fingerprint the mcp-approval plugin computes for a named definition.
pub fn fingerprint(name: &str, config: &Value) -> Option<String> {
    let effective: Map<String, Value> = config
        .as_object()?
        .iter()
        .filter(|(key, _)| key.as_str() != "enabled")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let serialized = canonical(&Value::Array(vec![
        Value::String(name.to_string()),
        Value::Object(effective),
    ]))?;
    let digest = Sha256::digest(serialized.as_bytes());
    Some(format!("sha256:{digest:x}"))
}

/// Reads one candidate file's `mcp` member spans. Unreadable, oversized, or unparseable files
/// yield `None`: they cannot have produced the fingerprint the engine reported.
fn read_members(path: &PathBuf) -> Option<(String, String, McpObjectSpans)> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_EXTERNAL_FILE_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let neutral = neutralize_jsonc(&text);
    let object = mcp_object_spans(&neutral).ok()??;
    Some((text, neutral, object))
}

/// Every `mcp` member name defined across the candidate files, so a rename can be rejected when
/// a different config layer already defines the target name.
pub fn defined_names(files: &[PathBuf]) -> Vec<String> {
    files
        .iter()
        .filter_map(read_members)
        .flat_map(|(_, _, object)| object.members.into_iter().map(|member| member.name))
        .collect()
}

/// Scans candidate config files for members named `name` whose definition hashes to `fingerprint`.
pub fn locate(files: &[PathBuf], name: &str, expected: &str) -> Vec<ExternalLocation> {
    let mut result = Vec::new();
    for path in files {
        let Some((text, neutral, object)) = read_members(path) else {
            continue;
        };
        for index in 0..object.members.len() {
            let member = &object.members[index];
            if member.name != name {
                continue;
            }
            let Ok(config) =
                serde_json::from_str::<Value>(&neutral[member.value_start..member.value_end])
            else {
                continue;
            };
            if fingerprint(name, &config).as_deref() == Some(expected) {
                result.push(ExternalLocation {
                    path: path.clone(),
                    text: text.clone(),
                    object: McpObjectSpans {
                        members: object
                            .members
                            .iter()
                            .map(|item| McpMemberSpan {
                                name: item.name.clone(),
                                key_start: item.key_start,
                                value_start: item.value_start,
                                value_end: item.value_end,
                            })
                            .collect(),
                    },
                    index,
                    config,
                });
            }
        }
    }
    result
}

/// Rewrites the located member as `"name": config`, renaming it when `name` differs.
pub fn apply_save(location: &ExternalLocation, name: &str, config: &Value) -> Result<String, String> {
    let member = &location.object.members[location.index];
    if name != member.name
        && location
            .object
            .members
            .iter()
            .any(|item| item.name == name)
    {
        return Err(format!(
            "An MCP server named {name} already exists in {}",
            location.path.display()
        ));
    }
    let key = serde_json::to_string(name).map_err(|error| error.to_string())?;
    let value = serde_json::to_string(config).map_err(|error| error.to_string())?;
    let mut text = location.text.clone();
    text.replace_range(member.key_start..member.value_end, &format!("{key}: {value}"));
    verify(&text, name, Some(config))?;
    Ok(text)
}

/// Deletes the located member together with whichever separating comma keeps the object valid.
pub fn apply_remove(location: &ExternalLocation) -> Result<String, String> {
    let members = &location.object.members;
    let member = &members[location.index];
    let (start, end) = if location.index + 1 < members.len() {
        (member.key_start, members[location.index + 1].key_start)
    } else if location.index > 0 {
        (members[location.index - 1].value_end, member.value_end)
    } else {
        (member.key_start, member.value_end)
    };
    let mut text = location.text.clone();
    text.replace_range(start..end, "");
    verify(&text, &member.name, None)?;
    Ok(text)
}

/// Re-parses the rewritten file and confirms the member now matches the intent. A verification
/// failure means the span math was wrong for this input; nothing is written in that case.
fn verify(text: &str, name: &str, expected: Option<&Value>) -> Result<(), String> {
    let parsed: Value = serde_json::from_str(&neutralize_jsonc(text))
        .map_err(|_| "the rewritten config file would not parse; the original was left untouched")?;
    let current = parsed.get("mcp").and_then(|mcp| mcp.get(name));
    match expected {
        Some(config) if current == Some(config) => Ok(()),
        None if current.is_none() => Ok(()),
        _ => Err("the rewritten config file did not contain the expected change; the original was left untouched".into()),
    }
}

/// Candidate config files: every root the engine reads user config from. Mirrors the watcher's
/// root list so anything that can define an observed server is searched.
pub fn candidate_files(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = roots
        .iter()
        .flat_map(|root| {
            [
                root.join("opencode.json"),
                root.join("opencode.jsonc"),
                root.join("config.json"),
            ]
        })
        .collect();
    files.sort();
    files.dedup();
    files
}

/// Display forms of every located defining file, for user-facing messages.
pub fn display_paths(locations: &[ExternalLocation]) -> Vec<String> {
    locations
        .iter()
        .map(|location| location.path.display().to_string())
        .collect()
}

#[cfg(test)]
#[path = "mcp_external_tests.rs"]
mod tests;
