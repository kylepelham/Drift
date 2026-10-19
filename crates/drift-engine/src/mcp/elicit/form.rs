//! An elicitation form as question-card questions, and the card's answers as the content the schema asks for.
//! Each property is one question: a choice becomes options, a boolean yes or no, the rest typed text.

use serde_json::{Map, Value};

use crate::question::{Option_, Question};

pub(super) const DONE: &str = "Done";
const YES: &str = "Yes";
const NO: &str = "No";
/// Offered on an optional question with fixed options, so it can be left out.
const SKIP: &str = "Skip";

pub(super) struct Field {
    name: String,
    label: String,
    description: String,
    required: bool,
    kind: Kind,
}

enum Kind {
    Text {
        min: Option<u64>,
        max: Option<u64>,
        format: Option<String>,
    },
    Number {
        integer: bool,
        min: Option<f64>,
        max: Option<f64>,
    },
    Boolean,
    /// Values with the labels shown for them.
    Choice {
        options: Vec<(Value, String)>,
        multiple: bool,
    },
}

/// The schema's properties in the order the server wrote them, when it is known.
pub(super) fn fields(schema: &Value, order: Option<Vec<String>>) -> Vec<Field> {
    let empty = Map::new();
    let properties = schema["properties"].as_object().unwrap_or(&empty);
    let required: Vec<&str> = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let names = order.unwrap_or_else(|| properties.keys().cloned().collect());

    names
        .into_iter()
        .filter_map(|name| {
            let property = properties.get(&name)?;
            Some(Field {
                label: property["title"].as_str().unwrap_or(&name).to_string(),
                description: property["description"].as_str().unwrap_or_default().to_string(),
                required: required.contains(&name.as_str()),
                kind: kind(property),
                name,
            })
        })
        .collect()
}

fn kind(property: &Value) -> Kind {
    if property["type"] == "array" {
        return Kind::Choice {
            options: choices(&property["items"]),
            multiple: true,
        };
    }
    let options = choices(property);
    if !options.is_empty() {
        return Kind::Choice {
            options,
            multiple: false,
        };
    }

    match property["type"].as_str() {
        Some("boolean") => Kind::Boolean,
        Some(kind @ ("number" | "integer")) => Kind::Number {
            integer: kind == "integer",
            min: property["minimum"].as_f64(),
            max: property["maximum"].as_f64(),
        },
        _ => Kind::Text {
            min: property["minLength"].as_u64(),
            max: property["maxLength"].as_u64(),
            format: property["format"].as_str().map(str::to_string),
        },
    }
}

/// `enum` (with `enumNames` as its labels), or `oneOf` / `anyOf` entries of `const` and `title`.
fn choices(schema: &Value) -> Vec<(Value, String)> {
    if let Some(values) = schema["enum"].as_array() {
        let names = schema["enumNames"].as_array();
        return values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let name = names.and_then(|names| names.get(index)).and_then(Value::as_str);
                (value.clone(), name.map_or_else(|| shown(value), str::to_string))
            })
            .collect();
    }
    let titled = schema["oneOf"].as_array().or_else(|| schema["anyOf"].as_array());

    titled
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let value = entry.get("const")?.clone();
            let label = entry["title"].as_str().map_or_else(|| shown(&value), str::to_string);
            Some((value, label))
        })
        .collect()
}

fn shown(value: &Value) -> String {
    value.as_str().map_or_else(|| value.to_string(), str::to_string)
}

fn option(label: &str) -> Option_ {
    Option_ {
        label: label.to_string(),
        description: String::new(),
    }
}

/// One question per field, each headed by the server's name; a form with no fields asks to go on.
pub(super) fn questions(fields: &[Field], server: &str) -> Vec<Question> {
    if fields.is_empty() {
        return vec![Question {
            question: String::new(),
            header: server.to_string(),
            options: vec![option(DONE)],
            multiple: false,
            custom: false,
        }];
    }

    fields.iter().map(|field| question(field, server)).collect()
}

fn question(field: &Field, server: &str) -> Question {
    let optional = if field.required { "" } else { " (optional)" };
    let text = match field.description.as_str() {
        "" => format!("{}{optional}", field.label),
        description => format!("{}{optional}: {description}", field.label),
    };
    let skip = (!field.required).then(|| option(SKIP));
    let (options, multiple, custom) = match &field.kind {
        Kind::Boolean => (
            [option(YES), option(NO)].into_iter().chain(skip).collect(),
            false,
            false,
        ),
        Kind::Choice { options, multiple } => {
            let listed = options.iter().map(|(_, label)| option(label));
            let skip = skip.filter(|_| !multiple);
            (listed.chain(skip).collect(), *multiple, false)
        }
        Kind::Text { .. } | Kind::Number { .. } => (Vec::new(), false, true),
    };

    Question {
        question: text,
        header: server.to_string(),
        options,
        multiple,
        custom,
    }
}

/// The answers as the schema's object, or the first problem to show on the card asked again.
pub(super) fn content(fields: &[Field], answers: &[Vec<String>]) -> Result<Value, String> {
    let mut content = Map::new();
    for (index, field) in fields.iter().enumerate() {
        let given: Vec<&str> = answers
            .get(index)
            .into_iter()
            .flatten()
            .map(|answer| answer.trim())
            .filter(|answer| !answer.is_empty() && *answer != SKIP)
            .collect();
        if given.is_empty() {
            if field.required {
                return Err(format!("{} needs an answer.", field.label));
            }
            continue;
        }
        let value = read(field, &given).map_err(|why| format!("{}: {why}", field.label))?;
        content.insert(field.name.clone(), value);
    }

    Ok(Value::Object(content))
}

fn read(field: &Field, given: &[&str]) -> Result<Value, String> {
    match &field.kind {
        Kind::Boolean => Ok(Value::Bool(given[0] == YES)),
        Kind::Choice { options, multiple } => {
            let picked: Vec<Value> = given
                .iter()
                .map(|label| {
                    let found = options.iter().find(|(_, shown)| shown == label);
                    found
                        .map(|(value, _)| value.clone())
                        .ok_or_else(|| format!("{label} is not one of the choices"))
                })
                .collect::<Result<_, _>>()?;
            Ok(if *multiple {
                Value::Array(picked)
            } else {
                picked[0].clone()
            })
        }
        Kind::Number { integer, min, max } => number(given[0], *integer, *min, *max),
        Kind::Text { min, max, format } => text(given[0], *min, *max, format.as_deref()),
    }
}

fn number(answer: &str, integer: bool, min: Option<f64>, max: Option<f64>) -> Result<Value, String> {
    let parsed: f64 = answer.parse().map_err(|_| format!("{answer} is not a number"))?;
    if integer && parsed.fract() != 0.0 {
        return Err(format!("{answer} is not a whole number"));
    }
    if min.is_some_and(|min| parsed < min) || max.is_some_and(|max| parsed > max) {
        let bound = |bound: Option<f64>| bound.map_or_else(|| "any".to_string(), |bound| bound.to_string());
        return Err(format!("{answer} is outside {} to {}", bound(min), bound(max)));
    }

    Ok(if integer {
        Value::from(parsed as i64)
    } else {
        Value::from(parsed)
    })
}

fn text(answer: &str, min: Option<u64>, max: Option<u64>, format: Option<&str>) -> Result<Value, String> {
    let length = answer.chars().count() as u64;
    if min.is_some_and(|min| length < min) {
        return Err(format!("needs at least {} characters", min.unwrap_or_default()));
    }
    if max.is_some_and(|max| length > max) {
        return Err(format!("takes at most {} characters", max.unwrap_or_default()));
    }
    let fits = match format {
        Some("email") => answer
            .split_once('@')
            .is_some_and(|(name, host)| !name.is_empty() && host.contains('.')),
        Some("uri") => reqwest::Url::parse(answer).is_ok(),
        Some("date") => is_date(answer),
        _ => true,
    };
    if !fits {
        return Err(format!("{answer} is not a valid {}", format.unwrap_or_default()));
    }

    Ok(Value::String(answer.to_string()))
}

/// `YYYY-MM-DD`, as JSON Schema's `date` format writes it.
fn is_date(answer: &str) -> bool {
    let parts: Vec<&str> = answer.split('-').collect();
    let digits = |part: &str, length: usize| part.len() == length && part.bytes().all(|byte| byte.is_ascii_digit());

    parts.len() == 3 && digits(parts[0], 4) && digits(parts[1], 2) && digits(parts[2], 2)
}

#[cfg(test)]
mod tests;
