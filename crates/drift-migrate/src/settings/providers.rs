//! opencode's providers as drift.json `providers`, which Drift reads from the user's own file only.
//! v1's `provider` points one with `api` or `options.baseURL`, v2's `providers` with `settings.baseURL`; v2's wins for an id in both.

use super::SettingsReport;
use serde_json::{Map, Value};

/// Where each shape keeps a provider's request settings and a model's input kinds.
struct Shape {
    v1: bool,
    settings: &'static str,
    inputs: &'static str,
}

const V1: Shape = Shape {
    v1: true,
    settings: "options",
    inputs: "modalities",
};
const V2: Shape = Shape {
    v1: false,
    settings: "settings",
    inputs: "capabilities",
};

pub(super) fn convert(config: &Value, report: &mut SettingsReport) -> Map<String, Value> {
    let native = config["providers"].as_object();
    let legacy = config["provider"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(id, _)| native.is_none_or(|native| !native.contains_key(*id)))
        .map(|(id, entry)| (id, entry, &V1, format!("provider.{id}")));
    let current = native
        .into_iter()
        .flatten()
        .map(|(id, entry)| (id, entry, &V2, format!("providers.{id}")));

    let mut providers = Map::new();
    for (id, entry, shape, at) in legacy.chain(current) {
        let provider = provider(entry, shape, &at, report);
        if !provider.is_empty() {
            providers.insert(id.clone(), Value::Object(provider));
        }
    }

    providers
}

/// Name, address, key variable and models; whatever else a provider sets is reported by name.
fn provider(entry: &Value, shape: &Shape, at: &str, report: &mut SettingsReport) -> Map<String, Value> {
    let mut provider = Map::new();

    for (key, value) in entry.as_object().into_iter().flatten() {
        match key.as_str() {
            "name" => {
                provider.insert("name".into(), value.clone());
            }
            "api" if shape.v1 => {
                provider.insert("baseUrl".into(), value.clone());
            }
            "env" => {
                if let Some(name) = value[0].as_str() {
                    provider.entry("apiKeyEnv").or_insert(name.into());
                }
            }
            "models" => {
                provider.insert("models".into(), Value::Object(models(value, shape, at, report)));
            }
            key if key == shape.settings => settings(value, &mut provider, &format!("{at}.{key}"), report),
            other => report.setting(&format!("{at}.{other}"), "Drift has no setting for it"),
        }
    }

    provider
}

/// A key written as `{env:NAME}` is read from that variable; a key written out is left to Drift's sign-ins.
fn settings(value: &Value, provider: &mut Map<String, Value>, at: &str, report: &mut SettingsReport) {
    for (key, value) in value.as_object().into_iter().flatten() {
        let variable = value
            .as_str()
            .and_then(|text| text.strip_prefix("{env:"))
            .and_then(|text| text.strip_suffix('}'));
        match (key.as_str(), variable) {
            ("baseURL", _) => {
                provider.entry("baseUrl").or_insert(value.clone());
            }
            ("apiKey", Some(name)) => {
                provider.insert("apiKeyEnv".into(), name.into());
            }
            ("apiKey", None) => report.setting(
                &format!("{at}.apiKey"),
                "Drift keeps keys with its sign-ins; add this one in Settings",
            ),
            (other, _) => report.setting(&format!("{at}.{other}"), "Drift has no setting for it"),
        }
    }
}

fn models(value: &Value, shape: &Shape, at: &str, report: &mut SettingsReport) -> Map<String, Value> {
    let mut models = Map::new();

    for (id, entry) in value.as_object().into_iter().flatten() {
        let at = format!("{at}.models.{id}");
        let mut model = Map::new();
        for (key, value) in entry.as_object().into_iter().flatten() {
            match key.as_str() {
                "name" => {
                    model.insert("name".into(), value.clone());
                }
                "limit" => limits(value, &mut model, &at, report),
                key if key == shape.inputs => {
                    let inputs = value["input"].as_array().into_iter().flatten();
                    if inputs.filter_map(Value::as_str).any(|input| input == "image") {
                        model.insert("images".into(), true.into());
                    }
                }
                "id" | "modelID" if value.as_str() == Some(id) => {}
                other => report.setting(&format!("{at}.{other}"), "Drift has no setting for it"),
            }
        }
        models.insert(id.clone(), Value::Object(model));
    }

    models
}

fn limits(value: &Value, model: &mut Map<String, Value>, at: &str, report: &mut SettingsReport) {
    for (key, value) in value.as_object().into_iter().flatten() {
        match key.as_str() {
            "context" | "output" => {
                model.insert(key.clone(), value.clone());
            }
            other => report.setting(&format!("{at}.limit.{other}"), "Drift has no setting for it"),
        }
    }
}
