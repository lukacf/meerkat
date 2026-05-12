use serde_json::{Map, Value};

const MAX_REF_DEPTH: usize = 16;

/// Project Meerkat tool schemas into the JSON Schema subset OpenAI accepts for
/// function parameters.
///
/// Core dispatchers still validate the authoritative typed argument contract.
/// This projection removes schema features that are only provider guidance,
/// while preserving object-vs-scalar shape and closed scalar enums.
pub(crate) fn openai_function_parameters(schema: &Value) -> Value {
    let defs = schema
        .get("$defs")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut projected = project_schema(schema, &defs, None, 0);
    strip_definition_metadata(&mut projected);
    ensure_object_schema_shape(&mut projected);
    projected
}

fn project_schema(
    schema: &Value,
    defs: &Map<String, Value>,
    property_name: Option<&str>,
    ref_depth: usize,
) -> Value {
    if is_semantic_payload_slot(property_name) && resolves_to_composite_payload(schema, defs) {
        return semantic_payload_schema(schema);
    }

    match schema {
        Value::Object(obj) => {
            if let Some(reference) = obj.get("$ref").and_then(Value::as_str)
                && let Some(target) = resolve_local_ref(reference, defs)
            {
                if ref_depth >= MAX_REF_DEPTH {
                    return fallback_schema_for_ref(target);
                }
                let mut resolved = project_schema(target, defs, property_name, ref_depth + 1);
                copy_description(obj, &mut resolved);
                return resolved;
            }

            let mut out = Map::new();
            for (key, value) in obj {
                match key.as_str() {
                    "$defs" | "$schema" | "title" => {}
                    "properties" => {
                        if let Some(properties) = value.as_object() {
                            let projected = properties
                                .iter()
                                .map(|(name, property_schema)| {
                                    (
                                        name.clone(),
                                        project_schema(
                                            property_schema,
                                            defs,
                                            Some(name),
                                            ref_depth,
                                        ),
                                    )
                                })
                                .collect();
                            out.insert(key.clone(), Value::Object(projected));
                        }
                    }
                    "items" => {
                        out.insert(key.clone(), project_schema(value, defs, None, ref_depth));
                    }
                    "anyOf" | "oneOf" | "allOf" => {
                        if let Some(items) = value.as_array() {
                            out.insert(
                                key.clone(),
                                Value::Array(
                                    items
                                        .iter()
                                        .map(|item| {
                                            project_schema(item, defs, property_name, ref_depth)
                                        })
                                        .collect(),
                                ),
                            );
                        }
                    }
                    _ => {
                        out.insert(key.clone(), value.clone());
                    }
                }
            }
            let mut value = Value::Object(out);
            ensure_object_schema_shape(&mut value);
            value
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| project_schema(item, defs, property_name, ref_depth))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn is_semantic_payload_slot(property_name: Option<&str>) -> bool {
    matches!(property_name, Some("params" | "result"))
}

fn resolves_to_composite_payload(schema: &Value, defs: &Map<String, Value>) -> bool {
    let mut current = schema;
    for _ in 0..MAX_REF_DEPTH {
        let Some(reference) = current.get("$ref").and_then(Value::as_str) else {
            break;
        };
        if reference.ends_with("/CommsPeerRequestParams")
            || reference.ends_with("/CommsPeerResponseResult")
        {
            return true;
        }
        let Some(target) = resolve_local_ref(reference, defs) else {
            break;
        };
        current = target;
    }

    current.get("anyOf").is_some() || current.get("oneOf").is_some()
}

fn semantic_payload_schema(original: &Value) -> Value {
    let mut object = Map::new();
    object.insert(
        "description".to_string(),
        Value::String(
            original
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("Typed semantic payload; validated by the Meerkat dispatcher.")
                .to_string(),
        ),
    );
    object.insert("type".to_string(), Value::String("object".to_string()));
    object.insert("additionalProperties".to_string(), Value::Bool(true));
    Value::Object(object)
}

fn resolve_local_ref<'a>(reference: &str, defs: &'a Map<String, Value>) -> Option<&'a Value> {
    let key = reference.strip_prefix("#/$defs/")?;
    defs.get(key)
}

fn fallback_schema_for_ref(target: &Value) -> Value {
    if target.get("enum").is_some() {
        target.clone()
    } else if target.get("type").and_then(Value::as_str) == Some("string") {
        serde_json::json!({"type": "string"})
    } else {
        serde_json::json!({"type": "object", "additionalProperties": true})
    }
}

fn strip_definition_metadata(value: &mut Value) {
    match value {
        Value::Object(obj) => {
            obj.remove("$defs");
            obj.remove("$schema");
            obj.remove("title");
            for child in obj.values_mut() {
                strip_definition_metadata(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                strip_definition_metadata(item);
            }
        }
        _ => {}
    }
}

fn ensure_object_schema_shape(value: &mut Value) {
    match value {
        Value::Object(obj) => {
            let is_object_type = match obj.get("type") {
                Some(Value::String(t)) => t == "object",
                Some(Value::Array(types)) => types.iter().any(|t| t.as_str() == Some("object")),
                _ => obj.contains_key("properties") || obj.contains_key("required"),
            };

            if is_object_type {
                obj.entry("properties".to_string())
                    .or_insert_with(|| Value::Object(Map::new()));
                obj.entry("required".to_string())
                    .or_insert_with(|| Value::Array(Vec::new()));
                obj.entry("additionalProperties".to_string())
                    .or_insert(Value::Bool(false));
            }

            for child in obj.values_mut() {
                ensure_object_schema_shape(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                ensure_object_schema_shape(item);
            }
        }
        _ => {}
    }
}

fn copy_description(source: &Map<String, Value>, target: &mut Value) {
    let Some(description) = source.get("description") else {
        return;
    };
    if let Value::Object(obj) = target {
        obj.entry("description".to_string())
            .or_insert_with(|| description.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::openai_function_parameters;
    use serde_json::json;

    #[test]
    fn comms_request_payload_projects_to_openai_object_slot() {
        let schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$defs": {
                "PeerId": { "type": "string" },
                "HandlingMode": { "type": "string", "enum": ["queue", "steer"] },
                "CommsPeerRequestIntent": {
                    "type": "string",
                    "enum": ["supervisor.bridge", "checksum_token"]
                },
                "CommsPeerRequestParams": {
                    "anyOf": [
                        { "$ref": "#/$defs/BridgeCommand" },
                        { "$ref": "#/$defs/CommsChecksumTokenParams" }
                    ]
                },
                "CommsChecksumTokenParams": {
                    "type": "object",
                    "properties": { "subject": { "type": "string" } },
                    "required": ["subject"],
                    "additionalProperties": false
                },
                "BridgeCommand": {
                    "oneOf": [
                        {
                            "type": "object",
                            "properties": {
                                "command": { "const": "deliver_member_input", "type": "string" },
                                "content": { "$ref": "#/$defs/ContentInput" }
                            },
                            "required": ["command", "content"],
                            "additionalProperties": false
                        }
                    ]
                },
                "ContentInput": {
                    "anyOf": [
                        { "type": "string" },
                        {
                            "type": "array",
                            "items": { "$ref": "#/$defs/ContentBlock" }
                        }
                    ]
                },
                "ContentBlock": {
                    "oneOf": [
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "json", "type": "string" },
                                "value": true
                            },
                            "required": ["type", "value"]
                        }
                    ]
                }
            },
            "type": "object",
            "properties": {
                "peer_id": { "$ref": "#/$defs/PeerId" },
                "intent": { "$ref": "#/$defs/CommsPeerRequestIntent" },
                "handling_mode": { "$ref": "#/$defs/HandlingMode" },
                "params": {
                    "$ref": "#/$defs/CommsPeerRequestParams",
                    "description": "Request parameters"
                }
            },
            "required": ["peer_id", "intent", "handling_mode", "params"]
        });

        let projected = openai_function_parameters(&schema);

        assert!(projected.get("$defs").is_none());
        assert!(projected.get("$schema").is_none());
        assert_eq!(projected["properties"]["peer_id"]["type"], "string");
        assert_eq!(
            projected["properties"]["intent"]["enum"],
            json!(["supervisor.bridge", "checksum_token"])
        );
        assert_eq!(projected["properties"]["params"]["type"], "object");
        assert_eq!(
            projected["properties"]["params"]["additionalProperties"],
            true
        );
    }
}
