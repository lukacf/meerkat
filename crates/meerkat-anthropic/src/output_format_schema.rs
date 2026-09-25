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
//! The lowering removes a keyword only when all three hold:
//!
//! 1. Anthropic rejects it (the documented "Not supported" list for structured
//!    outputs, confirmed keyword by keyword against the live API for number,
//!    integer, string, array, object and composition nodes on 2026-09-25 with
//!    claude-haiku-4-5, claude-sonnet-4-6 and claude-opus-5-5, which all
//!    answered identically).
//! 2. Removing it only relaxes the schema, so every value the validation
//!    schema accepts is still accepted by the slot. The grammar can then never
//!    force the model away from a value meerkat would accept.
//! 3. meerkat's reply validator enforces it for the validation schema's JSON
//!    Schema draft (see [`ValidationDraft`]), so anything the removed keyword
//!    forbade is still caught by validation and retried. `discriminator` is
//!    the one keyword removed without this: it is an OpenAPI annotation that
//!    forbids nothing, so removing it changes no accepted value.
//!
//! Keywords Anthropic rejects that fail one of these tests are left in place,
//! so the provider keeps failing loudly instead of meerkat silently narrowing
//! the output or silently dropping a constraint:
//!
//! - `patternProperties`, `prefixItems`, array-form `items`,
//!   `additionalProperties` other than `false`, empty `{}` schemas, enums with
//!   object or array members, and unsupported regex features in `pattern`:
//!   removing them would narrow what the slot accepts.
//! - `format`. Anthropic rejects string formats outside its list, but the
//!   validator's default draft (2020-12) treats `format` as an annotation, so
//!   a format removed from the slot would be enforced nowhere.
//! - A keyword the declared draft does not define, for example
//!   `dependentRequired` in a schema whose `$schema` is draft-07: the
//!   validator ignores it there. A `$schema` the validator does not recognize
//!   turns the lowering off.
//!
//! String keywords (`minLength`, `pattern`, `format`, ...) that sit directly
//! beside `anyOf` or `$ref` instead of inside the typed schema are rejected
//! as well and are not lowered either. `minLength`, `maxLength` and `pattern`
//! there would pass all three tests; `format` would fail the third.
//!
//! Keywords Anthropic accepts are never touched. That includes `minLength`,
//! `maxLength` and `pattern`: the documentation lists string length as
//! unsupported, but the API accepts both and the grammar enforces them.
//!
//! Each removed keyword is restated in the node's `description`, as
//! Anthropic's own SDKs do, so the model still reads it in the format
//! instructions Anthropic derives from the slot. A schema that uses none of
//! the rejected keywords lowers to itself.
//!
//! The lowering adds no `SchemaWarning` and does not read
//! `OutputSchema::compat`: validation still enforces everything it removes.
//!
//! [`LlmClient::compile_schema`]: meerkat_llm_core::LlmClient::compile_schema

use serde_json::{Map, Value};

/// A keyword Anthropic's structured-output grammar compiler rejects wherever
/// it appears and whose removal only relaxes the schema.
///
/// Grouped by the instance type the keyword constrains. The keyword is
/// removed from any node, whatever its declared `type`: on a node of another
/// type it constrains nothing, so removing it there changes no accepted value
/// either, and the API rejects these keywords beside `anyOf`, beside `$ref`
/// and on type arrays such as `["number", "null"]` as well. A keyword is only
/// removed where [`Self::removable_under`] holds for the validation draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotRejectedKeyword {
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
    /// OpenAPI's discriminator hint, which pydantic emits beside the `oneOf`
    /// of a discriminated union. The slot rejects it beside `anyOf`, which is
    /// where the `oneOf` rewrite leaves it. No JSON Schema draft asserts it.
    Discriminator,
}

impl SlotRejectedKeyword {
    /// Every rejected keyword, in the order removed keywords are restated in
    /// a node's description.
    const ALL: [Self; 17] = [
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
        Self::Discriminator,
    ];

    /// The JSON Schema keyword this is spelled as.
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
            Self::Discriminator => "discriminator",
        }
    }

    /// Whether removing the keyword leaves nothing unenforced when the
    /// validator applies `draft`: the validator asserts the keyword in that
    /// draft, or the keyword asserts nothing in any draft.
    const fn removable_under(self, draft: ValidationDraft) -> bool {
        match self {
            Self::Minimum
            | Self::Maximum
            | Self::MultipleOf
            | Self::MaxItems
            | Self::UniqueItems
            | Self::MinProperties
            | Self::MaxProperties
            | Self::Dependencies
            | Self::Not => true,
            // Draft-04 spells these as boolean modifiers of `minimum` /
            // `maximum` and ignores the numeric form, so they are kept there.
            Self::ExclusiveMinimum | Self::ExclusiveMaximum => {
                !matches!(draft, ValidationDraft::Draft4)
            }
            // Introduced in draft-06.
            Self::Contains | Self::PropertyNames => !matches!(draft, ValidationDraft::Draft4),
            // Introduced in draft 2019-09.
            Self::DependentRequired | Self::DependentSchemas | Self::UnevaluatedProperties => {
                matches!(
                    draft,
                    ValidationDraft::Draft201909 | ValidationDraft::Draft202012
                )
            }
            Self::Discriminator => true,
        }
    }
}

/// The JSON Schema draft meerkat's reply validator applies to a validation
/// schema.
///
/// The validator (the `jsonschema` crate, as `meerkat-core` uses it) takes
/// the draft from the root `$schema`: one of the draft URIs in
/// [`Self::DECLARATIONS`], with or without a trailing `#`. A missing or
/// non-string `$schema` means draft 2020-12, and the validator refuses any
/// other string. A test checks this against the validator's own detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValidationDraft {
    Draft4,
    Draft6,
    Draft7,
    Draft201909,
    Draft202012,
}

impl ValidationDraft {
    const DECLARATIONS: [(&'static str, Self); 5] = [
        (
            "https://json-schema.org/draft/2020-12/schema",
            Self::Draft202012,
        ),
        (
            "https://json-schema.org/draft/2019-09/schema",
            Self::Draft201909,
        ),
        ("http://json-schema.org/draft-07/schema", Self::Draft7),
        ("http://json-schema.org/draft-06/schema", Self::Draft6),
        ("http://json-schema.org/draft-04/schema", Self::Draft4),
    ];

    /// The draft the validator applies to `validation_schema`, or `None` when
    /// its `$schema` names a dialect the validator refuses.
    fn of(validation_schema: &Value) -> Option<Self> {
        let Some(declared) = validation_schema
            .get(SCHEMA_DIALECT)
            .and_then(Value::as_str)
        else {
            return Some(Self::Draft202012);
        };
        let declared = declared.trim_end_matches('#');
        Self::DECLARATIONS
            .iter()
            .find_map(|(uri, draft)| (*uri == declared).then_some(*draft))
    }
}

/// `minItems` values the slot accepts. Any other value is rejected and is
/// removed (a lower bound only ever narrows the accepted arrays, and every
/// draft asserts `minItems`).
const SLOT_ACCEPTED_MIN_ITEMS: [u64; 2] = [0, 1];

const SCHEMA_DIALECT: &str = "$schema";
const MIN_ITEMS: &str = "minItems";
const DESCRIPTION: &str = "description";
const ONE_OF: &str = "oneOf";
const ANY_OF: &str = "anyOf";

/// Label that introduces restated constraints in a node's description.
const RESTATED_CONSTRAINTS_LABEL: &str = "Constraints (JSON Schema): ";

/// Lower a validation schema into the schema sent in Anthropic's
/// `output_config.format` slot.
///
/// See the module documentation for the rules. The input is never modified;
/// the caller keeps validating against it. A validation schema whose
/// `$schema` the validator refuses is sent as it is.
pub(crate) fn lower_output_format_schema(validation_schema: &Value) -> Value {
    let mut slot = validation_schema.clone();
    if let Some(draft) = ValidationDraft::of(validation_schema) {
        lower_schema_node(&mut slot, draft);
    }
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
fn lower_schema_node(node: &mut Value, draft: ValidationDraft) {
    // Boolean schemas carry no keywords.
    let Value::Object(schema) = node else {
        return;
    };

    let mut restated: Vec<(&'static str, Value)> = Vec::new();
    for rejected in SlotRejectedKeyword::ALL {
        if rejected.removable_under(draft)
            && let Some(value) = schema.remove(rejected.keyword())
        {
            restated.push((rejected.keyword(), value));
        }
    }
    if schema
        .get(MIN_ITEMS)
        .is_some_and(|min_items| !is_slot_accepted_min_items(min_items))
        && let Some(value) = schema.remove(MIN_ITEMS)
    {
        restated.push((MIN_ITEMS, value));
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
            for child in children.values_mut() {
                lower_schema_node(child, draft);
            }
        }
    }
    if let Some(items @ Value::Object(_)) = schema.get_mut("items") {
        lower_schema_node(items, draft);
    }
    for key in ["allOf", ANY_OF] {
        // An `anyOf` that stays beside an un-rewritten `oneOf` is still
        // walked: relaxing an `anyOf` member relaxes the node. The `oneOf`
        // members themselves are not, because relaxing a `oneOf` member can
        // make a value match two alternatives and so be rejected.
        if let Some(Value::Array(members)) = schema.get_mut(key) {
            for member in members {
                lower_schema_node(member, draft);
            }
        }
    }
}

fn is_slot_accepted_min_items(min_items: &Value) -> bool {
    min_items
        .as_u64()
        .is_some_and(|count| SLOT_ACCEPTED_MIN_ITEMS.contains(&count))
}

/// Append the removed keywords to the node's description as one compact
/// JSON object, in [`SlotRejectedKeyword::ALL`] order followed by
/// `minItems`. A non-string `description` is left alone.
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
    /// `format` is not one of them: see
    /// `string_formats_are_never_lowered_because_validation_does_not_assert_them`.
    fn removable_keyword(keyword: &str) -> bool {
        SlotRejectedKeyword::ALL
            .iter()
            .any(|rejected| rejected.keyword() == keyword)
            || keyword == MIN_ITEMS
    }

    /// Structural proof that `slot` only relaxes `validation`.
    ///
    /// Walks both trees in the positions the lowering walks and accepts
    /// exactly three kinds of difference: a removed keyword from the
    /// rejected set (or `minItems`), `oneOf` renamed to
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
                SlotRejectedKeyword::Minimum,
                json!({"type": "number", "minimum": 0.5}),
            ),
            (
                SlotRejectedKeyword::Maximum,
                json!({"type": "integer", "maximum": 3}),
            ),
            (
                SlotRejectedKeyword::ExclusiveMinimum,
                json!({"type": "number", "exclusiveMinimum": 0}),
            ),
            (
                SlotRejectedKeyword::ExclusiveMaximum,
                json!({"type": "integer", "exclusiveMaximum": 9}),
            ),
            (
                SlotRejectedKeyword::MultipleOf,
                json!({"type": "number", "multipleOf": 0.25}),
            ),
            (
                SlotRejectedKeyword::MaxItems,
                json!({"type": "array", "items": {"type": "string"}, "maxItems": 3}),
            ),
            (
                SlotRejectedKeyword::UniqueItems,
                json!({"type": "array", "items": {"type": "string"}, "uniqueItems": false}),
            ),
            (
                SlotRejectedKeyword::Contains,
                json!({"type": "array", "items": {"type": "string"}, "contains": {"const": "a"}}),
            ),
            (
                SlotRejectedKeyword::MinProperties,
                json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": [],
                       "additionalProperties": false, "minProperties": 1}),
            ),
            (
                SlotRejectedKeyword::MaxProperties,
                json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": [],
                       "additionalProperties": false, "maxProperties": 1}),
            ),
            (
                SlotRejectedKeyword::PropertyNames,
                json!({"type": "object", "properties": {}, "required": [],
                       "additionalProperties": false, "propertyNames": {"pattern": "^x"}}),
            ),
            (
                SlotRejectedKeyword::DependentRequired,
                json!({"type": "object", "properties": {"a": {"type": "string"}, "b": {"type": "string"}},
                       "required": [], "additionalProperties": false,
                       "dependentRequired": {"a": ["b"]}}),
            ),
            (
                SlotRejectedKeyword::DependentSchemas,
                json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": [],
                       "additionalProperties": false,
                       "dependentSchemas": {"a": {"required": ["a"]}}}),
            ),
            (
                SlotRejectedKeyword::Dependencies,
                json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": [],
                       "additionalProperties": false, "dependencies": {"a": ["a"]}}),
            ),
            (
                SlotRejectedKeyword::UnevaluatedProperties,
                json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"],
                       "additionalProperties": false, "unevaluatedProperties": false}),
            ),
            (
                SlotRejectedKeyword::Not,
                json!({"type": "string", "not": {"const": "x"}}),
            ),
            (
                SlotRejectedKeyword::Discriminator,
                json!({"anyOf": [{"type": "string"}, {"type": "integer"}],
                       "discriminator": {"propertyName": "kind"}}),
            ),
        ];
        assert_eq!(
            cases.len(),
            SlotRejectedKeyword::ALL.len(),
            "every rejected constraint has a case"
        );
        for (rejected, property) in cases {
            let keyword = rejected.keyword();
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

    /// String formats outside Anthropic's list (`json-pointer`, `iri`,
    /// `idn-email`, `regex`, `uri-reference`, `uri-template`,
    /// `idn-hostname`, ...) get HTTP 400 from the slot. They stay in it
    /// anyway: the reply validator treats `format` as an annotation under its
    /// default draft, so removing a format would leave it enforced nowhere
    /// and turn a loud failure into a silent pass.
    #[test]
    fn string_formats_are_never_lowered_because_validation_does_not_assert_them() {
        for format in [
            "date-time",
            "email",
            "uuid",
            "json-pointer",
            "relative-json-pointer",
            "uri-reference",
            "uri-template",
            "iri",
            "iri-reference",
            "idn-email",
            "idn-hostname",
            "regex",
            "int64",
            "",
        ] {
            for instance_type in [json!("string"), json!(["string", "null"])] {
                let property = json!({"type": instance_type, "format": format});
                assert_eq!(
                    slot_property(property.clone()),
                    property,
                    "{format} on {instance_type}"
                );
            }
        }
        for (instance_type, format) in [("integer", "int32"), ("number", "double")] {
            let property = json!({"type": instance_type, "format": format});
            assert_eq!(slot_property(property.clone()), property, "{format}");
        }
        // Beside another removed keyword the format still stays.
        assert_eq!(
            slot_property(
                json!({"type": "string", "format": "json-pointer", "not": {"const": ""}})
            ),
            json!({"type": "string", "format": "json-pointer",
                   "description": restated(r#"{"not":{"const":""}}"#)})
        );
    }

    /// Pydantic's discriminated union: `oneOf` over `$ref`s with an OpenAPI
    /// `discriminator` beside it. After the `oneOf` rewrite the slot would
    /// reject `discriminator` beside `anyOf`, so it is removed as well.
    #[test]
    fn pydantic_discriminated_union_fits_the_slot() {
        let validation = json!({
            "$defs": {
                "Cat": {"properties": {"kind": {"const": "cat", "title": "Kind", "type": "string"},
                                       "meows": {"title": "Meows", "type": "integer"}},
                        "required": ["kind", "meows"], "title": "Cat", "type": "object",
                        "additionalProperties": false},
                "Dog": {"properties": {"kind": {"const": "dog", "title": "Kind", "type": "string"},
                                       "barks": {"title": "Barks", "type": "integer"}},
                        "required": ["kind", "barks"], "title": "Dog", "type": "object",
                        "additionalProperties": false}
            },
            "properties": {
                "pet": {
                    "discriminator": {"mapping": {"cat": "#/$defs/Cat", "dog": "#/$defs/Dog"},
                                      "propertyName": "kind"},
                    "oneOf": [{"$ref": "#/$defs/Cat"}, {"$ref": "#/$defs/Dog"}],
                    "title": "Pet"
                }
            },
            "required": ["pet"], "title": "M", "type": "object", "additionalProperties": false
        });
        let slot = lower_and_check(&validation);
        assert_eq!(
            slot["properties"]["pet"],
            json!({
                "anyOf": [{"$ref": "#/$defs/Cat"}, {"$ref": "#/$defs/Dog"}],
                "title": "Pet",
                "description": restated(
                    r##"{"discriminator":{"mapping":{"cat":"#/$defs/Cat","dog":"#/$defs/Dog"},"propertyName":"kind"}}"##
                )
            })
        );
        assert_eq!(slot["$defs"], validation["$defs"]);
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

    const DRAFTS: [ValidationDraft; 5] = [
        ValidationDraft::Draft4,
        ValidationDraft::Draft6,
        ValidationDraft::Draft7,
        ValidationDraft::Draft201909,
        ValidationDraft::Draft202012,
    ];

    fn declared_uri(draft: ValidationDraft) -> &'static str {
        ValidationDraft::DECLARATIONS
            .iter()
            .find_map(|(uri, declared)| (*declared == draft).then_some(*uri))
            .unwrap()
    }

    /// `closed(property)` with the draft declared as `$schema`, or with no
    /// `$schema` (the validator's default, 2020-12) when `draft` is `None`.
    fn closed_under(property: Value, draft: Option<ValidationDraft>) -> Value {
        let mut schema = closed(property);
        if let Some(draft) = draft {
            schema[SCHEMA_DIALECT] = json!(format!("{}#", declared_uri(draft)));
        }
        schema
    }

    #[test]
    fn keywords_the_declared_draft_does_not_define_are_kept() {
        let property = json!({
            "type": "object",
            "properties": {"a": {"type": "string"}, "b": {"type": "string"}},
            "required": [],
            "additionalProperties": false,
            "dependentRequired": {"a": ["b"]},
            "dependentSchemas": {"a": {"required": ["b"]}},
            "unevaluatedProperties": false,
            "maxProperties": 1
        });
        for draft in [
            ValidationDraft::Draft4,
            ValidationDraft::Draft6,
            ValidationDraft::Draft7,
        ] {
            let slot = lower_and_check(&closed_under(property.clone(), Some(draft)));
            let mut expected = property.clone();
            expected.as_object_mut().unwrap().remove("maxProperties");
            expected[DESCRIPTION] = restated(r#"{"maxProperties":1}"#);
            assert_eq!(slot["properties"]["x"], expected, "{draft:?}");
        }
        for draft in [
            None,
            Some(ValidationDraft::Draft201909),
            Some(ValidationDraft::Draft202012),
        ] {
            let slot = lower_and_check(&closed_under(property.clone(), draft));
            assert_eq!(
                slot["properties"]["x"],
                json!({
                    "type": "object",
                    "properties": {"a": {"type": "string"}, "b": {"type": "string"}},
                    "required": [],
                    "additionalProperties": false,
                    "description": restated(
                        r#"{"maxProperties":1,"dependentRequired":{"a":["b"]},"dependentSchemas":{"a":{"required":["b"]}},"unevaluatedProperties":false}"#
                    )
                }),
                "{draft:?}"
            );
        }

        let draft4 = closed_under(
            json!({"type": "array", "items": {"type": "number", "minimum": 0, "exclusiveMinimum": true},
                   "contains": {"type": "number"}, "maxItems": 3}),
            Some(ValidationDraft::Draft4),
        );
        assert_eq!(
            lower_and_check(&draft4)["properties"]["x"],
            json!({"type": "array",
                   "items": {"type": "number", "exclusiveMinimum": true,
                             "description": restated(r#"{"minimum":0}"#)},
                   "contains": {"type": "number"},
                   "description": restated(r#"{"maxItems":3}"#)}),
            "draft-04 keeps its boolean exclusive bounds and `contains`"
        );
    }

    #[test]
    fn a_schema_dialect_the_validator_refuses_is_not_lowered() {
        for dialect in [
            "https://json-schema.org/draft/2021-01/schema",
            "http://json-schema.org/schema#",
            "https://json-schema.org/draft-07/schema",
        ] {
            let mut schema = closed(json!({"type": "number", "minimum": 0}));
            schema[SCHEMA_DIALECT] = json!(dialect);
            assert_eq!(lower_output_format_schema(&schema), schema, "{dialect}");
        }
    }

    fn to_validator_draft(draft: ValidationDraft) -> jsonschema::Draft {
        match draft {
            ValidationDraft::Draft4 => jsonschema::Draft::Draft4,
            ValidationDraft::Draft6 => jsonschema::Draft::Draft6,
            ValidationDraft::Draft7 => jsonschema::Draft::Draft7,
            ValidationDraft::Draft201909 => jsonschema::Draft::Draft201909,
            ValidationDraft::Draft202012 => jsonschema::Draft::Draft202012,
        }
    }

    /// The draft table agrees with the validator's own draft detection,
    /// including a missing, non-string and unrecognized `$schema`.
    #[test]
    fn validation_draft_matches_the_validator_detection() {
        let mut dialects: Vec<Option<Value>> = vec![
            None,
            Some(json!(7)),
            Some(json!("https://json-schema.org/draft/2021-01/schema")),
            Some(json!("http://json-schema.org/schema#")),
            Some(json!("")),
        ];
        for (uri, _) in ValidationDraft::DECLARATIONS {
            dialects.push(Some(json!(uri)));
            dialects.push(Some(json!(format!("{uri}#"))));
        }
        for dialect in dialects {
            let mut schema = json!({"type": "object"});
            if let Some(dialect) = &dialect {
                schema[SCHEMA_DIALECT] = dialect.clone();
            }
            assert_eq!(
                ValidationDraft::of(&schema).map(to_validator_draft),
                jsonschema::Draft::default().detect(&schema).ok(),
                "$schema {dialect:?}"
            );
        }
    }

    fn validator(schema: &Value) -> jsonschema::Validator {
        // The same construction `meerkat-core` uses to validate a reply.
        jsonschema::Validator::new(schema).expect("validation schema compiles")
    }

    /// Every keyword the lowering removes is still enforced by the reply
    /// validator, in every draft the validator supports. Each case holds a
    /// value that breaks exactly one keyword. Where the lowering removes the
    /// keyword, the slot must accept the value (the keyword was the only
    /// thing forbidding it) and the validation schema must reject it.
    #[test]
    fn every_removed_keyword_is_still_enforced_by_the_reply_validator() {
        let pair = || json!({"a": {"type": "string"}, "b": {"type": "string"}});
        let cases: Vec<(&str, Value, Value)> = vec![
            (
                "minimum",
                json!({"type": "number", "minimum": 0}),
                json!(-1),
            ),
            (
                "maximum",
                json!({"type": "number", "maximum": 1}),
                json!(95),
            ),
            (
                "exclusiveMinimum",
                json!({"type": "number", "exclusiveMinimum": 0}),
                json!(0),
            ),
            (
                "exclusiveMaximum",
                json!({"type": "number", "exclusiveMaximum": 1}),
                json!(1),
            ),
            (
                "multipleOf",
                json!({"type": "integer", "multipleOf": 2}),
                json!(3),
            ),
            (
                "maxItems",
                json!({"type": "array", "items": {"type": "string"}, "maxItems": 1}),
                json!(["a", "b"]),
            ),
            (
                "uniqueItems",
                json!({"type": "array", "items": {"type": "string"}, "uniqueItems": true}),
                json!(["a", "a"]),
            ),
            (
                "contains",
                json!({"type": "array", "items": {"type": "string"}, "contains": {"enum": ["a"]}}),
                json!(["b"]),
            ),
            (
                "minItems",
                json!({"type": "array", "items": {"type": "string"}, "minItems": 2}),
                json!(["a"]),
            ),
            (
                "minProperties",
                json!({"type": "object", "properties": pair(), "minProperties": 1}),
                json!({}),
            ),
            (
                "maxProperties",
                json!({"type": "object", "properties": pair(), "maxProperties": 1}),
                json!({"a": "1", "b": "2"}),
            ),
            (
                "propertyNames",
                json!({"type": "object", "properties": {"ab": {"type": "string"}},
                       "propertyNames": {"maxLength": 1}}),
                json!({"ab": "1"}),
            ),
            (
                "dependentRequired",
                json!({"type": "object", "properties": pair(), "dependentRequired": {"a": ["b"]}}),
                json!({"a": "1"}),
            ),
            (
                "dependentSchemas",
                json!({"type": "object", "properties": pair(),
                       "dependentSchemas": {"a": {"required": ["b"]}}}),
                json!({"a": "1"}),
            ),
            (
                "dependencies",
                json!({"type": "object", "properties": pair(), "dependencies": {"a": ["b"]}}),
                json!({"a": "1"}),
            ),
            (
                "unevaluatedProperties",
                json!({"type": "object", "properties": pair(), "unevaluatedProperties": false}),
                json!({"z": "1"}),
            ),
            (
                "not",
                json!({"type": "string", "not": {"enum": ["x"]}}),
                json!("x"),
            ),
            (
                "oneOf",
                json!({"oneOf": [{"type": "integer"}, {"type": "number"}]}),
                json!(1),
            ),
            (
                "format",
                json!({"type": "string", "format": "json-pointer"}),
                json!("no-leading-slash"),
            ),
            (
                "format",
                json!({"type": "string", "format": "regex"}),
                json!("["),
            ),
            (
                "format",
                json!({"type": "string", "format": "idn-email"}),
                json!("not an email"),
            ),
        ];
        let rejected_keywords: Vec<&str> = SlotRejectedKeyword::ALL
            .iter()
            .filter(|rejected| **rejected != SlotRejectedKeyword::Discriminator)
            .map(|rejected| rejected.keyword())
            .collect();
        assert!(
            rejected_keywords
                .iter()
                .all(|keyword| cases.iter().any(|(case, _, _)| case == keyword)),
            "every rejected keyword that asserts something has a case"
        );

        let mut removals = 0;
        for draft in std::iter::once(None).chain(DRAFTS.map(Some)) {
            for (keyword, property, value) in &cases {
                let validation = closed_under(property.clone(), draft);
                let slot = lower_output_format_schema(&validation);
                let instance = json!({"x": value});
                let removed = slot["properties"]["x"].get(*keyword).is_none();
                if let Some(rejected) = SlotRejectedKeyword::ALL
                    .iter()
                    .find(|rejected| rejected.keyword() == *keyword)
                {
                    assert_eq!(
                        removed,
                        rejected.removable_under(draft.unwrap_or(ValidationDraft::Draft202012)),
                        "{keyword} under {draft:?}"
                    );
                }
                if !removed {
                    continue;
                }
                removals += 1;
                assert!(
                    validator(&slot).is_valid(&instance),
                    "{keyword} under {draft:?}: the case must break only the removed keyword"
                );
                assert!(
                    !validator(&validation).is_valid(&instance),
                    "{keyword} removed under {draft:?}, but the reply validator does not enforce it"
                );
            }
        }
        assert!(removals > 0);

        // The premise for keeping `format`: without a `$schema`, the reply
        // validator does not assert formats.
        let unasserted = closed(json!({"type": "string", "format": "json-pointer"}));
        assert!(validator(&unasserted).is_valid(&json!({"x": "no-leading-slash"})));
    }
}
