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
            return Some(items.iter().filter_map(|item| entry(item.trim().strip_prefix("- ").unwrap_or(item.trim()))).collect());
        }
        let inline = self.field(key)?;
        let inner = inline.trim().trim_start_matches(['[', '{']).trim_end_matches([']', '}']);
        Some(inner.split(',').filter_map(entry).collect())
    }

    /// Rules in the order written; `Config::agent_policy` makes the last match win.
    pub fn permissions(&self) -> Result<Vec<crate::permission::Rule>, String> {
        let key = if self.fields.contains_key("permissions") { "permissions" } else { "permission" };
        if let Some(value) = self.fields.get(key).map(|value| value.trim()).filter(|value| !value.is_empty()) {
            if value.starts_with('[') { return serde_json::from_str(value).map_err(|_| "permissions must be an array of kind/pattern/decision rules".into()); }
            if value.starts_with('{') { return map_permissions(&Flow::parse(value)?); }
            return Ok(vec![rule("*", "*", unquote(value))?]);
        }
        let Some(lines) = self.nested.get(key) else { return Ok(Vec::new()) };
        let base = lines.iter().map(|line| line.len() - line.trim_start().len()).min().unwrap_or(0);
        let mut parent = None;
        let mut rules = Vec::new();
        for line in lines {
            let depth = line.len() - line.trim_start().len();
            let (name, value) = line.trim().rsplit_once(':').ok_or("permission entries must name a rule")?;
            let (name, value) = (unquote(name.trim()), unquote(value.trim()));
            if depth == base {
                parent = Some(name);
                if !value.is_empty() { rules.push(rule(name, "*", value)?); }
            } else {
                rules.push(rule(parent.ok_or("permission pattern has no tool namespace")?, name, value)?);
            }
        }
        Ok(rules)
    }
}

/// A YAML flow value (`{ edit: deny, bash: { "git *": allow } }`, JSON included), keys in the order written.
#[derive(Debug, PartialEq)]
enum Flow {
    Text(String),
    Map(Vec<(String, Flow)>),
}

impl Flow {
    fn parse(text: &str) -> Result<Self, String> {
        let mut chars = text.chars().peekable();
        let value = Self::value(&mut chars)?;
        skip_space(&mut chars);
        match chars.next() {
            None => Ok(value),
            Some(c) => Err(format!("unexpected `{c}` after the permission map")),
        }
    }

    fn value(chars: &mut std::iter::Peekable<std::str::Chars>) -> Result<Self, String> {
        skip_space(chars);
        if chars.peek() != Some(&'{') {
            return scalar(chars, &[',', '}']).map(Flow::Text);
        }
        chars.next();
        let mut entries = Vec::new();
        loop {
            skip_space(chars);
            if chars.peek() == Some(&'}') { chars.next(); return Ok(Flow::Map(entries)); }
            let key = scalar(chars, &[':'])?;
            if chars.next() != Some(':') { return Err(format!("permission entry {key} needs a `:`")); }
            entries.push((key, Self::value(chars)?));
            skip_space(chars);
            match chars.next() {
                Some(',') => {}
                Some('}') => return Ok(Flow::Map(entries)),
                _ => return Err("a permission map must close with `}`".into()),
            }
        }
    }
}

fn skip_space(chars: &mut std::iter::Peekable<std::str::Chars>) {
    while chars.next_if(|c| c.is_whitespace()).is_some() {}
}

/// A quoted string, or bare text up to one of `ends`.
fn scalar(chars: &mut std::iter::Peekable<std::str::Chars>, ends: &[char]) -> Result<String, String> {
    skip_space(chars);
    if let Some(quote) = chars.next_if(|c| *c == '"' || *c == '\'') {
        let text: String = chars.by_ref().take_while(|c| *c != quote).collect();
        skip_space(chars);
        return Ok(text);
    }
    let mut text = String::new();
    while let Some(c) = chars.next_if(|c| !ends.contains(c) && *c != '}') {
        text.push(c);
    }
    let text = text.trim().to_string();
    if text.is_empty() { Err("a permission map has an empty entry".into()) } else { Ok(text) }
}

fn rule(kind: &str, pattern: &str, value: &str) -> Result<crate::permission::Rule, String> {
    let decision = match value { "allow" => crate::permission::Decision::Allow, "ask" => crate::permission::Decision::Ask, "deny" => crate::permission::Decision::Deny, _ => return Err("permission decisions must be allow, ask or deny".into()) };
    Ok(crate::permission::Rule { kind: kind.into(), pattern: pattern.into(), decision })
}

fn map_permissions(value: &Flow) -> Result<Vec<crate::permission::Rule>, String> {
    let Flow::Map(kinds) = value else { return Err("permission must be a tool map".into()) };
    let mut rules = Vec::new();
    for (kind, value) in kinds {
        match value {
            Flow::Text(decision) => rules.push(rule(kind, "*", decision)?),
            Flow::Map(patterns) => {
                for (pattern, decision) in patterns {
                    let Flow::Text(decision) = decision else { return Err("permission decisions must be allow, ask or deny".into()) };
                    rules.push(rule(kind, pattern, decision)?);
                }
            }
        }
    }
    Ok(rules)
}

pub fn parse(text: &str) -> Document {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text).replace("\r\n", "\n");
    let Some(rest) = text.strip_prefix("---\n") else { return Document { body: text, ..Document::default() } };
    let Some((head, body)) = rest.split_once("\n---") else { return Document { body: text, ..Document::default() } };
    let mut doc = Document { body: body.trim_start_matches('-').trim_start_matches('\n').to_string(), ..Document::default() };
    let mut open: Option<String> = None;
    for line in head.lines() {
        let indented = line.starts_with([' ', '\t']);
        match (indented, &open) {
            (true, Some(key)) => {
                doc.nested.entry(key.clone()).or_default().push(line.to_string());
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

/// One list or map entry: `name`, or in a map `name: false` (as `!name`). A map's `name: true`
/// changes nothing, as in opencode, where it only leaves the tool on.
fn entry(text: &str) -> Option<String> {
    let (name, flag) = match text.split_once(':') {
        Some((name, flag)) => (name, Some(flag.trim())),
        None => (text, None),
    };
    let name = unquote(name.trim());
    match flag {
        _ if name.is_empty() => None,
        Some("false") => Some(format!("!{name}")),
        Some(_) => None,
        None => Some(name.to_string()),
    }
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
        assert_eq!(list("tools:\n  write: false\n  bash: true"), Some(vec!["!write".into()]), "true only leaves a tool on");
        assert_eq!(list("tools: []"), Some(vec![]));
        assert_eq!(list("tools: { write: false }"), Some(vec!["!write".into()]));
        assert_eq!(list("model: x"), None);
        assert_eq!(parse("---\ntools:\n  - read\nmodel: x\n---\n").field("model").as_deref(), Some("x"), "a list ends at the next key");
    }

    #[test]
    fn permission_maps_keep_namespaces_and_specific_pattern_overrides() {
        let doc = parse("---\npermission:\n  read:\n    \"*\": allow\n    \"private*\": deny\n  bash: ask\nvariant: high\n---\nbody");
        let rules = doc.permissions().unwrap().into_iter().rev().collect();
        let policy = crate::permission::Policy { rules };
        assert_eq!(policy.explicit(&crate::tool::Ask::new("read", "private.txt", "")), Some(crate::permission::Decision::Deny));
        assert_eq!(policy.explicit(&crate::tool::Ask::new("read", "other.txt", "")), Some(crate::permission::Decision::Allow));
        assert_eq!(policy.explicit(&crate::tool::Ask::new("bash", "git status", "")), Some(crate::permission::Decision::Ask));
        assert_eq!(doc.field("variant").as_deref(), Some("high"));
        assert!(parse("---\npermission: {\"read\":\"invalid\"}\n---\n").permissions().is_err());
    }

    #[test]
    fn yaml_flow_maps_are_read_and_the_last_matching_entry_wins() {
        use crate::permission::Decision;
        let decide = |head: &str, kind: &str, target: &str| {
            let rules = parse(&format!("---\n{head}\n---\n")).permissions().unwrap().into_iter().rev().collect();
            crate::permission::Policy { rules }.explicit(&crate::tool::Ask::new(kind, target, ""))
        };
        let flow = "permission: { edit: deny, bash: { \"*\": ask, 'git *': allow } }";
        assert_eq!(decide(flow, "edit", "a.rs"), Some(Decision::Deny));
        assert_eq!(decide(flow, "bash", "git status"), Some(Decision::Allow), "written after `*`, so it wins");
        assert_eq!(decide(flow, "bash", "rm -rf x"), Some(Decision::Ask));
        let reversed = "permission: { bash: { 'git *': allow, \"*\": ask } }";
        assert_eq!(decide(reversed, "bash", "git status"), Some(Decision::Ask), "as in opencode, a later `*` overrides");
        assert_eq!(decide("permission: {\"bash\": {\"*\": \"deny\"}}", "bash", "ls"), Some(Decision::Deny), "JSON is a flow map too");
        assert_eq!(decide("permission:\n  \"*\": ask\n  read: allow", "read", "a"), Some(Decision::Allow));
        assert!(parse("---\npermission: { edit: deny\n---\n").permissions().is_err());
    }
}
