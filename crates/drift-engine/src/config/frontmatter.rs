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

    pub fn permissions(&self) -> Result<Vec<crate::permission::Rule>, String> {
        let key = if self.fields.contains_key("permissions") { "permissions" } else { "permission" };
        if let Some(value) = self.field(key) {
            if value.starts_with('[') { return serde_json::from_str(&value).map_err(|_| "permissions must be an array of kind/pattern/decision rules".into()); }
            if value.starts_with('{') { return map_permissions(serde_json::from_str(&value).map_err(|_| "permission maps must be valid JSON")?); }
            return Ok(vec![rule("*", "*", &value)?]);
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
        prioritise(&mut rules);
        Ok(rules)
    }
}

fn rule(kind: &str, pattern: &str, value: &str) -> Result<crate::permission::Rule, String> {
    let decision = match value { "allow" => crate::permission::Decision::Allow, "ask" => crate::permission::Decision::Ask, "deny" => crate::permission::Decision::Deny, _ => return Err("permission decisions must be allow, ask or deny".into()) };
    Ok(crate::permission::Rule { kind: kind.into(), pattern: pattern.into(), decision })
}

fn map_permissions(value: serde_json::Value) -> Result<Vec<crate::permission::Rule>, String> {
    let mut rules = Vec::new();
    for (kind, value) in value.as_object().ok_or("permission must be a tool map")? {
        if let Some(value) = value.as_str() { rules.push(rule(kind, "*", value)?); continue; }
        for (pattern, decision) in value.as_object().ok_or("permission patterns must be a map")? {
            rules.push(rule(kind, pattern, decision.as_str().ok_or("permission decisions must be strings")?)?);
        }
    }
    prioritise(&mut rules);
    Ok(rules)
}

fn prioritise(rules: &mut [crate::permission::Rule]) {
    rules.sort_by_key(|rule| (rule.kind == "*", rule.pattern == "*", std::cmp::Reverse(rule.pattern.len())));
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
        let rules = doc.permissions().unwrap();
        let policy = crate::permission::Policy { rules };
        assert_eq!(policy.explicit(&crate::tool::Ask::new("read", "private.txt", "")), Some(crate::permission::Decision::Deny));
        assert_eq!(policy.explicit(&crate::tool::Ask::new("read", "other.txt", "")), Some(crate::permission::Decision::Allow));
        assert_eq!(policy.explicit(&crate::tool::Ask::new("bash", "git status", "")), Some(crate::permission::Decision::Ask));
        assert_eq!(doc.field("variant").as_deref(), Some("high"));
        assert!(parse("---\npermission: {\"read\":\"invalid\"}\n---\n").permissions().is_err());
    }
}
