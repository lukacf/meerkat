//! Structured output extraction turn.
//!
//! When `output_schema` is configured, the agent performs an extraction turn
//! after the agentic loop completes to force validated JSON output.

use crate::error::AgentError;
use crate::types::{BlockAssistantMessage, Message, OutputSchema, RunResult, UserMessage};
#[cfg(feature = "jsonschema")]
use jsonschema::Validator;
use serde_json::{Value, json};

use super::{Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};

/// Default prompt for the structured output extraction turn.
pub const DEFAULT_EXTRACTION_PROMPT: &str = "Provide the final output as valid JSON matching \
    the required schema. Output ONLY the JSON, no additional text or markdown formatting.";

impl<C, T, S> Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized + 'static,
    T: AgentToolDispatcher + ?Sized + 'static,
    S: AgentSessionStore + ?Sized + 'static,
{
    /// Perform the extraction turn to get structured output.
    ///
    /// Deprecated: extraction is now integrated into the main state machine loop
    /// in state.rs. This function is retained only for reference.
    #[allow(dead_code)]
    pub(super) async fn perform_extraction_turn(
        &mut self,
        turn_count: u32,
        tool_call_count: u32,
    ) -> Result<RunResult, AgentError> {
        // SAFETY: This method is only called when output_schema is Some (checked by caller)
        let output_schema = self.config.output_schema.as_ref().ok_or_else(|| {
            AgentError::InternalError("perform_extraction_turn called without output_schema".into())
        })?;

        let compiled = self
            .client
            .compile_schema(output_schema)
            .map_err(|e| AgentError::InvalidOutputSchema(e.to_string()))?;

        // Build the JSON schema validator (not available on wasm32)
        #[cfg(feature = "jsonschema")]
        let validator = Validator::new(&compiled.schema)
            .map_err(|e| AgentError::InvalidOutputSchema(e.to_string()))?;

        let max_attempts = self.config.structured_output_retries + 1;
        let mut last_error = String::new();
        let schema_warnings = if compiled.warnings.is_empty() {
            None
        } else {
            Some(compiled.warnings.clone())
        };

        let extraction_prompt = self
            .config
            .extraction_prompt
            .as_deref()
            .unwrap_or(DEFAULT_EXTRACTION_PROMPT);

        for attempt in 0..max_attempts {
            // Add extraction prompt
            let prompt = if attempt == 0 {
                extraction_prompt.to_string()
            } else {
                format!(
                    "The previous output was invalid: {last_error}. Please provide valid JSON matching the schema. Output ONLY the JSON, no additional text."
                )
            };

            self.session
                .push(Message::User(UserMessage { content: prompt }));

            // Build provider params with structured output configuration
            let mut params = self
                .config
                .provider_params
                .clone()
                .unwrap_or_else(|| json!({}));

            // Add structured output params - providers will interpret this
            if let Some(obj) = params.as_object_mut() {
                obj.insert("structured_output".to_string(), output_schema.to_value());
            }

            // Call LLM with NO tools (extraction turn is pure text generation)
            let result = self
                .client
                .stream_response(
                    self.session.messages(),
                    &[], // No tools for extraction
                    self.config.max_tokens_per_turn,
                    Some(0.0), // Low temperature for deterministic output
                    Some(&params),
                )
                .await?;

            // Update budget + session usage
            self.budget.record_usage(&result.usage);
            self.session.record_usage(result.usage.clone());

            let (blocks, stop_reason, _usage) = result.into_parts();
            let assistant_msg = BlockAssistantMessage {
                blocks,
                stop_reason,
            };
            let content = assistant_msg.to_string();

            // Add assistant response to session
            self.session.push(Message::BlockAssistant(assistant_msg));

            // Try to parse and validate the output
            let content = content.trim();

            // Strip markdown code fences if present
            let json_content = strip_code_fences(content);

            match serde_json::from_str::<Value>(json_content) {
                Ok(parsed) => {
                    let normalized = unwrap_named_object_wrapper(parsed, output_schema);
                    // Validate against schema (when jsonschema is available)
                    #[cfg(feature = "jsonschema")]
                    {
                        match validator.validate(&normalized) {
                            Ok(()) => {}
                            Err(error) => {
                                last_error = format!("Schema validation failed: {error}");
                                continue;
                            }
                        }
                    }
                    #[cfg(not(feature = "jsonschema"))]
                    {
                        tracing::warn!(
                            "Structured output schema validation unavailable \
                             (jsonschema feature disabled). Accepting parsed JSON without \
                             schema validation."
                        );
                    }
                    // Success! Return with structured output
                    return Ok(RunResult {
                        text: self.session.last_assistant_text().unwrap_or_default(),
                        session_id: self.session.id().clone(),
                        usage: self.session.total_usage(),
                        turns: turn_count + 1 + attempt + 1,
                        tool_calls: tool_call_count,
                        structured_output: Some(normalized),
                        schema_warnings: schema_warnings.clone(),
                        skill_diagnostics: None,
                    });
                }
                Err(e) => {
                    last_error = format!("Invalid JSON: {e}");
                }
            }
        }

        // Exhausted retries
        Err(AgentError::StructuredOutputValidationFailed {
            attempts: max_attempts,
            reason: last_error,
            last_output: self.session.last_assistant_text().unwrap_or_default(),
        })
    }
}

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
