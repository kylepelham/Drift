use super::{Skill, arguments};
use crate::session::types::ModelRef;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Command {
    pub name: String,
    pub description: String,
    /// The prompt; see [`Command::expand`] for how what follows the command fills it.
    pub template: String,
    /// For an MCP server's prompt (`server:prompt`): the server that fills it; the template is unused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// The prompt's arguments in order, which what follows the command fills word by word.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtask: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
    /// How to call it, from its skill's `argument-hint` (`[audit|polish] [target]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<String>,
    /// The choices its skill documents, which the slash menu offers (`config::arguments`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subcommands: Vec<arguments::Subcommand>,
}

impl Command {
    pub fn new(name: String, description: String, template: String) -> Self {
        Self {
            name,
            description,
            template,
            server: None,
            arguments: Vec::new(),
            agent: None,
            model: None,
            subtask: None,
            skill: None,
            usage: None,
            subcommands: Vec::new(),
        }
    }

    /// Takes the usage and choices `skill` documents, keeping its own template and settings.
    pub(super) fn document(&mut self, skill: &Skill) {
        (self.usage, self.subcommands) =
            arguments::skill_arguments(&skill.name, &skill.instructions, skill.argument_hint.as_deref());
    }

    /// `arguments` split for this command's named arguments: one word each, the last taking the rest.
    pub fn named_arguments(&self, arguments: &str) -> serde_json::Map<String, serde_json::Value> {
        let words = split_arguments(arguments);
        let last = self.arguments.len().saturating_sub(1);

        self.arguments
            .iter()
            .enumerate()
            .filter_map(|(index, name)| {
                let value = if index == last {
                    words.get(index..).map(|rest| rest.join(" "))
                } else {
                    words.get(index).cloned()
                };

                value
                    .filter(|value| !value.is_empty())
                    .map(|value| (name.clone(), serde_json::Value::String(value)))
            })
            .collect()
    }

    /// The prompt for `arguments`: `$ARGUMENTS` is all of them, `$1`, `$2`, ... one argument each (quotes
    /// keep spaces, as in a shell) with the highest taking the rest, and a template that names none gets
    /// them appended so nothing typed is lost.
    pub fn expand(&self, arguments: &str) -> String {
        let arguments = arguments.trim();
        let words = split_arguments(arguments);
        let highest = highest_placeholder(&self.template);
        let mut text = self.template.replace("$ARGUMENTS", arguments);

        // Replace high-numbered placeholders first so $1 does not consume the start of $10.
        for number in (1..=highest).rev() {
            let word = if number == highest {
                words.get(number - 1..).map(|rest| rest.join(" ")).unwrap_or_default()
            } else {
                words.get(number - 1).cloned().unwrap_or_default()
            };
            text = text.replace(&format!("${number}"), &word);
        }

        if highest == 0 && !self.template.contains("$ARGUMENTS") && !arguments.is_empty() {
            text = format!("{}\n\n{arguments}", text.trim_end());
        }

        text
    }
}

/// The highest `$N` a template names, 0 when it names none.
pub fn highest_placeholder(template: &str) -> usize {
    template
        .split('$')
        .skip(1)
        .filter_map(|rest| {
            rest.split(|character: char| !character.is_ascii_digit())
                .next()?
                .parse()
                .ok()
        })
        .max()
        .unwrap_or(0)
}

/// What follows a command, split as a shell splits words: quotes (single or double) keep spaces and are dropped.
pub fn split_arguments(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let (mut quote, mut started) = (None, false);

    for character in text.chars() {
        match quote {
            Some(open) if character == open => quote = None,
            Some(_) => current.push(character),
            None if character == '"' || character == '\'' => (quote, started) = (Some(character), true),
            None if character.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                }
                started = false;
            }
            None => {
                current.push(character);
                started = true;
            }
        }
    }
    if started {
        words.push(current);
    }

    words
}
