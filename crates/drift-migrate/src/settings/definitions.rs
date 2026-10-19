//! opencode's agents and commands defined in its config, written as Drift's Markdown definitions in ~/.config/drift.
//! v1's `agent`, `mode` and `command` and v2's `agents` and `commands` may sit side by side; for a name in more than one,
//! v2's wins, then `mode`'s, as v2 merges them.

use super::config::selection;
use super::ordered::OcConfig;
use super::{SettingsReport, files, permissions, servers};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// A definition and the section it was found in.
struct Found<'a> {
    section: &'static str,
    name: &'a str,
    value: &'a Value,
}

impl Found<'_> {
    fn at(&self) -> String {
        format!("{}.{}", self.section, self.name)
    }

    fn v2(&self) -> bool {
        matches!(self.section, "agents" | "commands")
    }
}

/// Front matter lines and a body, as Drift reads an agent or command file.
#[derive(Default)]
struct Definition {
    head: Vec<String>,
    body: String,
}

impl Definition {
    fn field(&mut self, key: &str, value: &str) {
        self.head.push(format!("{key}: {}", value.replace(['\r', '\n'], " ")));
    }

    fn text(&self) -> String {
        let body = self.body.trim();
        match self.head.is_empty() {
            true => format!("{body}\n"),
            false => format!("---\n{}\n---\n\n{body}\n", self.head.join("\n")),
        }
    }
}

/// Paths and inputs shared by every definition of one import.
struct Context<'a> {
    config: &'a OcConfig,
    config_dir: &'a Path,
    drift_dir: &'a Path,
}

pub(super) fn write(config: &OcConfig, config_dir: &Path, drift_dir: &Path, report: &mut SettingsReport) {
    let context = Context {
        config,
        config_dir,
        drift_dir,
    };

    for found in collect(&config.value, &["agent", "mode", "agents"]) {
        let definition = agent(&found, &context, report);
        save(&context, "agents", &found, definition, report);
    }
    for found in collect(&config.value, &["command", "commands"]) {
        let definition = command(&found, &context, report);
        save(&context, "commands", &found, definition, report);
    }
}

/// Every definition by name; a later section replaces an earlier one's.
fn collect<'a>(config: &'a Value, sections: &[&'static str]) -> Vec<Found<'a>> {
    let mut found = BTreeMap::new();
    for section in sections {
        for (name, value) in config[*section].as_object().into_iter().flatten() {
            found.insert(name.as_str(), Found { section, name, value });
        }
    }

    found.into_values().collect()
}

fn save(
    context: &Context<'_>,
    folder: &str,
    found: &Found<'_>,
    definition: Option<Definition>,
    report: &mut SettingsReport,
) {
    let Some(definition) = definition else { return };
    let name = found.name;
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\', ':']) {
        report.setting(&found.at(), "its name cannot be a file name");
        return;
    }

    let path = context.drift_dir.join(folder).join(format!("{name}.md"));
    files::write_new(&path, &definition.text(), &format!("{folder}/{name}.md"), report);
}

fn agent(found: &Found<'_>, context: &Context<'_>, report: &mut SettingsReport) -> Option<Definition> {
    let Some(fields) = found.value.as_object() else {
        report.setting(&found.at(), "not an agent definition");
        return None;
    };

    let mut definition = Definition::default();
    if found.section == "mode" {
        definition.field("mode", "primary");
    }
    for (key, value) in fields {
        agent_field(&mut definition, found, (key, value), context, report)?;
    }

    Some(definition)
}

/// One field of an agent; None when the definition cannot be written at all.
fn agent_field(
    definition: &mut Definition,
    found: &Found<'_>,
    (key, value): (&str, &Value),
    context: &Context<'_>,
    report: &mut SettingsReport,
) -> Option<()> {
    let at = found.at();
    match (key, found.v2()) {
        ("description", _) => text(definition, key, value, &at, report),
        ("mode", _) if found.section == "mode" => {}
        ("mode", _) => text(definition, key, value, &at, report),
        ("hidden", _) => flag(definition, "hidden", value),
        ("disable", false) | ("disabled", true) => flag(definition, "disable", value),
        ("prompt", false) | ("system", true) => definition.body = body(value, &at, context, report)?,
        ("model", _) => model(definition, value, &at, report, true),
        ("variant", false) => text(definition, key, value, &at, report),
        ("permission", false) | ("permissions", true) => rules(definition, found, value, context.config, report),
        ("tools", false) => tools(definition, value, &at, report),
        _ => report.setting(&format!("{at}.{key}"), "Drift has no setting for it"),
    }

    Some(())
}

fn command(found: &Found<'_>, context: &Context<'_>, report: &mut SettingsReport) -> Option<Definition> {
    let at = found.at();
    let Some(template) = found.value.get("template") else {
        report.setting(&at, "it has no template");
        return None;
    };

    let mut definition = Definition {
        body: body(template, &at, context, report)?,
        ..Definition::default()
    };
    for (key, value) in found.value.as_object().into_iter().flatten() {
        match (key.as_str(), found.v2()) {
            ("template", _) => {}
            ("description" | "agent", _) => text(&mut definition, key, value, &at, report),
            ("model", _) => model(&mut definition, value, &at, report, false),
            ("subtask", _) | ("subagent", true) => {
                if let Some(on) = value.as_bool() {
                    definition.field("subtask", &on.to_string());
                }
            }
            _ => report.setting(&format!("{at}.{key}"), "Drift has no setting for it"),
        }
    }

    Some(definition)
}

fn text(definition: &mut Definition, key: &str, value: &Value, at: &str, report: &mut SettingsReport) {
    match value.as_str() {
        Some("") => {}
        Some(text) => definition.field(key, text),
        None => report.setting(&format!("{at}.{key}"), "not text"),
    }
}

fn flag(definition: &mut Definition, key: &str, value: &Value) {
    if *value == true {
        definition.field(key, "true");
    }
}

/// A prompt or template, with `{file:...}` and `{env:...}` read as opencode reads them.
fn body(value: &Value, at: &str, context: &Context<'_>, report: &mut SettingsReport) -> Option<String> {
    let Some(text) = value.as_str() else {
        report.setting(at, "its prompt is not text");
        return None;
    };

    match servers::substitute(text, context.config_dir) {
        Ok(text) => Some(text),
        Err(error) => {
            report.setting(at, &error.to_string());
            None
        }
    }
}

/// An agent keeps a model's level as its `variant`; a command has no level of its own, so that part is reported.
fn model(definition: &mut Definition, value: &Value, at: &str, report: &mut SettingsReport, levels: bool) {
    let Some(selected) = selection(value) else {
        report.setting(&format!("{at}.model"), "not written as provider/model");
        return;
    };

    definition.field("model", &format!("{}/{}", selected.provider, selected.model));
    match (selected.variant, levels) {
        (Some(variant), true) => definition.field("variant", &variant),
        (Some(_), false) => report.setting(&format!("{at}.model.variant"), "a Drift command has no model level"),
        (None, _) => {}
    }
}

/// An agent's rules stay in written order: Drift reads an agent's rules as opencode does, the last match winning.
fn rules(
    definition: &mut Definition,
    found: &Found<'_>,
    value: &Value,
    config: &OcConfig,
    report: &mut SettingsReport,
) {
    let at = found.at();
    let rules = match found.v2() {
        true => permissions::from_list(value, &format!("{at}.permissions"), report),
        false => match config.agent_permission(found.section, found.name) {
            Some(Some(kinds)) => permissions::from_kinds(kinds, &format!("{at}.permission"), report),
            _ => {
                report.setting(&format!("{at}.permission"), "neither a decision nor a map of them");
                Vec::new()
            }
        },
    };

    if !rules.is_empty() {
        definition.field("permissions", &Value::Array(rules).to_string());
    }
}

/// v1's `{ tool: false }` map, which Drift's agent files read the same way; `true` changes nothing in either.
fn tools(definition: &mut Definition, value: &Value, at: &str, report: &mut SettingsReport) {
    let Some(tools) = value.as_object() else {
        report.setting(&format!("{at}.tools"), "not a map of tools");
        return;
    };

    let off: Vec<&String> = tools
        .iter()
        .filter(|(_, on)| **on == false)
        .map(|(name, _)| name)
        .collect();
    if !off.is_empty() {
        definition.head.push("tools:".into());
        definition
            .head
            .extend(off.into_iter().map(|name| format!("  {name}: false")));
    }
}
