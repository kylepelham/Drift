//! `---` blocks of `key: value` lines at the top of a Markdown file. Enough for agents, commands and skills.

use std::collections::BTreeMap;

#[derive(Debug, Default, PartialEq)]
pub struct Document {
    pub fields: BTreeMap<String, String>,
    pub body: String,
}

impl Document {
    pub fn field(&self, key: &str) -> Option<String> {
        self.fields.get(key).map(|v| v.trim_matches(|c| c == '"' || c == '\'').to_string()).filter(|v| !v.is_empty())
    }
}

pub fn parse(text: &str) -> Document {
    let text = text.replace("\r\n", "\n");
    let Some(rest) = text.strip_prefix("---\n") else { return Document { fields: BTreeMap::new(), body: text } };
    let Some((head, body)) = rest.split_once("\n---") else { return Document { fields: BTreeMap::new(), body: text } };
    let fields = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .collect();
    Document { fields, body: body.trim_start_matches('-').trim_start_matches('\n').to_string() }
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
}
