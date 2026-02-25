//! Structured output extraction turn.
//!
//! When `output_schema` is configured, the agent performs an extraction turn
//! after the agentic loop completes to force validated JSON output.

use crate::error::AgentError;
use crate::types::{BlockAssistantMessage, Message, RunResult, UserMessage};
use jsonschema::Validator;
use serde_json::{Value, json};

use super::{Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};

impl<C, T, S> Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized + 'static,
    T: AgentToolDispatcher + ?Sized + 'static,
    S: AgentSessionStore + ?Sized + 'static,
{
    /// Perform the extraction turn to get structured output.
    ///
    /// This is called after the agentic loop completes (no more tool calls)
    /// when `output_schema` is configured. It prompts the LLM to produce
    /// validated JSON matching the schema.
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

        // Build the JSON schema validator
        let validator = Validator::new(&compiled.schema)
            .map_err(|e| AgentError::InvalidOutputSchema(e.to_string()))?;

        let max_attempts = self.config.structured_output_retries + 1;
        let mut last_error = String::new();
        let schema_warnings = if compiled.warnings.is_empty() {
            None
        } else {
            Some(compiled.warnings.clone())
        };

        for attempt in 0..max_attempts {
            // Add extraction prompt
            let prompt = if attempt == 0 {
                "Based on our conversation, provide the final output as valid JSON matching the required schema. Output ONLY the JSON, no additional text or markdown formatting.".to_string()
            } else {
                format!(
                    "The previous output was invalid: {}. Please provide valid JSON matching the schema. Output ONLY the JSON, no additional text.",
                    last_error
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
                    // Validate first using provider-compromised schema, then normalize
                    // known nullable string-literals to JSON null for downstream consumers.
                    // Validate against schema
                    match validator.validate(&parsed) {
                        Ok(()) => {
                            let mut normalized = parsed;
                            normalize_nullable_string_literals(&mut normalized, output_schema.schema.as_value());
                            // Success! Return with structured output
                            return Ok(RunResult {
                                text: self.session.last_assistant_text().unwrap_or_default(),
                                session_id: self.session.id().clone(),
                                usage: self.session.total_usage(),
                                turns: turn_count + 1 + attempt + 1, // Include extraction attempts
                                tool_calls: tool_call_count,
                                structured_output: Some(normalized),
                                schema_warnings: schema_warnings.clone(),
                                skill_diagnostics: None,
                            });
                        }
                        Err(error) => {
                            // Collect validation errors
                            last_error = format!("Schema validation failed: {}", error);
                        }
                    }
                }
                Err(e) => {
                    last_error = format!("Invalid JSON: {}", e);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::OutputSchema;

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
    fn test_normalize_nullable_string_literals_to_null() {
        let schema = OutputSchema::new(json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "date": { "type": ["string", "null"] },
                "count": { "type": ["integer", "null"] }
            },
            "required": ["name", "date", "count"]
        }))
        .expect("valid schema");

        let mut output = json!({
            "name": "Alice",
            "date": "null",
            "count": "null",
        });
        normalize_nullable_string_literals(&mut output, schema.schema.as_value());

        assert_eq!(output["name"], "Alice");
        assert!(output["date"].is_null());
        assert!(output["count"].is_null());
    }

    #[test]
    fn test_normalize_nullable_string_literals_leaves_required_strings() {
        let schema = OutputSchema::new(json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "date": { "type": "string" }
            },
            "required": ["name", "date"]
        }))
        .expect("valid schema");

        let mut output = json!({
            "name": "null",
            "date": "null",
        });
        normalize_nullable_string_literals(&mut output, schema.schema.as_value());

        assert_eq!(output["name"], "null");
        assert_eq!(output["date"], "null");
    }
}

fn normalize_nullable_string_literals(value: &mut Value, schema: &Value) {
    match value {
        Value::Object(obj) => {
            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                for (key, child) in obj.iter_mut() {
                    if let Some(child_schema) = properties.get(key) {
                        normalize_nullable_string_literals(child, child_schema);
                    } else if let Some(additional) = schema.get("additionalProperties")
                        && matches!(additional, Value::Object(_))
                    {
                        normalize_nullable_string_literals(child, additional);
                    }
                }
            }

            if let Some(items_schema) = schema.get("items") {
                for child in obj.values_mut() {
                    normalize_nullable_string_literals(child, items_schema);
                }
            }
        }
        Value::Array(values) => {
            if let Some(items_schema) = schema.get("items") {
                for value in values {
                    normalize_nullable_string_literals(value, items_schema);
                }
            }
        }
        Value::String(s) if s == "null" => {
            if schema_allows_json_null(schema) {
                *value = Value::Null;
            }
        }
        _ => {}
    }
}

fn schema_allows_json_null(schema: &Value) -> bool {
    if matches!(schema.get("type"), Some(Value::String(t)) if t == "null") {
        return true;
    }

    if let Some(Value::Array(types)) = schema.get("type")
        && types.iter().any(|t| t.as_str() == Some("null"))
    {
        return true;
    }

    if let Some(Value::Array(any_of)) = schema.get("anyOf") {
        return any_of.iter().any(schema_allows_json_null);
    }

    if let Some(Value::Array(one_of)) = schema.get("oneOf") {
        return one_of.iter().any(schema_allows_json_null);
    }

    if let Some(Value::Array(all_of)) = schema.get("allOf") {
        return all_of.iter().any(schema_allows_json_null);
    }

    false
}
