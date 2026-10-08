//! drift.json may carry comments and trailing commas, as opencode's JSONC config did.

/// The text as strict JSON: comments become spaces and trailing commas go; strings are untouched.
pub fn strip(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        match (chars[i], chars.get(i + 1)) {
            ('"', _) => i = copy_string(&chars, i, &mut out),
            ('/', Some('/')) => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            ('/', Some('*')) => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    out.push(if chars[i] == '\n' { '\n' } else { ' ' });
                    i += 1;
                }
                i += 2;
            }
            (',', _) if closes_next(&chars, i + 1) => i += 1,
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Copies a string literal, escapes included, and returns the index after its closing quote.
fn copy_string(chars: &[char], start: usize, out: &mut String) -> usize {
    out.push('"');
    let mut i = start + 1;
    while i < chars.len() {
        out.push(chars[i]);
        match chars[i] {
            '\\' if i + 1 < chars.len() => {
                out.push(chars[i + 1]);
                i += 2;
            }
            '"' => return i + 1,
            _ => i += 1,
        }
    }
    i
}

/// Whether only whitespace and comments stand between here and a closing bracket.
fn closes_next(chars: &[char], from: usize) -> bool {
    let mut i = from;
    while i < chars.len() {
        match (chars[i], chars.get(i + 1)) {
            (c, _) if c.is_whitespace() => i += 1,
            ('/', Some('/')) => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            ('/', Some('*')) => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
            }
            (c, _) => return matches!(c, '}' | ']'),
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::strip;

    #[test]
    fn comments_and_trailing_commas_go_and_strings_stay() {
        let text = "{\n  // deny pushes\n  \"permissions\": [\n    { \"pattern\": \"git push*\", \"note\": \"a // in a string, and /* this */\", }, /* last */\n  ],\n}";
        let value: serde_json::Value = serde_json::from_str(&strip(text)).unwrap();
        assert_eq!(value["permissions"][0]["pattern"], "git push*");
        assert_eq!(value["permissions"][0]["note"], "a // in a string, and /* this */");
        let escaped = r#"{ "a": "quote \" then // not a comment", }"#;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&strip(escaped)).unwrap()["a"],
            "quote \" then // not a comment"
        );
        assert_eq!(strip(r#"[1, 2]"#), "[1, 2]", "a comma between values stays");
    }
}
