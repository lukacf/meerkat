//! Tool registry for schema validation

use jsonschema::Validator;
use meerkat_core::ToolDef;
use serde_json::Value;
use std::collections::HashMap;

/// Registry for tool definitions and schema validation
pub struct ToolRegistry {
    tools: HashMap<String, ToolDef>,
    validators: HashMap<String, Validator>,
}

impl ToolRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            validators: HashMap::new(),
        }
    }

    /// Register a tool definition
    pub fn register(&mut self, tool: ToolDef) {
        // Compile the JSON Schema validator for this tool
        if let Ok(validator) = Validator::new(&tool.input_schema) {
            self.validators.insert(tool.name.clone(), validator);
        }
        self.tools.insert(tool.name.clone(), tool);
    }

    /// Get tool definitions for LLM requests
    pub fn tool_defs(&self) -> Vec<ToolDef> {
        self.tools.values().cloned().collect()
    }

    /// Validate arguments against a tool's schema
    pub fn validate(&self, name: &str, args: &Value) -> Result<(), crate::error::ToolValidationError> {
        let _tool = self.tools.get(name).ok_or_else(|| {
            crate::error::ToolValidationError::ToolNotFound(name.to_string())
        })?;

        // Validate against compiled schema if available
        if let Some(validator) = self.validators.get(name) {
            // Collect all validation errors
            let errors: Vec<String> = validator
                .iter_errors(args)
                .map(|e| format!("{}: {}", e.instance_path, e))
                .collect();

            if !errors.is_empty() {
                return Err(crate::error::ToolValidationError::SchemaValidation(
                    errors.join("; "),
                ));
            }
        }

        Ok(())
    }

    /// Check if a tool is registered
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_valid_args() {
        let mut registry = ToolRegistry::new();
        registry.register(ToolDef {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "count": {"type": "integer"}
                },
                "required": ["name"]
            }),
        });

        // Valid args
        let result = registry.validate(
            "test_tool",
            &serde_json::json!({"name": "test", "count": 5}),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_missing_required() {
        let mut registry = ToolRegistry::new();
        registry.register(ToolDef {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"}
                },
                "required": ["name"]
            }),
        });

        // Missing required field
        let result = registry.validate("test_tool", &serde_json::json!({}));
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::error::ToolValidationError::SchemaValidation(_)
        ));
    }

    #[test]
    fn test_validate_wrong_type() {
        let mut registry = ToolRegistry::new();
        registry.register(ToolDef {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "count": {"type": "integer"}
                }
            }),
        });

        // Wrong type (string instead of integer)
        let result = registry.validate("test_tool", &serde_json::json!({"count": "not a number"}));
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_tool_not_found() {
        let registry = ToolRegistry::new();
        let result = registry.validate("nonexistent", &serde_json::json!({}));
        assert!(matches!(
            result.unwrap_err(),
            crate::error::ToolValidationError::ToolNotFound(_)
        ));
    }
}
