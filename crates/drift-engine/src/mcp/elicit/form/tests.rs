use serde_json::json;

use super::{content, fields, questions};

fn answers(given: &[&[&str]]) -> Vec<Vec<String>> {
    given
        .iter()
        .map(|answer| answer.iter().map(ToString::to_string).collect())
        .collect()
}

fn schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string", "title": "Name", "minLength": 2 },
            "age": { "type": "integer", "minimum": 0, "maximum": 150 },
            "plan": { "type": "string", "oneOf": [{ "const": "p", "title": "Pro" }, { "const": "f", "title": "Free" }] },
            "tags": { "type": "array", "items": { "type": "string", "enum": ["a", "b"] } },
            "subscribe": { "type": "boolean", "description": "Mail me" },
        },
        "required": ["name", "plan"],
    })
}

#[test]
fn each_property_is_a_question_in_the_servers_order() {
    let order = ["name", "age", "plan", "tags", "subscribe"].map(String::from).to_vec();
    let fields = fields(&schema(), Some(order));
    let asked = questions(&fields, "billing");

    let texts: Vec<&str> = asked.iter().map(|question| question.question.as_str()).collect();
    assert_eq!(
        texts,
        [
            "Name",
            "age (optional)",
            "plan",
            "tags (optional)",
            "subscribe (optional): Mail me"
        ]
    );
    assert!(
        asked.iter().all(|question| question.header == "billing"),
        "the card names the server"
    );
    let labels = |at: usize| -> Vec<&str> { asked[at].options.iter().map(|option| option.label.as_str()).collect() };
    assert_eq!(labels(2), ["Pro", "Free"]);
    assert!(!asked[2].custom, "a choice is only its options");
    assert_eq!(labels(3), ["a", "b"]);
    assert!(asked[3].multiple);
    assert_eq!(labels(4), ["Yes", "No", "Skip"], "an optional question can be left out");
    assert!(asked[0].custom && asked[0].options.is_empty());
}

#[test]
fn answers_become_the_schemas_values() {
    let order = ["name", "age", "plan", "tags", "subscribe"].map(String::from).to_vec();
    let fields = fields(&schema(), Some(order));

    let given = answers(&[&["Ada"], &["36"], &["Pro"], &["a", "b"], &["Yes"]]);
    assert_eq!(
        content(&fields, &given).unwrap(),
        json!({ "name": "Ada", "age": 36, "plan": "p", "tags": ["a", "b"], "subscribe": true })
    );
    let skipped = answers(&[&["Ada"], &[], &["Free"], &[], &["Skip"]]);
    assert_eq!(
        content(&fields, &skipped).unwrap(),
        json!({ "name": "Ada", "plan": "f" })
    );
}

#[test]
fn a_bad_answer_is_refused_with_why() {
    let order = ["name", "age", "plan", "tags", "subscribe"].map(String::from).to_vec();
    let fields = fields(&schema(), Some(order));
    let refused = |given: &[&[&str]]| content(&fields, &answers(given)).unwrap_err();

    assert_eq!(refused(&[&["A"], &[], &["Pro"]]), "Name: needs at least 2 characters");
    assert_eq!(
        refused(&[&["Ada"], &["3.5"], &["Pro"]]),
        "age: 3.5 is not a whole number"
    );
    assert_eq!(refused(&[&["Ada"], &["200"], &["Pro"]]), "age: 200 is outside 0 to 150");
    assert_eq!(refused(&[&["Ada"], &["x"], &["Pro"]]), "age: x is not a number");
    assert_eq!(refused(&[&["Ada"], &[], &[]]), "plan needs an answer.");
    assert_eq!(
        refused(&[&["Ada"], &[], &["Gold"]]),
        "plan: Gold is not one of the choices"
    );
}

#[test]
fn formats_and_legacy_enum_names_are_understood() {
    let schema = json!({
        "type": "object",
        "properties": {
            "mail": { "type": "string", "format": "email" },
            "day": { "type": "string", "format": "date" },
            "size": { "type": "string", "enum": ["s", "l"], "enumNames": ["Small", "Large"] },
        },
    });
    // Without an order from the wire, properties come sorted: day, mail, size.
    let fields = fields(&schema, None);
    let names: Vec<String> = questions(&fields, "s")[2]
        .options
        .iter()
        .map(|option| option.label.clone())
        .collect();
    assert_eq!(names, ["Small", "Large", "Skip"]);

    assert!(content(&fields, &answers(&[&[], &["nope"], &[]])).is_err());
    assert!(content(&fields, &answers(&[&["2026-1-2"], &[], &[]])).is_err());
    assert_eq!(
        content(&fields, &answers(&[&["2026-10-19"], &["a@b.co"], &["Large"]])).unwrap(),
        json!({ "mail": "a@b.co", "day": "2026-10-19", "size": "l" })
    );
}

#[test]
fn a_form_without_fields_asks_to_go_on() {
    let asked = questions(&[], "deploy");
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].options[0].label, "Done");
    assert_eq!(content(&[], &answers(&[&["Done"]])).unwrap(), json!({}));
}
