//! The settings a plugin declares (the `configurable` world): the form Settings draws, the defaults its config
//! falls back to, and the check a saved config must pass.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use utoipa::ToSchema;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    /// Where its value lives in the plugin's config object.
    pub key: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub kind: SettingKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SettingKind {
    Text {
        default: String,
        placeholder: String,
        multiline: bool,
        secret: bool,
    },
    Number {
        default: Option<f64>,
        min: Option<f64>,
        max: Option<f64>,
        integer: bool,
    },
    Toggle {
        default: bool,
    },
    Choice {
        default: String,
        choices: Vec<Choice>,
    },
    /// A list of strings, a row each.
    Items {
        default: Vec<String>,
        placeholder: String,
        secret: bool,
    },
    /// An object of strings, a row of two fields each.
    #[serde(rename_all = "camelCase")]
    Pairs {
        default: Vec<Pair>,
        key_label: String,
        value_label: String,
        secret_keys: bool,
        secret_values: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Pair {
    pub key: String,
    pub value: String,
}

impl SettingKind {
    /// The value an unset setting has; a number with no default has none.
    fn default_value(&self) -> Option<Value> {
        match self {
            Self::Text { default, .. } | Self::Choice { default, .. } => Some(Value::from(default.as_str())),
            Self::Number { default, .. } => default.map(Value::from),
            Self::Toggle { default } => Some(Value::Bool(*default)),
            Self::Items { default, .. } => Some(Value::from(default.clone())),
            Self::Pairs { default, .. } => Some(Value::Object(
                default
                    .iter()
                    .map(|pair| (pair.key.clone(), Value::from(pair.value.as_str())))
                    .collect(),
            )),
        }
    }

    /// Why `value` cannot be this setting's, if it cannot.
    fn problem(&self, value: &Value) -> Option<String> {
        match self {
            Self::Text { .. } => (!value.is_string()).then(|| "is text".into()),
            Self::Toggle { .. } => (!value.is_boolean()).then(|| "is on or off".into()),
            Self::Number { min, max, integer, .. } => number_problem(value, *min, *max, *integer),
            Self::Choice { choices, .. } => {
                let known = choices
                    .iter()
                    .any(|choice| value.as_str() == Some(choice.value.as_str()));
                (!known).then(|| "is one of the choices offered".into())
            }
            Self::Items { .. } => {
                let strings = value.as_array().is_some_and(|items| items.iter().all(Value::is_string));
                (!strings).then(|| "is a list of text".into())
            }
            Self::Pairs { .. } => {
                let strings = value
                    .as_object()
                    .is_some_and(|pairs| pairs.values().all(Value::is_string));
                (!strings).then(|| "is pairs of text".into())
            }
        }
    }
}

fn number_problem(value: &Value, min: Option<f64>, max: Option<f64>, integer: bool) -> Option<String> {
    let Some(number) = value.as_f64() else {
        return Some("is a number".into());
    };
    if integer && number.fract() != 0.0 {
        return Some("is a whole number".into());
    }
    if let Some(min) = min.filter(|min| number < *min) {
        return Some(format!("is at least {min}"));
    }
    if let Some(max) = max.filter(|max| number > *max) {
        return Some(format!("is at most {max}"));
    }

    None
}

/// The config a plugin reads: what the user set, and each declared setting's default where they set nothing.
pub fn with_defaults(settings: &[Setting], config: &Value) -> Value {
    let mut filled = config.as_object().cloned().unwrap_or_default();
    for setting in settings {
        if let Some(default) = setting
            .kind
            .default_value()
            .filter(|_| !filled.contains_key(&setting.key))
        {
            filled.insert(setting.key.clone(), default);
        }
    }

    Value::Object(filled)
}

/// Why a config cannot be saved for these settings, naming the first setting it gets wrong. Keys the plugin
/// does not declare are left alone.
pub fn problem(settings: &[Setting], config: &Value) -> Option<String> {
    let empty = Map::new();
    let values = config.as_object().unwrap_or(&empty);

    settings.iter().find_map(|setting| {
        let value = values.get(&setting.key)?;
        let problem = setting.kind.problem(value)?;
        Some(format!("{} {problem}", setting.label))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn setting(key: &str, kind: SettingKind) -> Setting {
        Setting {
            key: key.into(),
            label: key.to_uppercase(),
            description: String::new(),
            kind,
        }
    }

    fn declared() -> Vec<Setting> {
        vec![
            setting(
                "replacements",
                SettingKind::Pairs {
                    default: vec![Pair {
                        key: "a".into(),
                        value: "b".into(),
                    }],
                    key_label: "Secret".into(),
                    value_label: "Placeholder".into(),
                    secret_keys: true,
                    secret_values: false,
                },
            ),
            setting(
                "limit",
                SettingKind::Number {
                    default: None,
                    min: Some(1.0),
                    max: Some(10.0),
                    integer: true,
                },
            ),
            setting(
                "mode",
                SettingKind::Choice {
                    default: "fast".into(),
                    choices: vec![Choice {
                        value: "fast".into(),
                        label: "Fast".into(),
                    }],
                },
            ),
            setting(
                "paths",
                SettingKind::Items {
                    default: vec![],
                    placeholder: String::new(),
                    secret: false,
                },
            ),
        ]
    }

    #[test]
    fn unset_values_take_their_defaults_and_set_ones_stay() {
        let config = with_defaults(&declared(), &json!({ "mode": "fast", "extra": 1 }));

        assert_eq!(
            config,
            json!({ "replacements": { "a": "b" }, "mode": "fast", "paths": [], "extra": 1 }),
            "a number with no default stays unset"
        );
        assert_eq!(with_defaults(&[], &json!({ "x": 1 })), json!({ "x": 1 }));
    }

    #[test]
    fn a_saved_value_must_fit_its_setting() {
        let settings = declared();
        let fits = json!({ "replacements": { "k": "v" }, "limit": 3, "mode": "fast", "paths": ["a"], "other": [1] });
        assert_eq!(problem(&settings, &fits), None);

        for (config, said) in [
            (json!({ "replacements": { "k": 1 } }), "REPLACEMENTS is pairs of text"),
            (json!({ "limit": 2.5 }), "LIMIT is a whole number"),
            (json!({ "limit": 11 }), "LIMIT is at most 10"),
            (json!({ "limit": "3" }), "LIMIT is a number"),
            (json!({ "mode": "slow" }), "MODE is one of the choices offered"),
            (json!({ "paths": "a" }), "PATHS is a list of text"),
        ] {
            assert_eq!(problem(&settings, &config).as_deref(), Some(said));
        }
    }
}
