//! Tool registry for tracking and validating tools

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::ToolValidationError;
use meerkat_core::types::ToolDef;

/// Registry for managing available tools and their schemas
#[derive(Debug, Clone, Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<ToolDef>>,
}

impl ToolRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new tool
    pub fn register(&mut self, def: ToolDef) {
        self.tools.insert(def.name.clone(), Arc::new(def));
    }

    /// Register multiple tools
    pub fn register_many(&mut self, defs: impl IntoIterator<Item = ToolDef>) {
        for def in defs {
            self.register(def);
        }
    }

    /// Get a tool definition by name
    pub fn get(&self, name: &str) -> Option<Arc<ToolDef>> {
        self.tools.get(name).cloned()
    }

    /// Get all registered tools
    pub fn list(&self) -> Vec<Arc<ToolDef>> {
        self.tools.values().cloned().collect()
    }

    /// Validate tool arguments against its schema
    pub fn validate(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<(), ToolValidationError> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ToolValidationError::not_found(name))?;

        // Basic schema validation using jsonschema
        let compiled = jsonschema::Validator::new(&tool.input_schema)
            .map_err(|e| ToolValidationError::invalid_arguments(name, e.to_string()))?;

        if let Err(error) = compiled.validate(args) {
            return Err(ToolValidationError::invalid_arguments(
                name,
                error.to_string(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::schema::empty_object_schema;
    use serde_json::json;

    #[test]
    fn test_registry_register_and_get() {
        let mut registry = ToolRegistry::new();
        let def = ToolDef {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: empty_object_schema(),
        };

        registry.register(def.clone());
        let fetched = registry.get("test_tool").unwrap();
        assert_eq!(fetched.name, def.name);
    }

    #[test]
    fn test_registry_validate_not_found() {
        let registry = ToolRegistry::new();
        let result = registry.validate("missing", &json!({}));
        assert!(matches!(result, Err(ToolValidationError::NotFound { .. })));
    }

    #[test]
    fn test_registry_validate_invalid_args() {
        let mut registry = ToolRegistry::new();
        let def = ToolDef {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "count": { "type": "integer" }
                },
                "required": ["count"]
            }),
        };

        registry.register(def);

        // Missing required field
        let result = registry.validate("test_tool", &json!({}));
        assert!(matches!(
            result,
            Err(ToolValidationError::InvalidArguments { .. })
        ));

        // Wrong type
        let result = registry.validate("test_tool", &json!({"count": "not a number"}));
        assert!(matches!(
            result,
            Err(ToolValidationError::InvalidArguments { .. })
        ));

        // Valid args
        let result = registry.validate("test_tool", &json!({"count": 42}));
        assert!(result.is_ok());
    }
}
