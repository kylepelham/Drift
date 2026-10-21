//! A call's input repaired where its schema leaves exactly one reading, before it is checked: models
//! send numbers and booleans as strings, objects as JSON text, and placeholders for optional fields.
//! Anything the schema leaves open (unions with several candidates, `anyOf` beside `oneOf`) is kept for
//! the check to report.

use serde_json::{Map, Value};

/// Deeper than any tool's arguments go; a cycle of references stops here.
const MAX_DEPTH: usize = 6;
/// Keywords that may let a property accept `null` or `{}`, so a placeholder there is kept.
const OPEN: [&str; 7] = ["anyOf", "oneOf", "allOf", "enum", "const", "$ref", "nullable"];
/// The largest integer a model's number keeps exactly, as JSON parsers read it.
const SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// The input as it should run; unchanged when nothing needs repair or the schema is not an object.
pub fn repair(schema: &Value, input: &Value) -> Value {
    if schema["type"] != "object" {
        return input.clone();
    }

    at(input, schema, schema, 0)
}

fn at(value: &Value, schema: &Value, root: &Value, depth: usize) -> Value {
    if depth > MAX_DEPTH {
        return value.clone();
    }
    if let Some(reference) = schema["$ref"].as_str() {
        return referenced(value, reference, root, depth);
    }

    match &schema["type"] {
        Value::Array(types) => among_types(value, schema, types, root, depth),
        Value::Null => composed(value, schema, root, depth),
        Value::String(kind) => as_type(kind, value, schema, root, depth),
        _ => value.clone(),
    }
}

/// `typeof` as the schemas' authors think of it: arrays and `null` are objects.
fn js_type(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
        _ => "object",
    }
}

/// A local `#/$defs/name` or `#/definitions/name`; any other reference is left alone.
fn referenced(value: &Value, reference: &str, root: &Value, depth: usize) -> Value {
    let found = ["#/$defs/", "#/definitions/"].iter().find_map(|prefix| {
        let name = reference.strip_prefix(prefix).filter(|name| !name.contains('/'))?;
        let section = &prefix[2..prefix.len() - 1];
        let name = name.replace("~1", "/").replace("~0", "~");
        root[section].get(&name).filter(|target| target.is_object())
    });

    found.map_or_else(|| value.clone(), |target| at(value, target, root, depth + 1))
}

fn among_types(value: &Value, schema: &Value, types: &[Value], root: &Value, depth: usize) -> Value {
    let named = |name: &str| types.iter().any(|kind| kind == name);
    if (value.is_null() && named("null")) || named(js_type(value)) {
        return value.clone();
    }

    let concrete: Vec<&Value> = types.iter().filter(|kind| *kind != "null").collect();
    let [only] = concrete[..] else { return value.clone() };
    let mut narrowed = schema.clone();
    narrowed["type"] = only.clone();

    at(value, &narrowed, root, depth + 1)
}

/// A schema with no type: one `anyOf` or `oneOf` branch that the value does not already fit.
fn composed(value: &Value, schema: &Value, root: &Value, depth: usize) -> Value {
    let (any, one) = (schema["anyOf"].as_array(), schema["oneOf"].as_array());
    let Some(branches) = any.xor(one).filter(|_| !value.is_null()) else {
        return value.clone();
    };
    if branches
        .iter()
        .any(|branch| !branch.is_object() || branch["type"] == js_type(value))
    {
        return value.clone();
    }

    let candidates: Vec<&Value> = branches.iter().filter(|branch| branch["type"] != "null").collect();
    match candidates[..] {
        [only] => at(value, only, root, depth + 1),
        _ => value.clone(),
    }
}

fn as_type(kind: &str, value: &Value, schema: &Value, root: &Value, depth: usize) -> Value {
    match kind {
        "number" | "integer" => number(value, kind == "integer"),
        "boolean" => match value.as_str() {
            Some("true") => Value::Bool(true),
            Some("false") => Value::Bool(false),
            _ => value.clone(),
        },
        "object" => object(value, schema, root, depth),
        "array" => array(value, schema, root, depth),
        _ => value.clone(),
    }
}

fn number(value: &Value, integer: bool) -> Value {
    let Some(text) = value.as_str().map(str::trim).filter(|text| !text.is_empty()) else {
        return value.clone();
    };
    let Some(parsed) = text.parse::<f64>().ok().filter(|parsed| parsed.is_finite()) else {
        return value.clone();
    };

    let whole = parsed.fract() == 0.0 && parsed.abs() <= SAFE_INTEGER;
    if whole {
        return Value::from(parsed as i64);
    }
    if integer {
        return value.clone();
    }

    serde_json::Number::from_f64(parsed).map_or_else(|| value.clone(), Value::Number)
}

/// JSON text for an object or array is read as one; anything else is left as it came.
fn parsed(value: &Value) -> Value {
    value
        .as_str()
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_else(|| value.clone())
}

fn object(value: &Value, schema: &Value, root: &Value, depth: usize) -> Value {
    let Value::Object(fields) = parsed(value) else {
        return value.clone();
    };
    let shape = Shape::of(schema);

    let mut repaired = Map::new();
    for (key, given) in fields {
        match shape.field(&key, &given) {
            Field::Drop => {}
            Field::Keep => {
                repaired.insert(key, given);
            }
            Field::Repair(property) => {
                let fixed = at(&given, property, root, depth + 1);
                repaired.insert(key, fixed);
            }
        }
    }

    Value::Object(repaired)
}

/// What an object schema says about its fields.
struct Shape<'a> {
    properties: Option<&'a Map<String, Value>>,
    required: Vec<&'a str>,
    additional: &'a Value,
    patterned: bool,
    composed: bool,
}

enum Field<'a> {
    Drop,
    Keep,
    Repair(&'a Value),
}

impl<'a> Shape<'a> {
    fn of(schema: &'a Value) -> Self {
        Self {
            properties: schema["properties"].as_object(),
            required: schema["required"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect(),
            additional: &schema["additionalProperties"],
            patterned: schema["patternProperties"].is_object(),
            composed: ["allOf", "anyOf", "oneOf"].iter().any(|key| schema[*key].is_array()),
        }
    }

    fn field(&self, key: &str, given: &Value) -> Field<'a> {
        let declared = self.properties.and_then(|properties| properties.get(key));
        let property = match declared {
            Some(property) => property,
            None if self.patterned => return Field::Keep,
            None if *self.additional == Value::Bool(false) && !self.composed => return Field::Drop,
            None => self.additional,
        };
        if !property.is_object() {
            return Field::Keep;
        }

        if declared.is_some() && !self.required.contains(&key) && self.placeholder(property, given) {
            return Field::Drop;
        }

        Field::Repair(property)
    }

    /// `null` or `{}` sent for an optional field whose plain single type cannot hold it.
    fn placeholder(&self, property: &Value, given: &Value) -> bool {
        let plain = property["type"].as_str().is_some_and(|kind| kind != "null")
            && !self.composed
            && OPEN.iter().all(|keyword| property.get(keyword).is_none());
        let empty = given.as_object().is_some_and(Map::is_empty) && property["type"] != "object";

        plain && (given.is_null() || empty)
    }
}

fn array(value: &Value, schema: &Value, root: &Value, depth: usize) -> Value {
    let tuple = schema["prefixItems"].as_array().or_else(|| schema["items"].as_array());

    if let Value::Array(items) = parsed(value) {
        let repaired = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let member = member(schema, tuple, index);
                if member.is_object() {
                    at(item, member, root, depth + 1)
                } else {
                    item.clone()
                }
            })
            .collect();
        return Value::Array(repaired);
    }

    let each = &schema["items"];
    if tuple.is_some() || !each.is_object() {
        return value.clone();
    }

    let repaired = at(value, each, root, depth + 1);
    if fits(&repaired, &each["type"]) {
        Value::Array(vec![repaired])
    } else {
        value.clone()
    }
}

/// The schema an array's item at `index` follows: its tuple position, else the rest.
fn member<'a>(schema: &'a Value, tuple: Option<&'a Vec<Value>>, index: usize) -> &'a Value {
    let Some(tuple) = tuple else { return &schema["items"] };
    let rest = if schema["prefixItems"].is_array() {
        &schema["items"]
    } else {
        &schema["additionalItems"]
    };

    tuple.get(index).unwrap_or(rest)
}

/// Whether a single value, repaired, can be wrapped as the array's one item.
fn fits(value: &Value, kind: &Value) -> bool {
    match kind.as_str() {
        Some("object") => value.is_object(),
        Some("array") => value.is_array(),
        Some("integer") => value.is_i64() || value.is_u64(),
        Some("number") => value.is_number(),
        Some("string") => value.is_string(),
        Some("boolean") => value.is_boolean(),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
