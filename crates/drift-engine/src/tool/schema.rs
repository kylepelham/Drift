//! A call's arguments checked against its tool's input schema before it runs, so a value of the wrong
//! type is refused with a reason instead of being read as missing. Only the keywords that decide
//! shape are checked (`type`, `required`, `properties`, `items`, `enum`); any other passes.

use serde_json::Value;

/// Every way `input` breaks `schema`, as sentences for the model; empty when it fits.
pub fn problems(schema: &Value, input: &Value) -> Vec<String> {
    let mut found = Vec::new();
    check(schema, input, "", &mut found);
    found
}

fn check(schema: &Value, value: &Value, at: &str, found: &mut Vec<String>) {
    let name = if at.is_empty() { "the arguments".to_string() } else { format!("`{at}`") };
    if let Some(expected) = types(schema).filter(|types| !types.iter().any(|t| is(value, t))) {
        found.push(format!("{name} must be {}, not {}", expected.join(" or "), kind(value)));
        return;
    }
    if let Some(choices) = schema["enum"].as_array().filter(|choices| !choices.contains(value)) {
        let listed: Vec<String> = choices.iter().map(Value::to_string).collect();
        found.push(format!("{name} must be one of {}", listed.join(", ")));
    }
    if let Value::Object(fields) = value {
        for missing in schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|key| !fields.contains_key(*key)) {
            found.push(format!("`{}` is required", path(at, missing)));
        }
        for (key, inner) in schema["properties"].as_object().into_iter().flatten() {
            if let Some(given) = fields.get(key).filter(|given| !given.is_null() || !optional_null(inner)) {
                check(inner, given, &path(at, key), found);
            }
        }
    }
    if let (Value::Array(items), Some(each)) = (value, schema.get("items").filter(|each| each.is_object())) {
        for (index, item) in items.iter().enumerate() {
            check(each, item, &format!("{at}[{index}]"), found);
        }
    }
}

/// A `null` for a property that does not allow it is read as left out, which models often send.
fn optional_null(schema: &Value) -> bool {
    types(schema).is_some_and(|types| !types.contains(&"null"))
}

fn types(schema: &Value) -> Option<Vec<&str>> {
    match &schema["type"] {
        Value::String(one) => Some(vec![one.as_str()]),
        Value::Array(many) => Some(many.iter().filter_map(Value::as_str).collect()),
        _ => None,
    }
}

fn is(value: &Value, kind: &str) -> bool {
    match kind {
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64() || value.as_f64().is_some_and(|n| n.fract() == 0.0),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "null" => value.is_null(),
        _ => true,
    }
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn path(at: &str, key: &str) -> String {
    if at.is_empty() { key.to_string() } else { format!("{at}.{key}") }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::problems;

    #[test]
    fn wrong_types_missing_fields_and_bad_choices_are_named() {
        let schema = json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "limit": { "type": "integer" },
                "format": { "type": "string", "enum": ["markdown", "text"] },
                "todos": { "type": "array", "items": { "type": "object", "properties": { "content": { "type": "string" } }, "required": ["content"] } }
            },
            "required": ["path"]
        });
        assert!(problems(&schema, &json!({ "path": "a", "limit": 20, "format": "text" })).is_empty());
        assert!(problems(&schema, &json!({ "path": "a", "limit": 20.0 })).is_empty(), "a whole float is an integer");
        assert!(problems(&schema, &json!({ "path": "a", "limit": null })).is_empty(), "a null is read as left out");
        assert_eq!(problems(&schema, &json!({ "limit": "20" })), ["`path` is required", "`limit` must be integer, not a string"]);
        assert_eq!(problems(&schema, &json!({ "path": "a", "format": "html" })), [r#"`format` must be one of "markdown", "text""#]);
        assert_eq!(problems(&schema, &json!({ "path": "a", "todos": [{ "content": 1 }, {}] })), ["`todos[0].content` must be string, not a number", "`todos[1].content` is required"]);
        assert!(problems(&json!({ "type": "object", "properties": { "x": { "anyOf": [] } } }), &json!({ "x": [1] })).is_empty(), "keywords it does not check pass");
    }
}
