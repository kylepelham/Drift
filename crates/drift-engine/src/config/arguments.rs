//! What a skill documents about its arguments, read without running it or interpreting its instructions:
//! alternatives in its `argument-hint` (`[audit|polish] [target]`), a table with Command and Description
//! columns, and literal invocations (`/design hooks <on|off>`). Fenced examples and other skills'
//! commands are not read, and a free-form hint (`[target]`) stays usage help, never a choice.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Subcommand {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<String>,
}

/// The one skill a command's template calls (`skill({ name: "design" })`), when it calls exactly one.
pub fn referenced_skill(template: &str) -> Option<String> {
    let mut found: Vec<String> = Vec::new();
    for (at, _) in template.match_indices("skill") {
        let boundary = template[..at].chars().next_back().is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        if let Some(name) = call_name(&template[at + "skill".len()..]).filter(|_| boundary) {
            if !found.contains(&name) {
                found.push(name);
            }
        }
    }
    (found.len() == 1).then(|| found.remove(0))
}

/// `({ name: "x" })` and its quoted-key and single-quoted spellings.
fn call_name(text: &str) -> Option<String> {
    let quote = |text: &str| text.strip_prefix(['"', '\'']).map(str::to_string).unwrap_or_else(|| text.to_string());
    let text = text.trim_start().strip_prefix('(')?.trim_start().strip_prefix('{')?.trim_start();
    let text = quote(text);
    let text = quote(text.strip_prefix("name")?);
    let text = text.trim_start().strip_prefix(':')?.trim_start();
    let open = text.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let rest = &text[1..];
    let name = &rest[..rest.find(open)?];
    let named = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    let closed = rest[name.len() + 1..].trim_start().strip_prefix('}').is_some_and(|tail| tail.trim_start().starts_with(')'));
    (named && closed).then(|| name.to_string())
}

/// The usage to show and the choices to offer for skill `name`.
pub fn skill_arguments(name: &str, content: &str, hint: Option<&str>) -> (Option<String>, Vec<Subcommand>) {
    let mut found = Vec::new();
    for choice in hint.and_then(hint_choices).unwrap_or_default() {
        add(&mut found, choice, "");
    }
    let prefix = format!("/{name} ");
    let mut fence: Option<(char, usize)> = None;
    let mut columns: Option<(usize, usize)> = None;
    for line in content.lines() {
        if let Some((marker, length)) = fence_marker(line) {
            fence = match fence {
                None => Some((marker, length)),
                Some((open, opened)) if open == marker && length >= opened => None,
                still => still,
            };
            columns = None;
            continue;
        }
        if fence.is_some() {
            continue;
        }
        if line.trim().starts_with('|') {
            columns = table_row(&mut found, &cells(line), columns, &prefix);
            continue;
        }
        columns = None;
        invocations(&mut found, line, &prefix);
    }
    (hint.map(str::to_string), found)
}

/// `[shape · audit|critique]` lists choices; `[target]` names one free-form argument, which is none.
fn hint_choices(hint: &str) -> Option<Vec<&str>> {
    let rest = hint.strip_prefix(['[', '<'])?;
    let inner = &rest[..rest.find([']', '>'])?];
    (!inner.is_empty() && inner.contains(['|', '·', ','])).then(|| inner.split(['|', '·', ',']).map(str::trim).collect())
}

/// A header row names the columns; a row under it adds its command. The columns, while the table lasts.
fn table_row(found: &mut Vec<Subcommand>, row: &[String], columns: Option<(usize, usize)>, prefix: &str) -> Option<(usize, usize)> {
    let named = |pattern: &[&str]| row.iter().position(|cell| pattern.iter().any(|name| cell.eq_ignore_ascii_case(name)));
    if let (Some(command), Some(description)) = (named(&["command", "subcommand"]), named(&["description"])) {
        return Some((command, description));
    }
    let (command, description) = columns?;
    if let Some(spec) = row.get(command).filter(|spec| !spec.is_empty()) {
        add(found, &spec.replace(prefix, ""), row.get(description).map_or("", String::as_str));
    }
    columns
}

/// `` `/design hooks <on|off>` manages the detector `` offers `hooks`, described by what follows it.
fn invocations(found: &mut Vec<Subcommand>, line: &str, prefix: &str) {
    let parts: Vec<&str> = line.split('`').collect();
    let mut offset = 0;
    for (index, part) in parts.iter().enumerate() {
        offset += part.len() + 1;
        let closed = index % 2 == 1 && index + 1 < parts.len();
        let Some(spec) = part.strip_prefix(prefix).filter(|_| closed) else { continue };
        let after = plain(line.get(offset.min(line.len())..).unwrap_or_default());
        let description = after.strip_prefix([':', '.', ',', ';', '-']).map_or(after.as_str(), str::trim_start);
        add(found, spec, description);
    }
}

/// One choice (`audit [target]`: the name, then its usage); a later mention fills in what an earlier one lacked.
fn add(found: &mut Vec<Subcommand>, spec: &str, description: &str) {
    let spec = plain(spec);
    let (name, usage) = match spec.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, Some(rest.trim_start()).filter(|rest| !rest.is_empty())),
        None => (spec.as_str(), None),
    };
    let mut chars = name.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphabetic()) || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')) {
        return;
    }
    match found.iter_mut().find(|known| known.name.eq_ignore_ascii_case(name)) {
        Some(known) => {
            known.name = name.into();
            if !description.is_empty() {
                known.description = description.into();
            }
            if let Some(usage) = usage {
                known.usage = Some(usage.into());
            }
        }
        None => found.push(Subcommand { name: name.into(), description: description.into(), usage: usage.map(str::to_string) }),
    }
}

/// A code fence's marker and length, indented at most three spaces.
fn fence_marker(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start();
    let marker = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let length = trimmed.chars().take_while(|c| *c == marker).count();
    (line.len() - trimmed.len() <= 3 && length >= 3).then_some((marker, length))
}

/// A table row's cells as plain text; a pipe inside code or escaped stays in its cell.
fn cells(line: &str) -> Vec<String> {
    let mut protected = String::new();
    let mut in_code = false;
    let mut chars = line.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        match c {
            '`' if in_code || line[at + 1..].contains('`') => {
                in_code = !in_code;
                protected.push(c);
            }
            '\\' if chars.peek().is_some_and(|(_, next)| *next == '|') => {
                chars.next();
                protected.push('\0');
            }
            '|' if in_code => protected.push('\0'),
            _ => protected.push(c),
        }
    }
    let trimmed = protected.trim();
    let trimmed = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let trimmed = trimmed.strip_suffix('|').unwrap_or(trimmed);
    trimmed.split('|').map(|cell| plain(&cell.replace('\0', "|"))).collect()
}

/// Markdown links become their text, and code and emphasis marks go.
fn plain(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let link = after.find(']').filter(|close| *close > 0 && after[close + 1..].starts_with('(')).and_then(|close| after[close + 2..].find(')').map(|end| (close, close + 2 + end + 1)));
        match link {
            Some((close, end)) => {
                out.push_str(&rest[..open]);
                out.push_str(&after[..close]);
                rest = &after[end..];
            }
            None => {
                out.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.replace(['`', '*'], "").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(name: &str, description: &str, usage: Option<&str>) -> Subcommand {
        Subcommand { name: name.into(), description: description.into(), usage: usage.map(str::to_string) }
    }

    #[test]
    fn hints_command_tables_and_invocations_become_choices_with_help() {
        let mut lines = vec![
            "| Command | Category | Description | Reference |".to_string(),
            "|---|---|---|---|".into(),
            "| `audit [target]` | Evaluate | Technical quality checks | [guide](audit.md) |".into(),
            "| `polish [target]` | Refine | Final quality pass | [guide](polish.md) |".into(),
        ];
        lines.extend((0..12).map(|i| format!("| action-{i} [target] | Refine | Action {i} | ref |")));
        lines.extend(["".into(), "**Doctor:** `/design doctor` reports outdated project artifacts.".into(), "**Hooks:** `/design hooks <on|off|status>` manages the detector.".into()]);
        let (usage, found) = skill_arguments("design", &lines.join("\n"), Some("[audit|polish] [target]"));
        assert_eq!(usage.as_deref(), Some("[audit|polish] [target]"));
        assert_eq!(found.len(), 16);
        assert_eq!(found[0], sub("audit", "Technical quality checks", Some("[target]")));
        assert_eq!(found[15], sub("hooks", "manages the detector.", Some("<on|off|status>")));
    }

    #[test]
    fn free_form_hints_and_fenced_or_foreign_examples_invent_nothing() {
        let content = ["```md", "| Command | Description |", "|---|---|", "| fake | Example |", "```", "~~~", "`/design hidden` fake", "~~~", "`/other foreign` example", "| Tool | Description |", "|---|---|", "| ignored | Description |"].join("\n");
        assert_eq!(skill_arguments("design", &content, Some("[target]")), (Some("[target]".into()), Vec::new()));
    }

    #[test]
    fn grouped_hint_alternatives_are_each_a_choice() {
        let names: Vec<String> = skill_arguments("design", "", Some("[shape · audit|critique · polish] [target]")).1.into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["shape", "audit", "critique", "polish"]);
    }

    #[test]
    fn a_table_cell_keeps_its_pipes_and_descriptions_read_as_plain_text() {
        let content = "| Command | Description |\n|---|---|\n| `hooks <on|off>` | **Toggle** the [hook](hook.md) |";
        assert_eq!(skill_arguments("design", content, None).1, [sub("hooks", "Toggle the hook", Some("<on|off>"))]);
    }

    #[test]
    fn a_command_names_the_one_skill_it_calls() {
        assert_eq!(referenced_skill(r#"Call skill({ name: "test-skill" }) and follow it for $ARGUMENTS."#).as_deref(), Some("test-skill"));
        assert_eq!(referenced_skill("skill({'name': 'a.b'})").as_deref(), Some("a.b"));
        assert_eq!(referenced_skill(r#"skill({ name: "a" }) then skill({ name: "b" })"#), None, "two skills, no inheritance");
        assert_eq!(referenced_skill(r#"myskill({ name: "a" })"#), None);
        assert_eq!(referenced_skill("Use the design skill."), None);
    }
}
