//! The schema sent in Anthropic's native structured-output slot.
//!
//! An Anthropic structured-output run carries two schemas, and they are kept
//! apart on purpose:
//!
//! - The **validation schema** is what [`LlmClient::compile_schema`] returns
//!   for Anthropic: the configured schema with `additionalProperties: false`
//!   on every object that declares `properties` and leaves it open. The agent
//!   validates the model's reply against it, retries on a violation, and it is
//!   the schema to show the model. Every constraint the caller wrote is in it.
//! - The **slot schema** is what [`lower_output_format_schema`] derives from
//!   the validation schema for `output_config.format`. Anthropic compiles that
//!   slot into a decoding grammar, and the grammar compiler rejects part of
//!   JSON Schema with HTTP 400. The slot schema is only ever sent in that slot;
//!   it never feeds validation.
//!
//! The lowering removes a keyword only when both hold:
//!
//! 1. Anthropic rejects it (the documented "Not supported" list for structured
//!    outputs, confirmed keyword by keyword against the live API for number,
//!    integer, string, array, object and composition nodes on 2026-09-25 with
//!    claude-haiku-4-5, claude-sonnet-4-6 and claude-opus-5-5, which all
//!    answered identically), and
//! 2. removing it only relaxes the schema, so every value the validation
//!    schema accepts is still accepted by the slot. The grammar can then never
//!    force the model away from a value meerkat would accept, and anything the
//!    removed keyword forbade is still caught by validation.
//!
//! Keywords Anthropic rejects that fail the second test are left in place, so
//! the provider keeps failing loudly instead of meerkat silently narrowing
//! the output: `patternProperties`, `prefixItems`, array-form `items`,
//! `additionalProperties` other than `false`, empty `{}` schemas, enums with
//! object or array members, unsupported regex features in `pattern`, and
//! string keywords (`minLength`, `pattern`, `format`, ...) that sit directly
//! beside `anyOf` or `$ref` instead of inside the typed schema.
//!
//! Keywords Anthropic accepts are never touched. That includes `minLength`,
//! `maxLength` and `pattern`: the documentation lists string length as
//! unsupported, but the API accepts both and the grammar enforces them.
//!
//! Each removed constraint is restated in the node's `description`, as
//! Anthropic's own SDKs do, so the model still reads it in the format
//! instructions Anthropic derives from the slot. A schema that uses none of
//! the rejected keywords lowers to itself.
//!
//! [`LlmClient::compile_schema`]: meerkat_llm_core::LlmClient::compile_schema

use serde_json::{Map, Value};

/// A constraint keyword Anthropic's structured-output grammar compiler
/// rejects wherever it appears and whose removal only relaxes the schema.
///
/// Grouped by the instance type the keyword constrains. The keyword is
/// removed from any node, whatever its declared `type`: on a node of another
/// type it constrains nothing, so removing it there changes no accepted value
/// either, and the API rejects these keywords beside `anyOf`, beside `$ref`
/// and on type arrays such as `["number", "null"]` as well.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotRejectedConstraint {
    // number and integer
    Minimum,
    Maximum,
    ExclusiveMinimum,
    ExclusiveMaximum,
    MultipleOf,
    // array (the slot accepts only `minItems` 0 or 1; see
    // `SLOT_ACCEPTED_MIN_ITEMS`). `minContains` / `maxContains` are accepted
    // and do nothing once `contains` is gone, so they are kept.
    MaxItems,
    UniqueItems,
    Contains,
    // object
    MinProperties,
    MaxProperties,
    PropertyNames,
    DependentRequired,
    DependentSchemas,
    /// Draft-07 predecessor of `dependentRequired` / `dependentSchemas`.
    Dependencies,
    UnevaluatedProperties,
    // composition
    Not,
}

impl SlotRejectedConstraint {
    /// Every rejected constraint, in the order removed constraints are
    /// restated in a node's description.
    const ALL: [Self; 16] = [
        Self::Minimum,
        Self::Maximum,
        Self::ExclusiveMinimum,
        Self::ExclusiveMaximum,
        Self::MultipleOf,
        Self::MaxItems,
        Self::UniqueItems,
        Self::Contains,
        Self::MinProperties,
        Self::MaxProperties,
        Self::PropertyNames,
        Self::DependentRequired,
        Self::DependentSchemas,
        Self::Dependencies,
        Self::UnevaluatedProperties,
        Self::Not,
    ];

    /// The JSON Schema keyword this constraint is spelled as.
    const fn keyword(self) -> &'static str {
        match self {
            Self::Minimum => "minimum",
            Self::Maximum => "maximum",
            Self::ExclusiveMinimum => "exclusiveMinimum",
            Self::ExclusiveMaximum => "exclusiveMaximum",
            Self::MultipleOf => "multipleOf",
            Self::MaxItems => "maxItems",
            Self::UniqueItems => "uniqueItems",
            Self::Contains => "contains",
            Self::MinProperties => "minProperties",
            Self::MaxProperties => "maxProperties",
            Self::PropertyNames => "propertyNames",
            Self::DependentRequired => "dependentRequired",
            Self::DependentSchemas => "dependentSchemas",
            Self::Dependencies => "dependencies",
            Self::UnevaluatedProperties => "unevaluatedProperties",
            Self::Not => "not",
        }
    }
}

/// `minItems` values the slot accepts. Any other value is rejected and is
/// removed (a lower bound only ever narrows the accepted arrays).
const SLOT_ACCEPTED_MIN_ITEMS: [u64; 2] = [0, 1];

/// `format` values the slot accepts on a string node. Any other format on a
/// node that may be a string is rejected and is removed. `format` on number
/// and integer nodes (for example `int32`, `double`) is accepted and kept.
const SLOT_ACCEPTED_STRING_FORMATS: [&str; 10] = [
    "date-time",
    "time",
    "date",
    "duration",
    "email",
    "hostname",
    "uri",
    "ipv4",
    "ipv6",
    "uuid",
];

const MIN_ITEMS: &str = "minItems";
const FORMAT: &str = "format";
const DESCRIPTION: &str = "description";
const ONE_OF: &str = "oneOf";
const ANY_OF: &str = "anyOf";

/// Label that introduces restated constraints in a node's description.
const RESTATED_CONSTRAINTS_LABEL: &str = "Constraints (JSON Schema): ";

/// Lower a validation schema into the schema sent in Anthropic's
/// `output_config.format` slot.
///
/// See the module documentation for the rules. The input is never modified;
/// the caller keeps validating against it.
pub(crate) fn lower_output_format_schema(validation_schema: &Value) -> Value {
    let mut slot = validation_schema.clone();
    lower_schema_node(&mut slot);
    slot
}

/// Lower one schema node, then the child schemas the grammar compiler reads.
///
/// Only schema-valued positions are walked, so literal data such as property
/// names, `required`, `enum`, `const`, `default` and `examples` is never
/// mistaken for a keyword. Every walked position is one where relaxing the
/// child relaxes the parent. `if`/`then`/`else` are not walked: the API
/// accepts them without validating inside them, and relaxing an `if`
/// condition would make `then` apply to more values.
fn lower_schema_node(node: &mut Value) {
    // Boolean schemas carry no keywords.
    let Value::Object(schema) = node else {
        return;
    };

    let mut restated: Vec<(&'static str, Value)> = Vec::new();
    for constraint in SlotRejectedConstraint::ALL {
        if let Some(value) = schema.remove(constraint.keyword()) {
            restated.push((constraint.keyword(), value));
        }
    }
    if schema
        .get(MIN_ITEMS)
        .is_some_and(|min_items| !is_slot_accepted_min_items(min_items))
        && let Some(value) = schema.remove(MIN_ITEMS)
    {
        restated.push((MIN_ITEMS, value));
    }
    if may_be_string(schema)
        && schema
            .get(FORMAT)
            .and_then(Value::as_str)
            .is_some_and(|format| !SLOT_ACCEPTED_STRING_FORMATS.contains(&format))
        && let Some(value) = schema.remove(FORMAT)
    {
        restated.push((FORMAT, value));
    }

    // The slot rejects `oneOf` but accepts `anyOf`, which accepts every value
    // `oneOf` accepts. With an `anyOf` already beside it there is no lossless
    // single rewrite, so the node is left for the provider to reject.
    if !schema.contains_key(ANY_OF)
        && let Some(alternatives) = schema.remove(ONE_OF)
    {
        schema.insert(ANY_OF.to_string(), alternatives);
    }

    if !restated.is_empty() {
        restate_in_description(schema, &restated);
    }

    for key in ["properties", "$defs", "definitions"] {
        if let Some(Value::Object(children)) = schema.get_mut(key) {
            children.values_mut().for_each(lower_schema_node);
        }
    }
    if let Some(items @ Value::Object(_)) = schema.get_mut("items") {
        lower_schema_node(items);
    }
    for key in ["allOf", ANY_OF] {
        // An `anyOf` that stays beside an un-rewritten `oneOf` is still
        // walked: relaxing an `anyOf` member relaxes the node. The `oneOf`
        // members themselves are not, because relaxing a `oneOf` member can
        // make a value match two alternatives and so be rejected.
        if let Some(Value::Array(members)) = schema.get_mut(key) {
            members.iter_mut().for_each(lower_schema_node);
        }
    }
}

fn is_slot_accepted_min_items(min_items: &Value) -> bool {
    min_items
        .as_u64()
        .is_some_and(|count| SLOT_ACCEPTED_MIN_ITEMS.contains(&count))
}

/// Whether the node's declared `type` admits strings.
fn may_be_string(schema: &Map<String, Value>) -> bool {
    match schema.get("type") {
        Some(Value::String(instance_type)) => instance_type == "string",
        Some(Value::Array(instance_types)) => instance_types
            .iter()
            .any(|instance_type| instance_type.as_str() == Some("string")),
        _ => false,
    }
}

/// Append the removed constraints to the node's description as one compact
/// JSON object, in [`SlotRejectedConstraint::ALL`] order followed by
/// `minItems` and `format`. A non-string `description` is left alone.
fn restate_in_description(schema: &mut Map<String, Value>, restated: &[(&'static str, Value)]) {
    let mut constraints = String::from("{");
    for (index, (keyword, value)) in restated.iter().enumerate() {
        if index > 0 {
            constraints.push(',');
        }
        // `Value`'s `Display` is compact JSON and cannot fail.
        constraints.push_str(&Value::String((*keyword).to_string()).to_string());
        constraints.push(':');
        constraints.push_str(&value.to_string());
    }
    constraints.push('}');

    match schema.get_mut(DESCRIPTION) {
        None => {
            schema.insert(
                DESCRIPTION.to_string(),
                Value::String(format!("{RESTATED_CONSTRAINTS_LABEL}{constraints}")),
            );
        }
        Some(Value::String(description)) if description.is_empty() => {
            *description = format!("{RESTATED_CONSTRAINTS_LABEL}{constraints}");
        }
        Some(Value::String(description)) => {
            description.push_str("\n\n");
            description.push_str(RESTATED_CONSTRAINTS_LABEL);
            description.push_str(&constraints);
        }
        Some(_) => {}
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Keywords whose removal the structural relaxation check accepts.
    fn removable_keyword(keyword: &str) -> bool {
        SlotRejectedConstraint::ALL
            .iter()
            .any(|constraint| constraint.keyword() == keyword)
            || keyword == MIN_ITEMS
            || keyword == FORMAT
    }

    /// Structural proof that `slot` only relaxes `validation`.
    ///
    /// Walks both trees in the positions the lowering walks and accepts
    /// exactly three kinds of difference: a removed constraint keyword from
    /// the rejected set (or `minItems` / string `format`), `oneOf` renamed to
    /// `anyOf`, and text appended to (or a new) `description`. Removing a
    /// conjunct constraint and widening `oneOf` to `anyOf` both only enlarge
    /// the set of accepted values, and descriptions are annotations, so a
    /// slot that passes accepts every value the validation schema accepts.
    fn assert_only_relaxes(validation: &Value, slot: &Value, path: &str) {
        let (Value::Object(validation), Value::Object(slot)) = (validation, slot) else {
            assert_eq!(
                validation, slot,
                "{path}: non-object schemas must be identical"
            );
            return;
        };
        for (key, validation_value) in validation {
            let child_path = format!("{path}/{key}");
            match slot.get(key) {
                None if removable_keyword(key) => {}
                None if key == ONE_OF && !validation.contains_key(ANY_OF) => {
                    assert_members_relax(validation_value, &slot[ANY_OF], &child_path);
                }
                None => panic!("{child_path}: keyword removed that is not relaxable"),
                Some(slot_value) => match key.as_str() {
                    "properties" | "$defs" | "definitions" => {
                        let (Value::Object(v), Value::Object(s)) = (validation_value, slot_value)
                        else {
                            assert_eq!(validation_value, slot_value, "{child_path}");
                            continue;
                        };
                        assert_eq!(
                            v.keys().collect::<Vec<_>>(),
                            s.keys().collect::<Vec<_>>(),
                            "{child_path}: names are literal data and never change"
                        );
                        for (name, child) in v {
                            assert_only_relaxes(child, &s[name], &format!("{child_path}/{name}"));
                        }
                    }
                    "items" if validation_value.is_object() => {
                        assert_only_relaxes(validation_value, slot_value, &child_path);
                    }
                    "allOf" | "anyOf" => {
                        assert_members_relax(validation_value, slot_value, &child_path);
                    }
                    DESCRIPTION => {
                        let v = validation_value.as_str().unwrap_or_default();
                        let s = slot_value.as_str().unwrap_or_default();
                        assert!(s.starts_with(v), "{child_path}: description only grows");
                    }
                    _ => assert_eq!(validation_value, slot_value, "{child_path}"),
                },
            }
        }
        for key in slot.keys() {
            if validation.contains_key(key) {
                continue;
            }
            let added_by_rewrite = key == ANY_OF && validation.contains_key(ONE_OF);
            assert!(
                key == DESCRIPTION || added_by_rewrite,
                "{path}/{key}: the slot may only add a description or the anyOf rewrite"
            );
        }
    }

    fn assert_members_relax(validation: &Value, slot: &Value, path: &str) {
        let (Value::Array(v), Value::Array(s)) = (validation, slot) else {
            panic!("{path}: composition members must stay an array");
        };
        assert_eq!(v.len(), s.len(), "{path}: members are never dropped");
        for (index, (v, s)) in v.iter().zip(s).enumerate() {
            assert_only_relaxes(v, s, &format!("{path}/{index}"));
        }
    }

    fn lower_and_check(validation: &Value) -> Value {
        let slot = lower_output_format_schema(validation);
        assert_only_relaxes(validation, &slot, "#");
        slot
    }

    fn closed(property: Value) -> Value {
        json!({
            "type": "object",
            "properties": {"x": property},
            "required": ["x"],
            "additionalProperties": false
        })
    }

    fn slot_property(property: Value) -> Value {
        lower_and_check(&closed(property))["properties"]["x"].clone()
    }

    fn restated(constraints: &str) -> Value {
        Value::String(format!("{RESTATED_CONSTRAINTS_LABEL}{constraints}"))
    }

    #[test]
    fn numeric_bounds_are_removed_and_restated_for_number_and_integer() {
        for instance_type in ["number", "integer"] {
            let slot = slot_property(json!({
                "type": instance_type,
                "minimum": 0,
                "maximum": 10,
                "exclusiveMinimum": -1,
                "exclusiveMaximum": 11,
                "multipleOf": 2
            }));
            assert_eq!(
                slot,
                json!({
                    "type": instance_type,
                    "description": restated(
                        r#"{"minimum":0,"maximum":10,"exclusiveMinimum":-1,"exclusiveMaximum":11,"multipleOf":2}"#
                    )
                }),
                "{instance_type}"
            );
        }
    }

    #[test]
    fn every_rejected_constraint_is_removed_on_its_own_instance_type() {
        let cases = [
            (
                SlotRejectedConstraint::Minimum,
                json!({"type": "number", "minimum": 0.5}),
            ),
            (
                SlotRejectedConstraint::Maximum,
                json!({"type": "integer", "maximum": 3}),
            ),
            (
                SlotRejectedConstraint::ExclusiveMinimum,
                json!({"type": "number", "exclusiveMinimum": 0}),
            ),
            (
                SlotRejectedConstraint::ExclusiveMaximum,
                json!({"type": "integer", "exclusiveMaximum": 9}),
            ),
            (
                SlotRejectedConstraint::MultipleOf,
                json!({"type": "number", "multipleOf": 0.25}),
            ),
            (
                SlotRejectedConstraint::MaxItems,
                json!({"type": "array", "items": {"type": "string"}, "maxItems": 3}),
            ),
            (
                SlotRejectedConstraint::UniqueItems,
                json!({"type": "array", "items": {"type": "string"}, "uniqueItems": false}),
            ),
            (
                SlotRejectedConstraint::Contains,
                json!({"type": "array", "items": {"type": "string"}, "contains": {"const": "a"}}),
            ),
            (
                SlotRejectedConstraint::MinProperties,
                json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": [],
                       "additionalProperties": false, "minProperties": 1}),
            ),
            (
                SlotRejectedConstraint::MaxProperties,
                json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": [],
                       "additionalProperties": false, "maxProperties": 1}),
            ),
            (
                SlotRejectedConstraint::PropertyNames,
                json!({"type": "object", "properties": {}, "required": [],
                       "additionalProperties": false, "propertyNames": {"pattern": "^x"}}),
            ),
            (
                SlotRejectedConstraint::DependentRequired,
                json!({"type": "object", "properties": {"a": {"type": "string"}, "b": {"type": "string"}},
                       "required": [], "additionalProperties": false,
                       "dependentRequired": {"a": ["b"]}}),
            ),
            (
                SlotRejectedConstraint::DependentSchemas,
                json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": [],
                       "additionalProperties": false,
                       "dependentSchemas": {"a": {"required": ["a"]}}}),
            ),
            (
                SlotRejectedConstraint::Dependencies,
                json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": [],
                       "additionalProperties": false, "dependencies": {"a": ["a"]}}),
            ),
            (
                SlotRejectedConstraint::UnevaluatedProperties,
                json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"],
                       "additionalProperties": false, "unevaluatedProperties": false}),
            ),
            (
                SlotRejectedConstraint::Not,
                json!({"type": "string", "not": {"const": "x"}}),
            ),
        ];
        assert_eq!(
            cases.len(),
            SlotRejectedConstraint::ALL.len(),
            "every rejected constraint has a case"
        );
        for (constraint, property) in cases {
            let keyword = constraint.keyword();
            let value = property[keyword].clone();
            let slot = slot_property(property.clone());
            assert!(
                slot.get(keyword).is_none(),
                "{keyword} must be removed: {slot}"
            );
            let mut expected = property;
            expected.as_object_mut().unwrap().remove(keyword);
            expected[DESCRIPTION] = restated(&format!(r#"{{"{keyword}":{value}}}"#));
            assert_eq!(slot, expected, "{keyword}");
        }
    }

    #[test]
    fn numeric_bounds_are_removed_beside_any_of_ref_and_on_type_arrays() {
        let schema = json!({
            "type": "object",
            "properties": {
                "type_array": {"type": ["integer", "null"], "format": "uint32", "minimum": 0},
                "beside_any_of": {"anyOf": [{"type": "number"}, {"type": "null"}], "minimum": 0},
                "beside_ref": {"$ref": "#/$defs/Score", "maximum": 1},
                "in_all_of": {"allOf": [{"type": "number", "minimum": 0}]}
            },
            "required": ["type_array", "beside_any_of", "beside_ref", "in_all_of"],
            "additionalProperties": false,
            "$defs": {"Score": {"type": "number", "minimum": 0}}
        });
        let slot = lower_and_check(&schema);
        assert_eq!(
            slot["properties"]["type_array"],
            json!({"type": ["integer", "null"], "format": "uint32",
                   "description": restated(r#"{"minimum":0}"#)}),
            "schemars-style unsigned fields keep their integer format"
        );
        assert_eq!(
            slot["properties"]["beside_any_of"],
            json!({"anyOf": [{"type": "number"}, {"type": "null"}],
                   "description": restated(r#"{"minimum":0}"#)})
        );
        assert_eq!(
            slot["properties"]["beside_ref"],
            json!({"$ref": "#/$defs/Score", "description": restated(r#"{"maximum":1}"#)})
        );
        assert_eq!(
            slot["properties"]["in_all_of"],
            json!({"allOf": [{"type": "number", "description": restated(r#"{"minimum":0}"#)}]})
        );
        assert_eq!(
            slot["$defs"]["Score"],
            json!({"type": "number", "description": restated(r#"{"minimum":0}"#)}),
            "definitions are lowered too, referenced or not"
        );
    }

    #[test]
    fn min_items_zero_and_one_are_kept_other_values_are_removed() {
        for kept in [0, 1] {
            let property = json!({"type": "array", "items": {"type": "string"}, "minItems": kept});
            assert_eq!(slot_property(property.clone()), property, "minItems {kept}");
        }
        for removed in [json!(2), json!(5), json!(1.5)] {
            let slot = slot_property(json!({
                "type": "array", "items": {"type": "string"}, "minItems": removed
            }));
            assert_eq!(
                slot,
                json!({"type": "array", "items": {"type": "string"},
                       "description": restated(&format!(r#"{{"minItems":{removed}}}"#))})
            );
        }
    }

    #[test]
    fn unsupported_string_formats_are_removed_supported_and_non_string_formats_are_kept() {
        for format in SLOT_ACCEPTED_STRING_FORMATS {
            let property = json!({"type": "string", "format": format});
            assert_eq!(slot_property(property.clone()), property, "{format}");
        }
        for format in [
            "int64",
            "uri-reference",
            "regex",
            "idn-email",
            "iri",
            "json-pointer",
            "",
        ] {
            for instance_type in [json!("string"), json!(["string", "null"])] {
                let slot = slot_property(json!({"type": instance_type, "format": format}));
                assert_eq!(
                    slot,
                    json!({"type": instance_type,
                           "description": restated(&format!(r#"{{"format":"{format}"}}"#))}),
                    "{format} on {instance_type}"
                );
            }
        }
        for (instance_type, format) in [("integer", "int32"), ("number", "double")] {
            let property = json!({"type": instance_type, "format": format});
            assert_eq!(slot_property(property.clone()), property, "{format}");
        }
    }

    #[test]
    fn one_of_becomes_any_of_and_its_members_are_lowered() {
        let slot = slot_property(json!({
            "oneOf": [{"type": "number", "minimum": 0}, {"type": "string"}]
        }));
        assert_eq!(
            slot,
            json!({"anyOf": [
                {"type": "number", "description": restated(r#"{"minimum":0}"#)},
                {"type": "string"}
            ]})
        );
    }

    #[test]
    fn one_of_beside_any_of_is_left_for_the_provider_to_reject() {
        let property = json!({
            "anyOf": [{"type": "string"}, {"type": "integer", "maximum": 5}],
            "oneOf": [{"type": "string"}, {"type": "integer", "maximum": 5}]
        });
        let slot = slot_property(property);
        assert_eq!(
            slot["oneOf"],
            json!([{"type": "string"}, {"type": "integer", "maximum": 5}]),
            "oneOf members are never relaxed: that could make a value match two alternatives"
        );
        assert_eq!(
            slot["anyOf"][1],
            json!({"type": "integer", "description": restated(r#"{"maximum":5}"#)})
        );
    }

    #[test]
    fn accepted_keywords_and_literal_data_are_never_touched() {
        let schema = json!({
            "type": "object",
            "title": "Review",
            "description": "A review.",
            "properties": {
                "minimum": {"type": "number"},
                "format": {"type": "string", "minLength": 1, "maxLength": 80, "pattern": "^[a-z]+$"},
                "not": {"type": "string", "enum": ["minimum", "maximum"], "default": "minimum"},
                "maxItems": {"const": "uniqueItems", "examples": ["multipleOf"]},
                "tags": {"type": "array", "items": {"type": "string", "format": "date"},
                         "minItems": 1, "minContains": 1, "maxContains": 2},
                "when": {"type": ["string", "null"], "format": "date-time"},
                "either": {"anyOf": [{"type": "string"}, {"type": "null"}]},
                "both": {"allOf": [{"type": "string"}, {"type": "string", "enum": ["a"]}]},
                "conditional": {"type": "number", "if": {"minimum": 0}, "then": {"maximum": 1}},
                "ref": {"$ref": "#/$defs/Name"},
                "flag": {"type": "boolean", "readOnly": true, "deprecated": true, "$comment": "c"}
            },
            "required": ["minimum", "format", "not", "maxItems"],
            "additionalProperties": false,
            "$defs": {"Name": {"type": "string", "minLength": 1}}
        });
        assert_eq!(
            lower_and_check(&schema),
            schema,
            "a schema without rejected keywords lowers to itself, byte for byte"
        );
    }

    #[test]
    fn keywords_the_provider_rejects_but_cannot_be_relaxed_away_are_kept() {
        let schema = json!({
            "type": "object",
            "properties": {
                "map": {"type": "object", "properties": {}, "required": [],
                        "additionalProperties": false, "patternProperties": {"^x": {"type": "string"}}},
                "tuple": {"type": "array", "prefixItems": [{"type": "string"}], "items": false},
                "open": {"type": "object", "properties": {}, "required": [],
                         "additionalProperties": {"type": "string"}},
                "anything": {},
                "string_rule_beside_any_of": {"anyOf": [{"type": "string"}, {"type": "null"}],
                                              "minLength": 1}
            },
            "required": ["map", "tuple", "open", "anything", "string_rule_beside_any_of"],
            "additionalProperties": false
        });
        assert_eq!(
            lower_and_check(&schema),
            schema,
            "removing these would narrow what the model may produce, so the provider keeps rejecting them"
        );
    }

    #[test]
    fn restated_constraints_extend_create_or_skip_the_description() {
        assert_eq!(
            slot_property(json!({"type": "number", "description": "Confidence.", "maximum": 1})),
            json!({"type": "number",
                   "description": format!("Confidence.\n\n{RESTATED_CONSTRAINTS_LABEL}{{\"maximum\":1}}")})
        );
        assert_eq!(
            slot_property(json!({"type": "number", "description": "", "maximum": 1})),
            json!({"type": "number", "description": restated(r#"{"maximum":1}"#)})
        );
        assert_eq!(
            slot_property(json!({"type": "number", "description": 7, "maximum": 1})),
            json!({"type": "number", "description": 7}),
            "a malformed description is left as written"
        );
    }

    #[test]
    fn lowering_never_modifies_the_validation_schema() {
        let validation = closed(json!({"type": "number", "minimum": 0, "maximum": 1}));
        let before = validation.clone();
        let slot = lower_output_format_schema(&validation);
        assert_eq!(validation, before);
        assert_ne!(slot, validation);
        assert_eq!(validation["properties"]["x"]["minimum"], json!(0));
        assert_eq!(validation["properties"]["x"]["maximum"], json!(1));
    }

    #[test]
    fn lowering_is_idempotent() {
        let validation = closed(json!({
            "type": "array",
            "items": {"type": "integer", "minimum": 1},
            "minItems": 2,
            "uniqueItems": true
        }));
        let once = lower_and_check(&validation);
        assert_eq!(lower_output_format_schema(&once), once);
    }

    #[test]
    #[should_panic(expected = "keyword removed that is not relaxable")]
    fn relaxation_check_rejects_removing_a_narrowing_keyword() {
        // Dropping `patternProperties` beside `additionalProperties: false`
        // would force the model to emit `{}`; the check must refuse it.
        let validation = closed(json!({
            "type": "object", "properties": {}, "required": [],
            "additionalProperties": false, "patternProperties": {"^x": {"type": "string"}}
        }));
        let mut narrowed = validation.clone();
        narrowed["properties"]["x"]
            .as_object_mut()
            .unwrap()
            .remove("patternProperties");
        assert_only_relaxes(&validation, &narrowed, "#");
    }
}
