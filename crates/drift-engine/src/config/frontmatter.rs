//! `---` blocks at the top of a Markdown file: `key: value` lines, plus the YAML list and map shapes
//! agent files use for `tools`. Enough for agents, commands and skills.

use std::collections::BTreeMap;

#[derive(Debug, Default, PartialEq)]
pub struct Document {
    pub fields: BTreeMap<String, String>,
    /// Indented lines under a key with no value of its own: `- item` lines, or `name: true|false` lines.
    nested: BTreeMap<String, Vec<String>>,
    pub body: String,
}

impl Document {
    pub fn field(&self, key: &str) -> Option<String> {
        self.fields.get(key).map(|v| unquote(v).to_string()).filter(|v| !v.is_empty())
    }

    /// A list given inline (`a, b` or `[a, b]`), as `- item` lines, or as a `name: true|false` map,
    /// where a false entry comes back as `!name`.
    pub fn list(&self, key: &str) -> Option<Vec<String>> {
        if let Some(items) = self.nested.get(key) {
            return Some(items.clone());
        }
        let inline = self.field(key)?;
        let inner = inline.trim().trim_start_matches(['[', '{']).trim_end_matches([']', '}']);
        Some(inner.split(',').filter_map(entry).collect())
    }
}

pub fn parse(text: &str) -> Document {
    let text = text.replace("\r\n", "\n");
    let Some(rest) = text.strip_prefix("---\n") else { return Document { body: text, ..Document::default() } };
    let Some((head, body)) = rest.split_once("\n---") else { return Document { body: text, ..Document::default() } };
    let mut doc = Document { body: body.trim_start_matches('-').trim_start_matches('\n').to_string(), ..Document::default() };
    let mut open: Option<String> = None;
    for line in head.lines() {
        let indented = line.starts_with([' ', '\t']);
        match (indented, &open) {
            (true, Some(key)) => {
                let item = line.trim().strip_prefix("- ").unwrap_or(line.trim());
                doc.nested.entry(key.clone()).or_default().extend(entry(item));
            }
            _ => {
                let Some((key, value)) = line.split_once(':') else { continue };
                let (key, value) = (key.trim().to_string(), value.trim().to_string());
                open = value.is_empty().then(|| key.clone());
                doc.fields.insert(key, value);
            }
        }
    }
    doc
}

/// One list or map entry: `name`, `name: true` or `name: false` (as `!name`).
fn entry(text: &str) -> Option<String> {
    let (name, flag) = match text.split_once(':') {
        Some((name, flag)) => (name, Some(flag.trim())),
        None => (text, None),
    };
    let name = unquote(name.trim());
    if name.is_empty() {
        return None;
    }
    Some(if flag == Some("false") { format!("!{name}") } else { name.to_string() })
}

fn unquote(text: &str) -> &str {
    text.trim_matches(|c| c == '"' || c == '\'')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fields_and_body() {
        let doc = parse("---\nname: x\ndescription: \"Does things: well\"\n---\n\nBody here.\n");
        assert_eq!(doc.field("name").as_deref(), Some("x"));
        assert_eq!(doc.field("description").as_deref(), Some("Does things: well"));
        assert_eq!(doc.body, "Body here.\n");
        assert_eq!(parse("no front matter").body, "no front matter");
        assert_eq!(parse("---\nname: x\n---").body, "");
    }

    #[test]
    fn lists_come_inline_as_items_or_as_a_map() {
        let list = |head: &str| parse(&format!("---\n{head}\n---\nbody")).list("tools");
        assert_eq!(list("tools: Read, Grep"), Some(vec!["Read".into(), "Grep".into()]));
        assert_eq!(list("tools: [read, \"grep\"]"), Some(vec!["read".into(), "grep".into()]));
        assert_eq!(list("tools:\n  - read\n  - grep\nmodel: x"), Some(vec!["read".into(), "grep".into()]));
        assert_eq!(list("tools:\n  write: false\n  bash: true"), Some(vec!["!write".into(), "bash".into()]));
        assert_eq!(list("tools: { write: false }"), Some(vec!["!write".into()]));
        assert_eq!(list("model: x"), None);
        assert_eq!(parse("---\ntools:\n  - read\nmodel: x\n---\n").field("model").as_deref(), Some("x"), "a list ends at the next key");
    }
}
