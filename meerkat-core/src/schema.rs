//! Meerkat-native schema abstraction and provider lowering.

use crate::Provider;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Schema format versions supported by Meerkat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaFormat {
    MeerkatV1,
}

impl Default for SchemaFormat {
    fn default() -> Self {
        Self::MeerkatV1
    }
}

/// Compatibility mode for provider lowering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaCompat {
    Lossy,
    Strict,
}

impl Default for SchemaCompat {
    fn default() -> Self {
        Self::Lossy
    }
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

    /// Compile this schema for a specific provider.
    pub fn compile_for(
        &self,
        provider: Provider,
        compat: SchemaCompat,
    ) -> Result<CompiledSchema, SchemaError> {
        let (schema, warnings) = match provider {
            Provider::OpenAI | Provider::Other => (self.0.clone(), Vec::new()),
            Provider::Anthropic => {
                let mut warnings = Vec::new();
                let mut schema = self.0.clone();
                ensure_additional_properties_false(&mut schema, &mut warnings, provider, "");
                (schema, warnings)
            }
            Provider::Gemini => sanitize_for_gemini(&self.0, provider),
        };

        if compat == SchemaCompat::Strict && !warnings.is_empty() {
            return Err(SchemaError::UnsupportedFeatures { provider, warnings });
        }

        Ok(CompiledSchema { schema, warnings })
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

fn ensure_additional_properties_false(
    value: &mut Value,
    warnings: &mut Vec<SchemaWarning>,
    provider: Provider,
    path: &str,
) {
    match value {
        Value::Object(obj) => {
            let is_object_type = match obj.get("type") {
                Some(Value::String(t)) => t == "object",
                Some(Value::Array(types)) => types.iter().any(|t| t.as_str() == Some("object")),
                _ => obj.contains_key("properties"),
            };
            if is_object_type
                && obj.contains_key("properties")
                && !obj.contains_key("additionalProperties")
            {
                obj.insert("additionalProperties".to_string(), Value::Bool(false));
            }

            let keys: Vec<String> = obj.keys().cloned().collect();
            for key in keys {
                let next = join_path(path, &key);
                if let Some(child) = obj.get_mut(&key) {
                    ensure_additional_properties_false(child, warnings, provider, &next);
                }
            }
        }
        Value::Array(items) => {
            for (idx, item) in items.iter_mut().enumerate() {
                let next = join_index(path, idx);
                ensure_additional_properties_false(item, warnings, provider, &next);
            }
        }
        _ => {}
    }
}

fn sanitize_for_gemini(schema: &Value, provider: Provider) -> (Value, Vec<SchemaWarning>) {
    let mut warnings = Vec::new();
    let sanitized = sanitize_gemini_value(schema, provider, "", &mut warnings);
    (sanitized, warnings)
}

fn sanitize_gemini_value(
    value: &Value,
    provider: Provider,
    path: &str,
    warnings: &mut Vec<SchemaWarning>,
) -> Value {
    match value {
        Value::Object(obj) => {
            let mut sanitized = Map::new();
            for (key, value) in obj {
                if is_gemini_unsupported_key(key) {
                    warnings.push(SchemaWarning {
                        provider,
                        path: join_path(path, key),
                        message: format!("Removed unsupported keyword '{key}'"),
                    });
                    continue;
                }

                if key == "type" {
                    if let Value::Array(types) = value {
                        let primary = types
                            .iter()
                            .find(|t| t.as_str() != Some("null"))
                            .cloned()
                            .unwrap_or_else(|| Value::String("string".to_string()));
                        warnings.push(SchemaWarning {
                            provider,
                            path: join_path(path, key),
                            message: "Collapsed array type to a single type".to_string(),
                        });
                        sanitized.insert(key.clone(), primary);
                        continue;
                    }
                }

                let next = join_path(path, key);
                sanitized.insert(
                    key.clone(),
                    sanitize_gemini_value(value, provider, &next, warnings),
                );
            }
            Value::Object(sanitized)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .enumerate()
                .map(|(idx, item)| {
                    let next = join_index(path, idx);
                    sanitize_gemini_value(item, provider, &next, warnings)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

fn is_gemini_unsupported_key(key: &str) -> bool {
    matches!(
        key,
        "$defs" | "$ref" | "$schema" | "additionalProperties" | "oneOf" | "anyOf" | "allOf"
    )
}

fn join_path(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        format!("/{key}")
    } else {
        format!("{prefix}/{key}")
    }
}

fn join_index(prefix: &str, index: usize) -> String {
    if prefix.is_empty() {
        format!("/{index}")
    } else {
        format!("{prefix}/{index}")
    }
}

#[cfg(test)]
mod tests {
    use super::{MeerkatSchema, SchemaCompat};
    use crate::Provider;
    use serde_json::json;

    #[test]
    fn test_compile_gemini_lossy_removes_unsupported() -> Result<(), Box<dyn std::error::Error>> {
        let schema = json!({
            "type": "object",
            "properties": {
                "id": {"$ref": "#/$defs/Id"},
                "choice": {"oneOf": [{"type": "string"}, {"type": "integer"}]}
            },
            "$defs": {
                "Id": {"type": "string"}
            }
        });

        let schema = MeerkatSchema::new(schema)?;
        let compiled = schema.compile_for(Provider::Gemini, SchemaCompat::Lossy)?;

        assert!(compiled.schema.get("$defs").is_none());
        assert!(
            compiled.schema["properties"]["id"].get("$ref").is_none(),
            "Gemini schema should remove $ref"
        );
        assert!(
            compiled.schema["properties"]["choice"].get("oneOf").is_none(),
            "Gemini schema should remove oneOf"
        );

        assert!(compiled
            .warnings
            .iter()
            .any(|w| w.path == "/$defs"));
        assert!(compiled
            .warnings
            .iter()
            .any(|w| w.path == "/properties/id/$ref"));
        assert!(compiled
            .warnings
            .iter()
            .any(|w| w.path == "/properties/choice/oneOf"));

        Ok(())
    }

    #[test]
    fn test_compile_gemini_strict_errors() -> Result<(), Box<dyn std::error::Error>> {
        let schema = json!({
            "type": "object",
            "properties": {
                "id": {"$ref": "#/$defs/Id"}
            },
            "$defs": {"Id": {"type": "string"}}
        });
        let schema = MeerkatSchema::new(schema)?;
        let result = schema.compile_for(Provider::Gemini, SchemaCompat::Strict);
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn test_compile_anthropic_adds_additional_properties() -> Result<(), Box<dyn std::error::Error>>
    {
        let schema = json!({
            "type": "object",
            "properties": {
                "nested": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"}
                    }
                }
            }
        });
        let schema = MeerkatSchema::new(schema)?;
        let compiled = schema.compile_for(Provider::Anthropic, SchemaCompat::Lossy)?;

        assert_eq!(compiled.schema["additionalProperties"], json!(false));
        assert_eq!(
            compiled.schema["properties"]["nested"]["additionalProperties"],
            json!(false)
        );
        Ok(())
    }

    #[test]
    fn test_compile_openai_passthrough() -> Result<(), Box<dyn std::error::Error>> {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"}
            },
            "required": ["name"]
        });
        let schema = MeerkatSchema::new(schema.clone())?;
        let compiled = schema.compile_for(Provider::OpenAI, SchemaCompat::Lossy)?;
        assert_eq!(compiled.schema, schema.as_value().clone());
        assert!(compiled.warnings.is_empty());
        Ok(())
    }
}
