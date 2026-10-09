use super::SettingsReport;
use serde::Deserialize;
use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};
use serde_json::{Map, Value, json};
use std::marker::PhantomData;
use std::path::Path;

/// opencode's global config, preserving the written order of its permission section.
/// opencode uses the last matching pattern, while an ordinary JSON object forgets that order.
pub struct OcConfig {
    pub value: Value,
    /// Ordered permission entries, or None when the value is neither a decision nor a map of decisions.
    permission: Option<Vec<(String, Setting)>>,
}

#[derive(Deserialize)]
struct Root {
    #[serde(default)]
    permission: Option<Permission>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Permission {
    One(String),
    Kinds(Entries<Setting>),
    Other(IgnoredAny),
}

/// One permission kind's decision, or its patterns in written order.
#[derive(Deserialize)]
#[serde(untagged)]
enum Setting {
    One(String),
    Patterns(Entries<Value>),
    Other(IgnoredAny),
}

/// A JSON object deserialized as entries in their written order.
struct Entries<T>(Vec<(String, T)>);
struct EntryVisitor<T>(PhantomData<T>);

impl<'de, T: Deserialize<'de>> Visitor<'de> for EntryVisitor<T> {
    type Value = Entries<T>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("an object")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Entries<T>, M::Error> {
        let mut entries = Vec::new();
        while let Some(entry) = map.next_entry()? {
            entries.push(entry);
        }

        Ok(Entries(entries))
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Entries<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(EntryVisitor(PhantomData))
    }
}

impl OcConfig {
    /// Parses config text whose comments have already been stripped.
    pub fn parse(text: &str) -> Option<Self> {
        let value = serde_json::from_str(text).ok()?;
        let permission = match serde_json::from_str::<Root>(text).ok().and_then(|root| root.permission) {
            Some(Permission::Kinds(kinds)) => Some(kinds.0),
            Some(Permission::One(decision)) => Some(vec![("*".into(), Setting::One(decision))]),
            Some(Permission::Other(_)) | None => None,
        };

        Some(Self { value, permission })
    }
}

/// Converts supported opencode keys to drift.json fields and reports every unsupported key by name.
pub(super) fn convert(config: &OcConfig, config_dir: &Path, report: &mut SettingsReport) -> Map<String, Value> {
    let mut file = Map::new();

    for (key, value) in config.value.as_object().into_iter().flatten() {
        match key.as_str() {
            "$schema" | "mcp" => {}
            "model" => write_model(value, &mut file, report),
            "default_agent" => write_agent(value, &mut file, report),
            "instructions" => {
                let paths = value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|path| json!(instruction(path, config_dir)))
                    .collect();
                file.insert("instructions".into(), Value::Array(paths));
            }
            "permission" => write_permissions(config, &mut file, report),
            "plugin" => {
                for plugin in value.as_array().into_iter().flatten().filter_map(Value::as_str) {
                    report.left(
                        plugin,
                        format!("plugin {plugin}: opencode plugins are JavaScript and Drift runs none"),
                        |left| &mut left.plugins,
                    );
                }
            }
            other => report.left(other, format!("config {other}: Drift has no setting for it"), |left| {
                &mut left.settings
            }),
        }
    }

    file
}

fn write_model(value: &Value, file: &mut Map<String, Value>, report: &mut SettingsReport) {
    match value.as_str().and_then(|model| model.split_once('/')) {
        Some((provider, model)) => {
            file.insert("model".into(), json!({ "provider": provider, "model": model }));
        }
        None => report.left("model", "config model: not written as provider/model".into(), |left| {
            &mut left.settings
        }),
    }
}

fn write_agent(value: &Value, file: &mut Map<String, Value>, report: &mut SettingsReport) {
    match value.as_str() {
        Some(agent) => {
            file.insert("defaultAgent".into(), json!(agent));
        }
        None => report.left(
            "default_agent",
            "config default_agent: not an agent's name".into(),
            |left| &mut left.settings,
        ),
    }
}

fn write_permissions(config: &OcConfig, file: &mut Map<String, Value>, report: &mut SettingsReport) {
    let Some(kinds) = &config.permission else {
        report.left(
            "permission",
            "config permission: neither a decision nor a map of them".into(),
            |left| &mut left.settings,
        );
        return;
    };

    let rules = permissions(kinds, report);
    if !rules.is_empty() {
        file.insert("permissions".into(), Value::Array(rules));
    }
}

/// Makes relative instruction paths absolute against opencode's config directory for use from ~/.config/drift.
/// Absolute paths and ~/ paths keep their spelling.
fn instruction(path: &str, config_dir: &Path) -> String {
    if path.starts_with("~/") || Path::new(path).is_absolute() {
        return path.into();
    }

    config_dir.join(path).to_string_lossy().replace('\\', "/")
}

/// Converts opencode permission decisions and ordered patterns into Drift rules, including `*` for every tool.
/// Reverses the full list because opencode uses the last match while Drift uses the first.
/// A `*` entry written before a tool-specific entry therefore still loses to that later entry.
fn permissions(kinds: &[(String, Setting)], report: &mut SettingsReport) -> Vec<Value> {
    let mut rules = Vec::new();

    for (kind, setting) in kinds {
        if !matches!(kind.as_str(), "*" | "read" | "edit" | "bash" | "webfetch") {
            report.left(
                &format!("permission.{kind}"),
                format!("config permission.{kind}: Drift has no such permission"),
                |left| &mut left.settings,
            );
            continue;
        }

        let patterns: Vec<(&str, Value)> = match setting {
            Setting::One(decision) => vec![("*", Value::String(decision.clone()))],
            Setting::Patterns(entries) => entries
                .0
                .iter()
                .map(|(pattern, decision)| (pattern.as_str(), decision.clone()))
                .collect(),
            Setting::Other(_) => Vec::new(),
        };

        for (pattern, decision) in patterns {
            match decision
                .as_str()
                .filter(|decision| matches!(*decision, "allow" | "ask" | "deny"))
            {
                Some(decision) => rules.push(json!({ "kind": kind, "pattern": pattern, "decision": decision })),
                None => report.left(
                    &format!("permission.{kind} {pattern}"),
                    format!("config permission.{kind} {pattern}: not allow, ask or deny"),
                    |left| &mut left.settings,
                ),
            }
        }
    }

    // Drift uses the first matching rule; reversing preserves opencode's last-match precedence.
    rules.reverse();
    rules
}
