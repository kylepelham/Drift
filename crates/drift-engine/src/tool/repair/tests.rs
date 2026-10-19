use serde_json::{Value, json};

use super::repair;

fn object(properties: Value) -> Value {
    json!({ "type": "object", "properties": properties })
}

#[test]
fn valid_input_and_unknown_properties_are_kept() {
    let input = json!({ "count": 2, "nested": { "enabled": true }, "items": [2, 3], "extra": "keep" });
    let schema = object(json!({
        "count": { "type": "integer" },
        "nested": object(json!({ "enabled": { "type": "boolean" } })),
        "items": { "type": "array", "items": { "type": "number" } },
    }));

    assert_eq!(repair(&schema, &input), input);
}

#[test]
fn a_stringified_root_and_nested_containers_are_parsed() {
    let schema = object(json!({
        "count": { "type": "integer" },
        "item": object(json!({ "enabled": { "type": "boolean" } })),
        "list": { "type": "array", "items": { "type": "integer" } },
    }));
    let text = json!(r#"{"count":"2","item":"{\"enabled\":\"false\"}","list":"[\"3\"]"}"#);

    assert_eq!(
        repair(&schema, &text),
        json!({ "count": 2, "item": { "enabled": false }, "list": [3] })
    );
    assert_eq!(repair(&schema, &json!("{broken")), json!("{broken"));
    assert_eq!(repair(&schema, &json!("[]")), json!("[]"), "an array is not an object");
    assert_eq!(repair(&schema, &Value::Null), Value::Null);
}

#[test]
fn extras_are_dropped_only_from_explicitly_closed_objects() {
    let closed = |properties: Value| {
        let mut schema = object(properties);
        schema["additionalProperties"] = json!(false);
        schema
    };
    let mut schema = closed(json!({
        "known": { "type": "integer" },
        "closed": closed(json!({ "keep": { "type": "integer" } })),
        "open": object(json!({ "keep": { "type": "integer" } })),
        "items": { "type": "array", "items": closed(json!({ "keep": { "type": "integer" } })) },
    }));
    let input = json!({
        "known": "2",
        "extra": true,
        "closed": { "keep": "3", "extra": true },
        "open": { "keep": "4", "extra": true },
        "items": [{ "keep": "5", "extra": true }],
    });

    assert_eq!(
        repair(&schema, &input),
        json!({ "known": 2, "closed": { "keep": 3 }, "open": { "keep": 4, "extra": true }, "items": [{ "keep": 5 }] })
    );

    schema = closed(json!({}));
    assert_eq!(repair(&schema, &json!({ "extra": true })), json!({}));
}

#[test]
fn unknown_keys_stay_when_pattern_properties_or_compositions_may_own_them() {
    let mut patterned = object(json!({ "known": { "type": "integer" } }));
    patterned["additionalProperties"] = json!(false);
    patterned["patternProperties"] = json!({ "^match$": { "type": "integer" } });
    let input = json!({ "known": 1, "match": "2", "extra": true });
    assert_eq!(repair(&patterned, &input), input);

    let input = json!({ "name": "example", "extra": true });
    for keyword in ["allOf", "anyOf", "oneOf"] {
        let schema = json!({
            "type": "object",
            keyword: [object(json!({ "name": { "type": "string" } }))],
            "additionalProperties": false,
        });
        assert_eq!(repair(&schema, &input), input, "{keyword}");
    }
}

#[test]
fn only_optional_plain_nulls_and_non_object_placeholders_are_removed() {
    let mut schema = object(json!({
        "optional": { "type": "string" },
        "required": { "type": "string" },
        "nullable": { "type": "string", "nullable": true },
        "union": { "anyOf": [{ "type": "integer" }, { "type": "null" }] },
        "constant": { "anyOf": [{ "type": "integer" }, { "const": null }] },
        "permissive": { "anyOf": [{ "type": "integer" }, true] },
        "referenced": { "anyOf": [{ "type": "integer" }, { "$ref": "#/$defs/nullable" }] },
        "placeholder": { "type": "integer" },
        "array": { "type": "array", "items": { "type": "string" } },
        "requiredPlaceholder": { "type": "boolean" },
        "object": { "type": "object" },
        "unknown": {},
    }));
    schema["required"] = json!(["required", "requiredPlaceholder"]);
    let input = json!({
        "optional": null, "required": null, "nullable": null, "union": null, "constant": null,
        "permissive": null, "referenced": null, "placeholder": {}, "array": {},
        "requiredPlaceholder": {}, "object": {}, "unknown": null,
    });

    assert_eq!(
        repair(&schema, &input),
        json!({
            "required": null, "nullable": null, "union": null, "constant": null, "permissive": null,
            "referenced": null, "requiredPlaceholder": {}, "object": {}, "unknown": null,
        })
    );
}

#[test]
fn nulls_whose_validity_hides_in_a_composition_are_kept() {
    let schema = object(json!({
        "wrapped": { "anyOf": [{ "type": ["string", "null"] }] },
        "enumerated": { "anyOf": [{ "enum": [null, "keep"] }] },
        "permissive": { "anyOf": [{}] },
        "constant": { "type": "string", "const": "keep" },
    }));
    let input = json!({ "wrapped": null, "enumerated": null, "permissive": null, "constant": null });
    assert_eq!(repair(&schema, &input), input);

    let mut composed = object(json!({ "value": { "type": "string" } }));
    composed["allOf"] = json!([{ "required": ["value"] }]);
    let input = json!({ "value": null });
    assert_eq!(repair(&composed, &input), input, "the parent may require it");
}

#[test]
fn numeric_and_boolean_strings_are_read_and_anything_else_kept() {
    let schema = object(json!({
        "number": { "type": "number" },
        "integer": { "type": "integer" },
        "enabled": { "type": "boolean" },
        "disabled": { "type": "boolean" },
        "valid": { "type": "integer" },
        "empty": { "type": "number" },
        "infinite": { "type": "number" },
        "fractional": { "type": "integer" },
        "unsafe": { "type": "integer" },
        "uppercase": { "type": "boolean" },
    }));
    let input = json!({
        "number": "1.5", "integer": "42", "enabled": "true", "disabled": "false", "valid": 3,
        "empty": " ", "infinite": "Infinity", "fractional": "1.5", "unsafe": "9007199254740992",
        "uppercase": "TRUE",
    });

    let mut expected = input.clone();
    expected["number"] = json!(1.5);
    expected["integer"] = json!(42);
    expected["enabled"] = json!(true);
    expected["disabled"] = json!(false);
    assert_eq!(repair(&schema, &input), expected);
}

#[test]
fn a_compatible_single_value_is_wrapped_as_an_array() {
    let schema = object(json!({
        "text": { "type": "array", "items": { "type": "string" } },
        "integer": { "type": "array", "items": { "type": "integer" } },
        "boolean": { "type": "array", "items": { "type": "boolean" } },
        "item": { "type": "array", "items": object(json!({ "count": { "type": "integer" } })) },
        "incompatible": { "type": "array", "items": { "type": "string" } },
        "fractional": { "type": "array", "items": { "type": "integer" } },
        "unconstrained": { "type": "array" },
    }));
    let input = json!({
        "text": "one", "integer": "42", "boolean": "false", "item": r#"{"count":"2"}"#,
        "incompatible": 2, "fractional": 1.5, "unconstrained": "4",
    });

    assert_eq!(
        repair(&schema, &input),
        json!({
            "text": ["one"], "integer": [42], "boolean": [false], "item": [{ "count": 2 }],
            "incompatible": 2, "fractional": 1.5, "unconstrained": "4",
        })
    );
}

#[test]
fn a_question_shaped_input_is_repaired_all_the_way_down() {
    let mut option = object(json!({ "label": { "type": "string" }, "description": { "type": "string" } }));
    option["required"] = json!(["label"]);
    let schema = object(json!({
        "questions": {
            "type": "array",
            "items": object(json!({
                "question": { "type": "string" },
                "multiple": { "type": "boolean" },
                "options": { "type": "array", "items": option },
            })),
        },
    }));
    let input = json!({
        "questions": [{ "question": "Pick one", "multiple": "false", "options": { "label": "First", "description": null } }],
    });

    assert_eq!(
        repair(&schema, &input),
        json!({ "questions": [{ "question": "Pick one", "multiple": false, "options": [{ "label": "First" }] }] })
    );
}

#[test]
fn a_union_with_one_concrete_alternative_is_repaired_and_accepted_values_kept() {
    let schema = object(json!({
        "number": { "anyOf": [{ "type": "number" }, { "type": "null" }] },
        "boolean": { "oneOf": [{ "type": "boolean" }, { "type": "null" }] },
        "nullable": { "anyOf": [{ "type": "number" }, { "type": "null" }] },
        "typed": { "type": ["integer", "null"] },
        "typedBoolean": { "type": ["boolean", "null"] },
        "accepted": { "anyOf": [{ "type": "string" }, { "type": "number" }] },
        "valid": { "type": ["number", "null"] },
    }));
    let input = json!({
        "number": "2", "boolean": "false", "nullable": null, "typed": "3", "typedBoolean": "true",
        "accepted": "4", "valid": 5,
    });

    let mut expected = input.clone();
    expected["number"] = json!(2);
    expected["boolean"] = json!(false);
    expected["typed"] = json!(3);
    expected["typedBoolean"] = json!(true);
    assert_eq!(repair(&schema, &input), expected);
}

#[test]
fn tuple_positions_and_rest_items_are_repaired() {
    let schema = object(json!({
        "prefix": {
            "type": "array",
            "prefixItems": [{ "type": "integer" }, { "type": "boolean" }],
            "items": { "type": "number" },
        },
        "draft": { "type": "array", "items": [{ "type": "integer" }, { "type": "boolean" }] },
        "valid": { "type": "array", "prefixItems": [{ "type": "integer" }, { "type": "boolean" }] },
        "scalar": { "type": "array", "prefixItems": [{ "type": "integer" }] },
    }));
    let input =
        json!({ "prefix": ["2", "false", "3"], "draft": r#"["4","true"]"#, "valid": [2, false], "scalar": "5" });

    assert_eq!(
        repair(&schema, &input),
        json!({ "prefix": [2, false, 3], "draft": [4, true], "valid": [2, false], "scalar": "5" })
    );
}

#[test]
fn typed_dictionaries_and_local_references_are_followed() {
    let mut schema = object(json!({
        "modern": { "$ref": "#/$defs/integer" },
        "legacy": { "$ref": "#/definitions/boolean" },
        "nested": { "$ref": "#/$defs/nested" },
        "dictionary": { "type": "object", "additionalProperties": { "$ref": "#/$defs/integer" } },
        "missing": { "$ref": "#/$defs/missing" },
        "pointer": { "$ref": "#/$defs/nested/properties/count" },
        "escaped": { "$ref": "#/$defs/a~1b~0c" },
    }));
    schema["$defs"] = json!({
        "integer": { "type": "integer" },
        "a/b~c": { "type": "integer" },
        "nested": object(json!({ "count": { "$ref": "#/$defs/integer" } })),
    });
    schema["definitions"] = json!({ "boolean": { "type": "boolean" } });
    let input = json!({
        "modern": "2", "legacy": "false", "nested": { "count": "3" }, "dictionary": { "first": "4" },
        "missing": "5", "pointer": "6", "escaped": "7",
    });

    assert_eq!(
        repair(&schema, &input),
        json!({
            "modern": 2, "legacy": false, "nested": { "count": 3 }, "dictionary": { "first": 4 },
            "missing": "5", "pointer": "6", "escaped": 7,
        })
    );
}

#[test]
fn ambiguous_unions_compositions_and_untyped_roots_are_left_for_the_check() {
    let mut first = object(json!({ "value": { "type": "integer" } }));
    first["required"] = json!(["value"]);
    let mut second = object(json!({ "value": { "type": "number" } }));
    second["required"] = json!(["value"]);
    let schema = object(json!({
        "numeric": { "anyOf": [{ "type": "number" }, { "type": "integer" }] },
        "objects": { "oneOf": [first, second] },
        "both": { "anyOf": [{ "type": "integer" }], "oneOf": [{ "type": "integer" }] },
        "composed": { "allOf": [{ "type": "integer" }] },
        "unknown": {},
    }));
    let input = json!({ "numeric": "2", "objects": { "value": "3" }, "both": "4", "composed": "5", "unknown": "6" });

    assert_eq!(repair(&schema, &input), input);
    assert_eq!(
        repair(&json!({ "properties": { "numeric": { "type": "integer" } } }), &input),
        input
    );
    assert_eq!(
        repair(
            &json!({ "allOf": [object(json!({ "numeric": { "type": "integer" } }))] }),
            &input
        ),
        input
    );
}

#[test]
fn a_reference_cycle_stops_at_the_depth_limit() {
    let mut schema = object(json!({ "node": { "$ref": "#/$defs/node" } }));
    schema["$defs"] =
        json!({ "node": object(json!({ "next": { "$ref": "#/$defs/node" }, "n": { "type": "integer" } })) });
    let input = json!({ "node": { "n": "1", "next": { "next": { "next": { "next": { "next": { "n": "9" } } } } } } });

    let repaired = repair(&schema, &input);
    assert_eq!(repaired["node"]["n"], json!(1));
    assert_eq!(
        repaired["node"]["next"]["next"]["next"]["next"]["next"]["n"],
        json!("9"),
        "past the limit nothing is touched"
    );
}
