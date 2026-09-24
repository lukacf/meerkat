//! Tool-parameter schema normalization shared by every OpenAI emission site.
//!
//! OpenAI's non-strict function-parameter validator rejects a schema whose
//! ROOT carries a JSON Schema combinator or literal constraint, with
//! `invalid_function_parameters`: `schema must have type 'object' and not have
//! 'oneOf'/'anyOf'/'allOf'/'enum'/'not' at the top level`. This is observable
//! on Chat Completions (the `openai_compatible` transport) today and was
//! observable on the Responses API before mid-2026. Nested combinators are
//! accepted, so this pass deliberately does not recurse into them: rewriting
//! MCP or user tool schemas below the root would be lossy on a provider that
//! does not need it.
//!
//! All five keywords the validator names are handled at the root: `not` and
//! `enum` are removed, and `allOf`/`oneOf`/`anyOf` are folded into the root.
//! The root must end as `"type": "object"`: a root without a type is declared
//! one (function arguments are always a JSON object on every provider wire),
//! while a root that declares a non-object type is a tool-definition defect no
//! provider can accept and is refused with a typed [`LlmError::InvalidRequest`]
//! naming the tool, rather than emitted as a body the provider will reject.
//!
//! The pass also inlines local `$ref`s and drops the definitions block once
//! nothing references it any more, so OpenAI-compatible servers whose
//! validators do not resolve references receive a self-contained schema. Note
//! that inlining duplicates a definition referenced from several places.
//!
//! Every OpenAI emission site (Responses tools, Chat Completions tools, the
//! realtime text adapter and the live session tools) routes through
//! [`normalize_openai_tool_parameters_schema`]. The root fold runs on the
//! Responses path as well, by decision: the Responses validator's current
//! tolerance of root combinators is undocumented and the two OpenAI request
//! builders are meant to emit one shape. A schema that needs no rewrite is
//! returned borrowed, so the common case costs one read-only walk and no
//! allocation per tool per request.

use std::borrow::Cow;

use meerkat_llm_core::LlmError;
use serde_json::{Map, Value};

/// Keywords OpenAI's function-parameter validator rejects at the schema root
/// (quoted from the validator message in the module documentation).
const ROOT_REJECTED_KEYWORDS: &[&str] = &["not", "oneOf", "anyOf", "allOf", "enum"];

/// Normalize a tool's `input_schema` for OpenAI function-parameter emission.
///
/// - Local `$ref`s (`#/$defs/...`, `#/definitions/...`) are inlined; the
///   definitions blocks are removed once nothing references them. References
///   that cannot be resolved locally are left untouched.
/// - A root-level `not` and a root-level `enum` are removed.
/// - Root-level `allOf`, `oneOf` and `anyOf` are folded into the root: every
///   object member contributes its `properties` and, when the root has none,
///   its object `type`; `required` is unioned for `allOf` and intersected for
///   `oneOf`/`anyOf` (a property is only required when every variant requires
///   it). On a property-name clash the root, then the first variant, wins,
///   except that literal discriminators (`const`/`enum`-only schemas) from
///   several `oneOf`/`anyOf` variants merge into one `enum`, so the model is
///   not steered to a single variant. The dispatcher validates arguments at
///   call time.
/// - A root without a `type` (or with a type array that admits `object`) is
///   declared `"type": "object"`.
/// - Nothing below the root is rewritten apart from `$ref` inlining.
///
/// Returns [`Cow::Borrowed`] when the schema already satisfies every rule.
///
/// # Errors
///
/// [`LlmError::InvalidRequest`] naming `tool_name` when the schema is not a
/// JSON object or its root declares a non-object `type`: OpenAI (like every
/// other provider) requires function parameters to be an object schema, so the
/// tool definition has to be fixed rather than sent.
pub fn normalize_openai_tool_parameters_schema<'a>(
    tool_name: &str,
    schema: &'a Value,
) -> Result<Cow<'a, Value>, LlmError> {
    let Value::Object(root) = schema else {
        return Err(non_object_root_error(tool_name, schema));
    };
    if !needs_rewrite(root, schema) {
        return Ok(Cow::Borrowed(schema));
    }

    let mut normalized = inline_local_schema_refs(schema, schema, &mut Vec::new(), 0);
    if let Value::Object(root) = &mut normalized {
        root.remove("not");
        root.remove("enum");
        fold_root_combinator(root, "allOf", RequiredMerge::Union);
        fold_root_combinator(root, "oneOf", RequiredMerge::Intersection);
        fold_root_combinator(root, "anyOf", RequiredMerge::Intersection);
        declare_object_root(tool_name, root)?;
    }
    if !contains_schema_ref(&normalized)
        && let Value::Object(root) = &mut normalized
    {
        root.remove("$defs");
        root.remove("definitions");
    }
    Ok(Cow::Owned(normalized))
}

/// Whether any rule of the normalizer would change `schema`. Read-only, so a
/// schema that is already in emission shape costs no allocation.
fn needs_rewrite(root: &Map<String, Value>, schema: &Value) -> bool {
    if ROOT_REJECTED_KEYWORDS
        .iter()
        .chain(["$defs", "definitions"].iter())
        .any(|keyword| root.contains_key(*keyword))
    {
        return true;
    }
    if !matches!(root.get("type"), Some(Value::String(kind)) if kind == "object") {
        return true;
    }
    contains_schema_ref(schema)
}

/// Guarantee `"type": "object"` at the root, or refuse the tool.
fn declare_object_root(tool_name: &str, root: &mut Map<String, Value>) -> Result<(), LlmError> {
    let admits_object = match root.get("type") {
        None => true,
        Some(Value::String(kind)) => kind == "object",
        Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind == "object"),
        Some(_) => false,
    };
    if !admits_object {
        let declared = root.get("type").cloned().unwrap_or(Value::Null);
        return Err(non_object_root_error(tool_name, &declared));
    }
    root.insert("type".to_string(), Value::String("object".to_string()));
    Ok(())
}

fn non_object_root_error(tool_name: &str, declared: &Value) -> LlmError {
    LlmError::InvalidRequest {
        message: format!(
            "tool `{tool_name}` declares a parameters schema whose root is {declared}, not a JSON \
             object schema; OpenAI function parameters must be `\"type\": \"object\"` describing \
             the call arguments, so this tool definition has to be fixed before it can be sent"
        ),
    }
}

/// How member `required` lists combine when a root combinator is folded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequiredMerge {
    /// `allOf`: every member applies, so every member's required names apply.
    Union,
    /// `oneOf`/`anyOf`: any single member may apply, so only names required by
    /// every member are guaranteed.
    Intersection,
}

fn fold_root_combinator(root: &mut Map<String, Value>, keyword: &str, merge: RequiredMerge) {
    let Some(Value::Array(members)) = root.remove(keyword) else {
        return;
    };

    let mut merged_required: Option<Vec<Value>> = None;
    for member in members {
        let Value::Object(member) = member else {
            continue;
        };

        let member_required = member
            .get("required")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        merged_required = Some(match (merged_required.take(), merge) {
            (None, _) => member_required,
            (Some(mut accumulated), RequiredMerge::Union) => {
                for name in member_required {
                    if !accumulated.contains(&name) {
                        accumulated.push(name);
                    }
                }
                accumulated
            }
            (Some(accumulated), RequiredMerge::Intersection) => accumulated
                .into_iter()
                .filter(|name| member_required.contains(name))
                .collect(),
        });

        if let Some(Value::Object(member_properties)) = member.get("properties") {
            let parent = root
                .entry("properties")
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(parent_properties) = parent {
                for (name, property_schema) in member_properties {
                    match parent_properties.get_mut(name) {
                        Some(existing) if merge == RequiredMerge::Intersection => {
                            merge_variant_discriminator(existing, property_schema);
                        }
                        Some(_) => {}
                        None => {
                            parent_properties.insert(name.clone(), property_schema.clone());
                        }
                    }
                }
            }
        }

        if !root.contains_key("type")
            && matches!(member.get("type"), Some(Value::String(kind)) if kind == "object")
        {
            root.insert("type".to_string(), Value::String("object".to_string()));
        }
    }

    let Some(merged_required) = merged_required.filter(|names| !names.is_empty()) else {
        return;
    };
    let parent = root
        .entry("required")
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Value::Array(parent_required) = parent {
        for name in merged_required {
            if !parent_required.contains(&name) {
                parent_required.push(name);
            }
        }
    }
}

/// When the same property is a literal discriminator in two `oneOf`/`anyOf`
/// variants (`kind: {const: "session"}` and `kind: {const: "owner"}`), keep
/// every variant's literal in one `enum` instead of pinning the first variant.
/// Anything other than a pure literal schema on either side keeps the existing
/// (first) definition.
fn merge_variant_discriminator(existing: &mut Value, incoming: &Value) {
    let (Some(existing_literals), Some(incoming_literals)) =
        (literal_values(existing), literal_values(incoming))
    else {
        return;
    };
    let Value::Object(existing_obj) = existing else {
        return;
    };
    let types_differ = matches!(
        (existing_obj.get("type"), incoming.get("type")),
        (Some(a), Some(b)) if a != b
    );
    let mut merged = existing_literals;
    for literal in incoming_literals {
        if !merged.contains(&literal) {
            merged.push(literal);
        }
    }
    existing_obj.remove("const");
    existing_obj.insert("enum".to_string(), Value::Array(merged));
    if types_differ {
        existing_obj.remove("type");
    }
}

/// The literals a `const`/`enum`-only schema admits, or `None` when the schema
/// carries any structural keyword beyond `type`, `title` and `description`.
fn literal_values(schema: &Value) -> Option<Vec<Value>> {
    let obj = schema.as_object()?;
    if obj.keys().any(|key| {
        !matches!(
            key.as_str(),
            "const" | "enum" | "type" | "title" | "description"
        )
    }) {
        return None;
    }
    if let Some(literal) = obj.get("const") {
        return Some(vec![literal.clone()]);
    }
    obj.get("enum").and_then(Value::as_array).cloned()
}

fn contains_schema_ref(value: &Value) -> bool {
    match value {
        Value::Object(obj) => {
            matches!(obj.get("$ref"), Some(Value::String(_)))
                || obj.values().any(contains_schema_ref)
        }
        Value::Array(items) => items.iter().any(contains_schema_ref),
        _ => false,
    }
}

fn inline_local_schema_refs(
    node: &Value,
    root: &Value,
    active_refs: &mut Vec<String>,
    depth: usize,
) -> Value {
    const MAX_REF_DEPTH: usize = 64;
    if depth > MAX_REF_DEPTH {
        return node.clone();
    }

    match node {
        Value::Object(obj) => {
            if let Some(reference) = obj.get("$ref").and_then(Value::as_str)
                && let Some(resolved) = resolve_local_schema_ref(root, reference)
                && !active_refs.iter().any(|active| active == reference)
            {
                active_refs.push(reference.to_string());
                let mut inlined = inline_local_schema_refs(resolved, root, active_refs, depth + 1);
                active_refs.pop();

                if let Value::Object(inlined_obj) = &mut inlined {
                    // Sibling keywords next to `$ref` (typically `description`)
                    // override the referenced definition.
                    for (key, value) in obj {
                        if key == "$ref" {
                            continue;
                        }
                        inlined_obj.insert(
                            key.clone(),
                            inline_local_schema_refs(value, root, active_refs, depth + 1),
                        );
                    }
                }
                return inlined;
            }

            let mut mapped = Map::new();
            for (key, value) in obj {
                mapped.insert(
                    key.clone(),
                    inline_local_schema_refs(value, root, active_refs, depth + 1),
                );
            }
            Value::Object(mapped)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| inline_local_schema_refs(item, root, active_refs, depth + 1))
                .collect(),
        ),
        _ => node.clone(),
    }
}

fn resolve_local_schema_ref<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    if !reference.starts_with("#/") {
        return None;
    }

    let mut cursor = root;
    for segment in reference.trim_start_matches("#/").split('/') {
        let key = segment.replace("~1", "/").replace("~0", "~");
        cursor = cursor.get(&key)?;
    }

    Some(cursor)
}

/// Shared fixtures for the request-builder regression tests in `client`,
/// `client_compatible`, `text_adapter` and `live`.
#[cfg(test)]
pub(crate) mod test_fixtures {
    use serde_json::{Value, json};

    /// The `workgraph_claim` schema exactly as shipped from 0.8.22 to 0.8.33:
    /// a root-level `not` expressing the lease exclusivity. The live tool no
    /// longer carries it; the request builders must drop it if a host or an
    /// older tool catalog still does.
    pub(crate) fn pre_fix_workgraph_claim_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "realm_id": { "type": "string" },
                "namespace": { "type": "string" },
                "id": { "type": "string" },
                "expected_revision": { "type": "integer", "minimum": 0 },
                "owner": {
                    "type": "object",
                    "properties": {
                        "key": {
                            "type": "object",
                            "properties": {
                                "kind": {
                                    "type": "string",
                                    "enum": ["principal", "agent", "session", "mob", "label"]
                                },
                                "id": { "type": "string" }
                            },
                            "required": ["kind", "id"],
                            "additionalProperties": false
                        },
                        "display_name": { "type": "string" }
                    },
                    "required": ["key"],
                    "additionalProperties": false
                },
                "lease_seconds": { "type": "integer", "minimum": 1, "maximum": 86400 },
                "lease_expires_at": { "type": "string", "format": "date-time" }
            },
            "required": ["id", "expected_revision", "owner"],
            "additionalProperties": false,
            "not": { "required": ["lease_seconds", "lease_expires_at"] }
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;

    fn normalize(schema: &Value) -> Value {
        normalize_openai_tool_parameters_schema("test_tool", schema)
            .expect("object-root schema normalizes")
            .into_owned()
    }

    #[test]
    fn root_not_is_removed_and_nested_not_is_kept() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "not": {"enum": [""]}}
            },
            "not": {"required": ["a", "b"]}
        });

        let normalized = normalize(&schema);

        assert!(normalized.get("not").is_none());
        assert_eq!(
            normalized["properties"]["name"]["not"],
            json!({"enum": [""]}),
            "nested combinators are accepted by OpenAI and stay untouched"
        );
        assert_eq!(normalized["type"], "object");
    }

    #[test]
    fn root_enum_is_removed_and_nested_enum_is_kept() {
        let schema = json!({
            "type": "object",
            "enum": [{"mode": "a"}, {"mode": "b"}],
            "properties": {"mode": {"type": "string", "enum": ["a", "b"]}}
        });

        let normalized = normalize(&schema);

        assert!(
            normalized.get("enum").is_none(),
            "the validator rejects a root enum: {normalized}"
        );
        assert_eq!(normalized["properties"]["mode"]["enum"], json!(["a", "b"]));
        assert_eq!(normalized["type"], "object");
    }

    #[test]
    fn root_without_type_is_declared_object_even_without_properties() {
        let empty = json!({});
        assert_eq!(normalize(&empty), json!({"type": "object"}));

        let described = json!({"description": "takes anything"});
        assert_eq!(
            normalize(&described),
            json!({"type": "object", "description": "takes anything"})
        );

        let nullable_object = json!({"type": ["object", "null"], "properties": {}});
        assert_eq!(normalize(&nullable_object)["type"], "object");
    }

    #[test]
    fn non_object_root_is_a_typed_error_naming_the_tool() {
        let bare_enum = json!({"type": "string", "enum": ["fast", "slow"]});
        let error = normalize_openai_tool_parameters_schema("set_speed", &bare_enum)
            .expect_err("a string root can never be function parameters");
        match error {
            LlmError::InvalidRequest { message } => {
                assert!(message.contains("`set_speed`"), "{message}");
                assert!(message.contains("\"string\""), "{message}");
                assert!(message.contains("\"type\": \"object\""), "{message}");
            }
            other => panic!("expected InvalidRequest, got {other:?}"),
        }

        let not_a_schema = json!(true);
        assert!(matches!(
            normalize_openai_tool_parameters_schema("set_speed", &not_a_schema),
            Err(LlmError::InvalidRequest { .. })
        ));
    }

    #[test]
    fn schema_already_in_emission_shape_is_returned_borrowed() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "not": {"enum": [""]}},
                "mode": {"oneOf": [{"const": "a"}, {"const": "b"}]}
            },
            "required": ["name"],
            "additionalProperties": false
        });

        let normalized =
            normalize_openai_tool_parameters_schema("test_tool", &schema).expect("object root");

        assert!(
            matches!(normalized, Cow::Borrowed(_)),
            "nothing to rewrite must not allocate a copy"
        );
        assert_eq!(*normalized, schema);

        for keyword in ROOT_REJECTED_KEYWORDS {
            let mut rejected = schema.clone();
            rejected[*keyword] = json!([]);
            assert!(
                matches!(
                    normalize_openai_tool_parameters_schema("test_tool", &rejected),
                    Ok(Cow::Owned(_))
                ),
                "a root `{keyword}` forces a rewrite"
            );
        }
    }

    #[test]
    fn local_refs_are_inlined_and_definitions_dropped() {
        let schema = json!({
            "type": "object",
            "properties": {
                "payload": {"$ref": "#/$defs/Payload", "description": "override"},
                "legacy": {"$ref": "#/definitions/Legacy"}
            },
            "$defs": {
                "Payload": {"type": "object", "properties": {"message": {"type": "string"}}}
            },
            "definitions": {
                "Legacy": {"type": "integer"}
            }
        });

        let normalized = normalize(&schema);

        assert_eq!(normalized["properties"]["payload"]["type"], "object");
        assert_eq!(
            normalized["properties"]["payload"]["properties"]["message"]["type"],
            "string"
        );
        assert_eq!(
            normalized["properties"]["payload"]["description"], "override",
            "keywords beside $ref override the referenced definition"
        );
        assert!(normalized["properties"]["payload"].get("$ref").is_none());
        assert_eq!(normalized["properties"]["legacy"]["type"], "integer");
        assert!(normalized.get("$defs").is_none());
        assert!(normalized.get("definitions").is_none());
    }

    #[test]
    fn definitions_are_kept_while_a_recursive_ref_remains() {
        let schema = json!({
            "type": "object",
            "properties": {"tree": {"$ref": "#/$defs/Node"}},
            "$defs": {
                "Node": {
                    "type": "object",
                    "properties": {
                        "children": {"type": "array", "items": {"$ref": "#/$defs/Node"}}
                    }
                }
            }
        });

        let normalized = normalize(&schema);

        assert_eq!(normalized["properties"]["tree"]["type"], "object");
        assert_eq!(
            normalized["properties"]["tree"]["properties"]["children"]["items"]["$ref"],
            "#/$defs/Node",
            "the cycle guard leaves the recursive reference in place"
        );
        assert!(
            normalized.get("$defs").is_some(),
            "a referenced definitions block must survive"
        );
    }

    #[test]
    fn unresolvable_external_ref_is_passed_through() {
        let schema = json!({
            "type": "object",
            "properties": {"payload": {"$ref": "https://example.com/Payload.json"}}
        });

        let normalized = normalize(&schema);

        assert_eq!(normalized, schema);
    }

    #[test]
    fn root_all_of_object_members_fold_with_required_union() {
        let schema = json!({
            "type": "object",
            "properties": {"mode": {"type": "string"}},
            "required": ["mode"],
            "allOf": [
                {"properties": {"command": {"type": "string"}, "mode": {"type": "integer"}},
                 "required": ["command"]},
                {"required": ["mode", "cwd"], "properties": {"cwd": {"type": "string"}}}
            ]
        });

        let normalized = normalize(&schema);

        assert!(normalized.get("allOf").is_none());
        assert_eq!(normalized["properties"]["command"]["type"], "string");
        assert_eq!(normalized["properties"]["cwd"]["type"], "string");
        assert_eq!(
            normalized["properties"]["mode"]["type"], "string",
            "the root property wins on an allOf name clash"
        );
        assert_eq!(normalized["required"], json!(["mode", "command", "cwd"]));
    }

    #[test]
    fn root_one_of_object_members_fold_with_required_intersection() {
        let schema = json!({
            "oneOf": [
                {"type": "object",
                 "properties": {"kind": {"const": "session"}, "session_id": {"type": "string"}},
                 "required": ["kind", "session_id"]},
                {"type": "object",
                 "properties": {"kind": {"const": "owner"}, "owner_key": {"type": "string"}},
                 "required": ["kind", "owner_key"]}
            ]
        });

        let normalized = normalize(&schema);

        assert!(normalized.get("oneOf").is_none());
        assert_eq!(
            normalized["type"], "object",
            "the folded root takes the members' object type"
        );
        assert_eq!(
            normalized["properties"]["kind"],
            json!({"enum": ["session", "owner"]}),
            "literal discriminators from every variant merge into one enum"
        );
        assert_eq!(normalized["properties"]["session_id"]["type"], "string");
        assert_eq!(normalized["properties"]["owner_key"]["type"], "string");
        assert_eq!(
            normalized["required"],
            json!(["kind"]),
            "only names required by every variant stay required"
        );
    }

    #[test]
    fn variant_discriminators_merge_literals_and_keep_structural_clashes_first_wins() {
        let schema = json!({
            "anyOf": [
                {"type": "object", "properties": {
                    "kind": {"type": "string", "enum": ["a", "b"], "description": "which"},
                    "level": {"const": 1},
                    "target": {"type": "object", "properties": {"id": {"type": "string"}}}
                }},
                {"type": "object", "properties": {
                    "kind": {"type": "string", "const": "c"},
                    "level": {"const": "high"},
                    "target": {"type": "string"}
                }}
            ]
        });

        let normalized = normalize(&schema);

        assert_eq!(
            normalized["properties"]["kind"],
            json!({"type": "string", "enum": ["a", "b", "c"], "description": "which"}),
            "enum and const literals merge, descriptions and a shared type survive"
        );
        assert_eq!(
            normalized["properties"]["level"],
            json!({"enum": [1, "high"]}),
            "literals of differing types merge without a type"
        );
        assert_eq!(
            normalized["properties"]["target"]["type"], "object",
            "a structural clash keeps the first variant's definition"
        );
    }

    #[test]
    fn root_any_of_without_common_required_leaves_required_absent() {
        let schema = json!({
            "anyOf": [
                {"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]},
                {"type": "object", "properties": {"b": {"type": "string"}}}
            ]
        });

        let normalized = normalize(&schema);

        assert!(normalized.get("anyOf").is_none());
        assert!(normalized.get("required").is_none());
        assert_eq!(normalized["properties"]["a"]["type"], "string");
        assert_eq!(normalized["properties"]["b"]["type"], "string");
    }

    #[test]
    fn nested_combinators_below_root_are_untouched() {
        let schema = json!({
            "type": "object",
            "properties": {
                "target": {
                    "oneOf": [
                        {"type": "object", "properties": {"kind": {"const": "a"}}},
                        {"type": "object", "properties": {"kind": {"const": "b"}}}
                    ]
                },
                "policy": {"allOf": [{"required": ["x"]}], "anyOf": [{"type": "string"}]}
            }
        });

        let normalized = normalize(&schema);

        assert_eq!(normalized, schema);
    }

    #[test]
    fn pre_fix_workgraph_claim_shape_loses_only_its_root_not() {
        let schema = test_fixtures::pre_fix_workgraph_claim_schema();

        let normalized = normalize(&schema);

        let mut expected = schema;
        expected
            .as_object_mut()
            .expect("object schema")
            .remove("not");
        assert_eq!(normalized, expected);
    }
}
