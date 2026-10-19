//! opencode's global config as drift.json fields. v1 and v2 keys may sit side by side in one file, as v2 allows;
//! for a value both set, v2's wins, and v2's permission rules come after v1's, so they win too.

use super::ordered::OcConfig;
use super::{SettingsReport, permissions, providers};
use drift_engine::store::Store;
use serde_json::{Map, Value, json};
use std::path::Path;

/// Keys read elsewhere: MCP servers by the shell, agents and commands as definition files, providers after the rest.
const ELSEWHERE: [&str; 9] = [
    "$schema",
    "mcp",
    "agent",
    "agents",
    "mode",
    "command",
    "commands",
    "provider",
    "providers",
];

/// A model as v1 (`provider/model`) and v2 (`provider/model#variant` or `{ providerID, model, variant }`) write it.
pub(super) struct Selection {
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) variant: Option<String>,
}

pub(super) fn selection(value: &Value) -> Option<Selection> {
    let Some(text) = value.as_str() else {
        return Some(Selection {
            provider: value["providerID"].as_str()?.into(),
            model: value["model"].as_str()?.into(),
            variant: value["variant"].as_str().map(String::from),
        });
    };

    let (reference, variant) = match text.split_once('#') {
        Some((reference, variant)) => (reference, Some(variant.to_string())),
        None => (text, None),
    };
    let (provider, model) = reference.split_once('/')?;

    Some(Selection {
        provider: provider.into(),
        model: model.into(),
        variant: variant.filter(|variant| !variant.is_empty()),
    })
}

/// Converts supported opencode keys to drift.json fields and reports every unsupported key by name.
pub(super) fn convert(config: &OcConfig, config_dir: &Path, report: &mut SettingsReport) -> Map<String, Value> {
    let mut file = Map::new();
    let mut rules = (Vec::new(), Vec::new());

    for (key, value) in config.value.as_object().into_iter().flatten() {
        match key.as_str() {
            key if ELSEWHERE.contains(&key) => {}
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
            "permission" => rules.0 = v1_permissions(config, report),
            "permissions" => rules.1 = permissions::from_list(value, "permissions", report),
            "plugin" | "plugins" => plugins(value, report),
            "snapshot" | "snapshots" if *value == false => report.setting(key, "Drift always keeps its undo history"),
            "snapshot" | "snapshots" => {}
            "compaction" => compaction(value, report),
            other => report.setting(other, "Drift has no setting for it"),
        }
    }

    write_providers(&config.value, &mut file, report);
    write_permissions(rules, &mut file);

    file
}

fn write_model(value: &Value, file: &mut Map<String, Value>, report: &mut SettingsReport) {
    let Some(selected) = selection(value) else {
        report.setting("model", "not written as provider/model");
        return;
    };

    file.insert(
        "model".into(),
        json!({ "provider": selected.provider, "model": selected.model }),
    );
    if selected.variant.is_some() {
        report.setting("model.variant", "drift.json names a model without its level");
    }
}

fn write_agent(value: &Value, file: &mut Map<String, Value>, report: &mut SettingsReport) {
    match value.as_str() {
        Some(agent) => {
            file.insert("defaultAgent".into(), json!(agent));
        }
        None => report.setting("default_agent", "not an agent's name"),
    }
}

fn v1_permissions(config: &OcConfig, report: &mut SettingsReport) -> Vec<Value> {
    match &config.permission {
        Some(kinds) => permissions::from_kinds(kinds, "permission", report),
        None => {
            report.setting("permission", "neither a decision nor a map of them");
            Vec::new()
        }
    }
}

/// v1's rules then v2's, reversed: opencode uses the last matching rule and drift.json the first.
/// A `*` entry written before a tool-specific entry therefore still loses to that later entry.
fn write_permissions((v1, v2): (Vec<Value>, Vec<Value>), file: &mut Map<String, Value>) {
    let mut rules: Vec<Value> = v1.into_iter().chain(v2).collect();
    rules.reverse();

    if !rules.is_empty() {
        file.insert("permissions".into(), Value::Array(rules));
    }
}

fn write_providers(config: &Value, file: &mut Map<String, Value>, report: &mut SettingsReport) {
    let providers = providers::convert(config, report);
    if !providers.is_empty() {
        file.insert("providers".into(), Value::Object(providers));
    }
}

/// v1 lists a plugin as its package or `[package, options]`, v2 as its package or `{ package, options }`.
fn plugins(value: &Value, report: &mut SettingsReport) {
    for plugin in value.as_array().into_iter().flatten() {
        let Some(name) = plugin.as_str().or(plugin["package"].as_str()).or(plugin[0].as_str()) else {
            continue;
        };
        report.left(
            name,
            format!("plugin {name}: opencode plugins are JavaScript and Drift runs none"),
            |left| &mut left.plugins,
        );
    }
}

/// `auto` becomes Drift's own switch ([`compaction_switch`]); the token budgets have no Drift setting.
fn compaction(value: &Value, report: &mut SettingsReport) {
    let Some(fields) = value.as_object() else {
        report.setting("compaction", "Drift has no setting for it");
        return;
    };

    for key in fields.keys() {
        match key.as_str() {
            "auto" => {}
            "keep" | "preserve_recent_tokens" => report.setting(
                "compaction.keep.tokens",
                "Drift sizes the history it keeps to each model",
            ),
            other => report.setting(&format!("compaction.{other}"), "Drift has no setting for it"),
        }
    }
}

/// Sets Drift's automatic compaction to opencode's `compaction.auto` unless the user has set it in Drift.
pub(super) fn compaction_switch(store: &Store, config: &OcConfig, report: &mut SettingsReport) -> rusqlite::Result<()> {
    use drift_engine::session::compaction::AUTO_COMPACT_KEY;

    let Some(auto) = config.value["compaction"]["auto"].as_bool() else {
        return Ok(());
    };
    if store.setting::<bool>(AUTO_COMPACT_KEY)?.is_some() {
        report
            .skipped
            .push("config compaction.auto: you already set automatic compaction in Drift, kept".into());
        return Ok(());
    }

    store.set_setting(AUTO_COMPACT_KEY, &auto)
}

/// Makes relative instruction paths absolute against opencode's config directory for use from ~/.config/drift.
/// Absolute paths and ~/ paths keep their spelling.
fn instruction(path: &str, config_dir: &Path) -> String {
    if path.starts_with("~/") || Path::new(path).is_absolute() {
        return path.into();
    }

    config_dir.join(path).to_string_lossy().replace('\\', "/")
}
