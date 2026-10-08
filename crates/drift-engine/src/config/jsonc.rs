//! drift.json may carry comments and trailing commas, as opencode's JSONC config did.

/// Removes comments and trailing commas while preserving JSON string literals.
pub fn strip(text: &str) -> String {
    let characters: Vec<char> = text.chars().collect();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;

    while index < characters.len() {
        match (characters[index], characters.get(index + 1)) {
            ('"', _) => index = copy_string(&characters, index, &mut output),
            ('/', Some('/')) => {
                while index < characters.len() && characters[index] != '\n' {
                    index += 1;
                }
            }
            ('/', Some('*')) => {
                index += 2;
                while index < characters.len() && !(characters[index] == '*' && characters.get(index + 1) == Some(&'/'))
                {
                    output.push(if characters[index] == '\n' { '\n' } else { ' ' });
                    index += 1;
                }
                index += 2;
            }
            (',', _) if closes_next(&characters, index + 1) => index += 1,
            (character, _) => {
                output.push(character);
                index += 1;
            }
        }
    }

    output
}

/// Copies a string literal, escapes included, and returns the index after its closing quote.
fn copy_string(characters: &[char], start: usize, output: &mut String) -> usize {
    output.push('"');
    let mut index = start + 1;

    while index < characters.len() {
        output.push(characters[index]);
        match characters[index] {
            '\\' if index + 1 < characters.len() => {
                output.push(characters[index + 1]);
                index += 2;
            }
            '"' => return index + 1,
            _ => index += 1,
        }
    }

    index
}

/// Whether only whitespace and comments stand between here and a closing bracket.
fn closes_next(characters: &[char], from: usize) -> bool {
    let mut index = from;

    while index < characters.len() {
        match (characters[index], characters.get(index + 1)) {
            (character, _) if character.is_whitespace() => index += 1,
            ('/', Some('/')) => {
                while index < characters.len() && characters[index] != '\n' {
                    index += 1;
                }
            }
            ('/', Some('*')) => {
                index += 2;
                while index < characters.len() && !(characters[index] == '*' && characters.get(index + 1) == Some(&'/'))
                {
                    index += 1;
                }
                index += 2;
            }
            (character, _) => return matches!(character, '}' | ']'),
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
