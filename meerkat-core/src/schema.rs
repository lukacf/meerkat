//! Meerkat-native schema abstraction and normalization.
//!
//! Provider-specific lowering lives in the adapter crates
//! (`meerkat-client/src/anthropic.rs`, `meerkat-client/src/gemini.rs`).

use crate::Provider;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Schema format versions supported by Meerkat.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaFormat {
    #[default]
    MeerkatV1,
}

/// Compatibility mode for provider lowering.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaCompat {
    #[default]
    Lossy,
    Strict,
}

/// Warnings emitted during schema lowering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SchemaWarning {
    pub provider: Provider,
    pub path: String,
    pub message: String,
}

/// Errors for schema parsing/compilation.
#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error("Schema must be a JSON object at the root")]
    InvalidRoot,
    #[error("Schema contains unsupported features for {provider:?}: {warnings:?}")]
    UnsupportedFeatures {
        provider: Provider,
        warnings: Vec<SchemaWarning>,
    },
}

/// A Meerkat-native JSON schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MeerkatSchema(Value);

impl MeerkatSchema {
    /// Create a new Meerkat schema with normalization.
    pub fn new(schema: Value) -> Result<Self, SchemaError> {
        if !schema.is_object() {
            return Err(SchemaError::InvalidRoot);
        }
        let mut normalized = schema;
        normalize_schema(&mut normalized);
        Ok(Self(normalized))
    }

    /// Access the underlying JSON value.
    pub fn as_value(&self) -> &Value {
        &self.0
    }
}

/// Provider-compiled schema and warnings.
#[derive(Debug, Clone)]
pub struct CompiledSchema {
    pub schema: Value,
    pub warnings: Vec<SchemaWarning>,
}

fn normalize_schema(value: &mut Value) {
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
            }

            for value in obj.values_mut() {
                normalize_schema(value);
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                normalize_schema(item);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::MeerkatSchema;
    use serde_json::json;

    #[test]
    fn test_normalize_adds_properties_and_required() -> Result<(), Box<dyn std::error::Error>> {
        let schema = json!({"type": "object"});
        let schema = MeerkatSchema::new(schema)?;
        assert!(schema.as_value().get("properties").is_some());
        assert!(schema.as_value().get("required").is_some());
        Ok(())
    }

    #[test]
    fn test_invalid_root_rejected() {
        assert!(MeerkatSchema::new(json!("string")).is_err());
        assert!(MeerkatSchema::new(json!(42)).is_err());
    }

    #[test]
    fn test_normalize_recurses_nested_objects() -> Result<(), Box<dyn std::error::Error>> {
        let schema = json!({
            "type": "object",
            "properties": {
                "profile": {
                    "type": "object",
                    "properties": {
                        "city": {"type": "string"}
                    }
                },
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object"
                    }
                },
                "variant": {
                    "anyOf": [
                        {"type": "object"},
                        {"type": "string"}
                    ]
                }
            }
        });

        let schema = MeerkatSchema::new(schema)?;
        let root = schema.as_value();

        assert!(root.get("required").is_some());
        assert!(root["properties"]["profile"].get("required").is_some());
        assert!(
            root["properties"]["items"]["items"]
                .get("properties")
                .is_some()
        );
        assert!(
            root["properties"]["variant"]["anyOf"][0]
                .get("required")
                .is_some()
        );
        Ok(())
    }

    #[test]
    fn test_normalize_preserves_existing_object_shape() -> Result<(), Box<dyn std::error::Error>> {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"}
            },
            "required": ["name"]
        });

        let schema = MeerkatSchema::new(schema)?;
        let root = schema.as_value();

        assert_eq!(root["required"], json!(["name"]));
        assert_eq!(root["properties"]["name"]["type"], "string");
        Ok(())
    }
}
