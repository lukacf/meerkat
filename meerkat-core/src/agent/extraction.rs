//! Structured output extraction authority.
//!
//! Extraction is integrated into the main state machine loop in `state.rs`.
//! This module owns the structured-output prompt, validation normalization,
//! retry prompt, warnings, and projected result for that post-run phase.

use crate::error::AgentError;
use crate::schema::SchemaWarning;
use crate::turn_execution_authority::StructuredOutputFailureReason;
use crate::types::{ExtractionError, OutputSchema};
use serde_json::Value;

/// Default prompt for the structured output extraction turn.
const DEFAULT_EXTRACTION_PROMPT: &str = "Provide the final output as valid JSON matching \
    the required schema. Output ONLY the JSON, no additional text or markdown formatting.";

#[derive(Debug, Default)]
pub(crate) struct StructuredOutputExtractionAuthority {
    primary_output: Option<String>,
    result: Option<Value>,
    schema_warnings: Option<Vec<SchemaWarning>>,
}

impl StructuredOutputExtractionAuthority {
    pub(super) fn reset(&mut self) {
        self.primary_output = None;
        self.result = None;
        self.schema_warnings = None;
    }

    pub(super) fn begin_extraction(&mut self, output: String) {
        self.reset();
        self.primary_output = Some(output);
    }

    pub(super) fn primary_output(&self) -> Option<&str> {
        self.primary_output.as_deref()
    }

    pub(super) fn record_schema_warnings(&mut self, warnings: Vec<SchemaWarning>) {
        self.schema_warnings = if warnings.is_empty() {
            None
        } else {
            Some(warnings)
        };
    }

    pub(super) fn initial_prompt(&self, configured_prompt: Option<&str>) -> String {
        configured_prompt
            .map(str::to_owned)
            .unwrap_or_else(|| DEFAULT_EXTRACTION_PROMPT.to_string())
    }

    pub(super) fn validate_and_record_response(
        &mut self,
        content: &str,
        output_schema: &OutputSchema,
        compiled_schema: &Value,
    ) -> Result<ExtractionValidation, AgentError> {
        match validate_response_text(content, output_schema, compiled_schema)? {
            ValidationOutcome::Passed(value) => {
                self.result = Some(value);
                Ok(ExtractionValidation::Passed)
            }
            ValidationOutcome::Failed(failure) => Ok(ExtractionValidation::Failed(failure)),
        }
    }

    pub(super) fn finish_success(
        &mut self,
    ) -> Result<StructuredOutputExtractionSuccess, AgentError> {
        let structured_output = self.result.take().ok_or_else(|| {
            AgentError::InternalError(
                "structured output extraction finished without a recorded result".to_string(),
            )
        })?;
        Ok(StructuredOutputExtractionSuccess {
            text: self.primary_output.clone().unwrap_or_default(),
            structured_output,
            schema_warnings: self.schema_warnings.take(),
        })
    }

    pub(super) fn finish_failure(
        &mut self,
        attempts: u32,
        reason: String,
    ) -> StructuredOutputExtractionFailure {
        let extraction_error = ExtractionError {
            last_output: self.primary_output.clone().unwrap_or_default(),
            attempts,
            reason,
        };
        StructuredOutputExtractionFailure {
            extraction_error,
            schema_warnings: self.schema_warnings.take(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum ExtractionValidation {
    Passed,
    Failed(StructuredOutputValidationFailure),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct StructuredOutputValidationFailure {
    pub(super) reason: StructuredOutputFailureReason,
    pub(super) retry_prompt: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct StructuredOutputExtractionSuccess {
    pub(super) text: String,
    pub(super) structured_output: Value,
    pub(super) schema_warnings: Option<Vec<SchemaWarning>>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct StructuredOutputExtractionFailure {
    pub(super) extraction_error: ExtractionError,
    pub(super) schema_warnings: Option<Vec<SchemaWarning>>,
}

#[derive(Debug, Clone, PartialEq)]
enum ValidationOutcome {
    Passed(Value),
    Failed(StructuredOutputValidationFailure),
}

fn validate_response_text(
    content: &str,
    output_schema: &OutputSchema,
    compiled_schema: &Value,
) -> Result<ValidationOutcome, AgentError> {
    let json_content = strip_code_fences(content.trim());
    let parsed = match serde_json::from_str::<Value>(json_content) {
        Ok(parsed) => parsed,
        Err(error) => {
            return Ok(invalid_validation(format!("Invalid JSON: {error}")));
        }
    };

    #[cfg(feature = "jsonschema")]
    {
        let normalized = unwrap_named_object_wrapper(parsed, output_schema);
        let validator = jsonschema::Validator::new(compiled_schema)
            .map_err(|error| AgentError::InvalidOutputSchema(error.to_string()))?;
        if let Err(error) = validator.validate(&normalized) {
            return Ok(invalid_validation(format!(
                "Schema validation failed: {error}"
            )));
        }
        Ok(ValidationOutcome::Passed(normalized))
    }
    #[cfg(not(feature = "jsonschema"))]
    {
        let _ = parsed;
        let _ = output_schema;
        let _ = compiled_schema;
        Err(AgentError::InvalidOutputSchema(
            "Structured output schema validation unavailable \
            (jsonschema feature disabled). Refusing schema-governed output without validation."
                .to_string(),
        ))
    }
}

fn invalid_validation(error: String) -> ValidationOutcome {
    let retry_prompt = retry_prompt_for_error(&error);
    ValidationOutcome::Failed(StructuredOutputValidationFailure {
        reason: StructuredOutputFailureReason::new(error),
        retry_prompt,
    })
}

fn retry_prompt_for_error(error: &str) -> String {
    format!(
        "The previous output was invalid: {error}. \
        Please provide valid JSON matching the schema. \
        Output ONLY the JSON, no additional text."
    )
}

/// Strip markdown code fences from JSON content.
///
/// LLMs sometimes wrap JSON in ```json ... ``` even when asked not to.
fn strip_code_fences(content: &str) -> &str {
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
fn unwrap_named_object_wrapper(parsed: Value, output_schema: &OutputSchema) -> Value {
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
#[allow(clippy::panic)]
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

    #[test]
    fn authority_returns_typed_retry_prompt_for_invalid_json()
    -> Result<(), Box<dyn std::error::Error>> {
        let schema = OutputSchema::new(json!({
            "type": "object",
            "properties": { "answer": { "type": "string" } },
            "required": ["answer"]
        }))?;
        let mut authority = StructuredOutputExtractionAuthority::default();

        let result = authority.validate_and_record_response(
            "not json {{{",
            &schema,
            schema.schema.as_value(),
        )?;

        match result {
            ExtractionValidation::Failed(failure) => {
                assert!(failure.reason.message().contains("Invalid JSON"));
                assert!(failure.retry_prompt.contains(failure.reason.message()));
                assert!(failure.retry_prompt.contains("Output ONLY the JSON"));
            }
            ExtractionValidation::Passed => panic!("expected invalid JSON failure"),
        }
        Ok(())
    }

    #[test]
    fn test_validate_response_text_rejects_schema_mismatch()
    -> Result<(), Box<dyn std::error::Error>> {
        let schema = OutputSchema::new(json!({
            "type": "object",
            "properties": { "count": { "type": "integer" } },
            "required": ["count"]
        }))?;

        let result =
            validate_response_text(r#"{"count":"wrong"}"#, &schema, schema.schema.as_value())?;

        match result {
            ValidationOutcome::Failed(failure) => {
                assert!(
                    failure
                        .reason
                        .message()
                        .contains("Schema validation failed")
                );
            }
            ValidationOutcome::Passed(value) => {
                panic!("expected schema failure, got {value:?}")
            }
        }
        Ok(())
    }

    #[cfg(not(feature = "jsonschema"))]
    #[test]
    fn test_validate_response_text_fails_closed_without_jsonschema()
    -> Result<(), Box<dyn std::error::Error>> {
        let schema = OutputSchema::new(json!({
            "type": "object",
            "properties": { "answer": { "type": "string" } },
            "required": ["answer"]
        }))?;

        let error = validate_response_text(r#"{"answer":"ok"}"#, &schema, schema.schema.as_value())
            .expect_err("schema-governed output must not pass without jsonschema");

        assert!(matches!(error, AgentError::InvalidOutputSchema(_)));
        Ok(())
    }

    #[test]
    fn test_validate_response_text_accepts_unwrapped_schema_match()
    -> Result<(), Box<dyn std::error::Error>> {
        let schema = OutputSchema::new(json!({
            "type": "object",
            "properties": { "response": { "type": "string" } },
            "required": ["response"]
        }))?
        .with_name("advisor");

        let result = validate_response_text(
            r#"{"advisor":{"response":"hello"}}"#,
            &schema,
            schema.schema.as_value(),
        )?;

        assert_eq!(
            result,
            ValidationOutcome::Passed(json!({"response": "hello"}))
        );
        Ok(())
    }

    #[test]
    fn authority_records_success_and_projects_result() -> Result<(), Box<dyn std::error::Error>> {
        let schema = OutputSchema::new(json!({
            "type": "object",
            "properties": { "response": { "type": "string" } },
            "required": ["response"]
        }))?
        .with_name("advisor");
        let mut authority = StructuredOutputExtractionAuthority::default();
        authority.begin_extraction("main answer".to_string());

        let validation = authority.validate_and_record_response(
            "```json\n{\"advisor\":{\"response\":\"hello\"}}\n```",
            &schema,
            schema.schema.as_value(),
        )?;

        assert_eq!(validation, ExtractionValidation::Passed);
        let success = authority.finish_success()?;
        assert_eq!(success.text, "main answer");
        assert_eq!(success.structured_output, json!({"response": "hello"}));
        assert_eq!(success.schema_warnings, None);
        Ok(())
    }
}
