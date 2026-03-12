//! Structured output extraction utilities.
//!
//! Extraction is integrated into the main state machine loop in `state.rs`.
//! This module provides the constant prompt and helper functions used by the
//! extraction path: markdown code-fence stripping and named object wrapper
//! unwrapping.

use crate::types::OutputSchema;
use serde_json::Value;

/// Default prompt for the structured output extraction turn.
pub const DEFAULT_EXTRACTION_PROMPT: &str = "Provide the final output as valid JSON matching \
    the required schema. Output ONLY the JSON, no additional text or markdown formatting.";

/// Strip markdown code fences from JSON content.
///
/// LLMs sometimes wrap JSON in ```json ... ``` even when asked not to.
pub(super) fn strip_code_fences(content: &str) -> &str {
    let trimmed = content.trim();

    // Check for ```json or ``` at start
    let without_prefix = if let Some(stripped) = trimmed.strip_prefix("```json") {
        stripped
    } else if let Some(stripped) = trimmed.strip_prefix("```") {
        stripped
    } else {
        return trimmed;
    };

    // Check for ``` at end
    let without_suffix = without_prefix.trim();
    if let Some(stripped) = without_suffix.strip_suffix("```") {
        stripped.trim()
    } else {
        without_suffix.trim()
    }
}

/// Some providers may wrap valid schema output in a named envelope
/// (for example, `{"advisor": {...actual schema object...}}`).
/// When the envelope key is the schema name and the inner object clearly
/// matches the root schema shape better than the wrapped object, unwrap it.
pub(super) fn unwrap_named_object_wrapper(parsed: Value, output_schema: &OutputSchema) -> Value {
    let Some(wrapper_key) = output_schema.name.as_deref() else {
        return parsed;
    };
    let Value::Object(outer) = &parsed else {
        return parsed;
    };
    if outer.len() != 1 {
        return parsed;
    }
    let Some(Value::Object(inner)) = outer.get(wrapper_key) else {
        return parsed;
    };

    let schema = output_schema.schema.as_value();
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|obj| {
            obj.keys()
                .map(std::string::String::as_str)
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();

    let wrapper_is_declared = required.contains(wrapper_key) || properties.contains(wrapper_key);
    if wrapper_is_declared {
        return parsed;
    }

    let outer_has_all_required = required.iter().all(|key| outer.contains_key(*key));
    let inner_has_all_required = required.iter().all(|key| inner.contains_key(*key));
    let outer_matches_properties = properties.iter().any(|key| outer.contains_key(*key));
    let inner_matches_properties = properties.iter().any(|key| inner.contains_key(*key));

    if inner_has_all_required && !outer_has_all_required {
        return Value::Object(inner.clone());
    }
    if inner_matches_properties && !outer_matches_properties {
        return Value::Object(inner.clone());
    }

    parsed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::OutputSchema;
    use serde_json::json;

    #[test]
    fn test_strip_code_fences_no_fences() {
        assert_eq!(
            strip_code_fences(r#"{"name": "test"}"#),
            r#"{"name": "test"}"#
        );
    }

    #[test]
    fn test_strip_code_fences_json_fence() {
        let input = r#"```json
{"name": "test"}
```"#;
        assert_eq!(strip_code_fences(input), r#"{"name": "test"}"#);
    }

    #[test]
    fn test_strip_code_fences_plain_fence() {
        let input = r#"```
{"name": "test"}
```"#;
        assert_eq!(strip_code_fences(input), r#"{"name": "test"}"#);
    }

    #[test]
    fn test_strip_code_fences_with_whitespace() {
        let input = r#"
```json
  {"name": "test"}
```
"#;
        assert_eq!(strip_code_fences(input), r#"{"name": "test"}"#);
    }

    #[test]
    fn test_unwrap_named_object_wrapper_when_inner_matches_schema()
    -> Result<(), Box<dyn std::error::Error>> {
        let schema = OutputSchema::new(json!({
            "type": "object",
            "properties": { "response": { "type": "string" } },
            "required": ["response"]
        }))?
        .with_name("advisor");

        let parsed = json!({
            "advisor": {
                "response": "hello"
            }
        });

        let normalized = unwrap_named_object_wrapper(parsed, &schema);
        assert_eq!(normalized, json!({"response": "hello"}));
        Ok(())
    }

    #[test]
    fn test_unwrap_named_object_wrapper_preserves_declared_wrapper_key()
    -> Result<(), Box<dyn std::error::Error>> {
        let schema = OutputSchema::new(json!({
            "type": "object",
            "properties": { "advisor": { "type": "object" } },
            "required": ["advisor"]
        }))?
        .with_name("advisor");

        let parsed = json!({
            "advisor": {
                "response": "hello"
            }
        });

        let normalized = unwrap_named_object_wrapper(parsed.clone(), &schema);
        assert_eq!(normalized, parsed);
        Ok(())
    }
}
