//! Build the JSON Schema advertised for a model's `provider_params` from a
//! [`ModelCapabilities`] row.
//!
//! The schema here is the single source of truth for UI-facing param shapes.
//! It replaces the previous approach of emitting schemars-derived schemas from
//! hand-written struct buckets (`AnthropicOpus46Params`, `OpenAiGpt5Params`,
//! etc.), which conflated unrelated models into the same bucket.

use crate::model_profile::capabilities::{ModelCapabilities, ThinkingSupport};
use serde_json::{Value, json};

/// Build the JSON Schema for a model's `provider_params`.
pub fn build_params_schema(caps: &ModelCapabilities) -> Value {
    match caps.provider {
        "anthropic" => build_anthropic_schema(caps),
        "openai" => build_openai_schema(caps),
        "gemini" => build_gemini_schema(caps),
        _ => json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {}
        }),
    }
}

// ---------------------------------------------------------------------------
// Anthropic
//
// Current hand-written shape (matched for parity):
// - thinking: oneOf
//     * { type: "adaptive" }                          (only when adaptive is supported)
//     * { type: "enabled", budget_tokens: integer }   (always supported where thinking != None)
// - thinking_budget: integer  (legacy flat alternative)
// - top_k: integer
// - effort: string enum       (only on models with effort_levels non-empty)
// - inference_geo: string     (only on models with supports_inference_geo)
// - compaction: object | "auto"  (only on models with supports_compaction)
// ---------------------------------------------------------------------------

fn build_anthropic_schema(caps: &ModelCapabilities) -> Value {
    let mut props = serde_json::Map::new();

    if let Some(thinking) = anthropic_thinking_schema(caps.thinking) {
        props.insert("thinking".into(), thinking);
    }
    if caps.supports_thinking_budget_legacy && caps.thinking != ThinkingSupport::None {
        props.insert("thinking_budget".into(), integer_nonneg_schema());
    }
    if caps.supports_top_k {
        props.insert("top_k".into(), integer_nonneg_schema());
    }
    if !caps.effort_levels.is_empty() {
        props.insert(
            "effort".into(),
            string_enum_schema("Output effort level.", caps.effort_levels),
        );
    }
    if caps.supports_inference_geo {
        props.insert(
            "inference_geo".into(),
            json!({
                "description": "Data residency region (e.g., \"us\" or \"global\").",
                "type": "string"
            }),
        );
    }
    if caps.supports_compaction {
        props.insert(
            "compaction".into(),
            json!({
                "description": "Context compaction. \"auto\" or an object like {\"trigger\": 150000}.",
            }),
        );
    }

    object_schema(props)
}

fn anthropic_thinking_schema(mode: ThinkingSupport) -> Option<Value> {
    match mode {
        ThinkingSupport::None | ThinkingSupport::GeminiThinkingLevel => None,
        ThinkingSupport::AnthropicEnabledOnly => Some(json!({
            "description": "Extended thinking configuration. Format: {\"type\": \"enabled\", \"budget_tokens\": N}.",
            "type": "object",
            "required": ["type", "budget_tokens"],
            "properties": {
                "type": { "type": "string", "enum": ["enabled"] },
                "budget_tokens": { "type": "integer", "minimum": 0 }
            }
        })),
        ThinkingSupport::AnthropicAdaptiveOnly => Some(json!({
            "description": "Extended thinking configuration. Format: {\"type\": \"adaptive\"}.",
            "type": "object",
            "required": ["type"],
            "properties": {
                "type": { "type": "string", "enum": ["adaptive"] }
            }
        })),
        ThinkingSupport::AnthropicAdaptiveAndEnabled => Some(json!({
            "description": "Extended thinking configuration. Format: {\"type\": \"adaptive\"} or {\"type\": \"enabled\", \"budget_tokens\": N}.",
            "oneOf": [
                {
                    "type": "object",
                    "required": ["type"],
                    "properties": {
                        "type": { "type": "string", "enum": ["adaptive"] }
                    }
                },
                {
                    "type": "object",
                    "required": ["type", "budget_tokens"],
                    "properties": {
                        "type": { "type": "string", "enum": ["enabled"] },
                        "budget_tokens": { "type": "integer", "minimum": 0 }
                    }
                }
            ]
        })),
    }
}

// ---------------------------------------------------------------------------
// OpenAI
//
// Current hand-written shape (matched for parity):
// - reasoning_effort: string enum  (only on reasoning models)
// - seed: integer
// - frequency_penalty: number
// - presence_penalty: number
// ---------------------------------------------------------------------------

fn build_openai_schema(caps: &ModelCapabilities) -> Value {
    let mut props = serde_json::Map::new();

    if caps.supports_reasoning && !caps.effort_levels.is_empty() {
        props.insert(
            "reasoning_effort".into(),
            string_enum_schema("Reasoning effort level.", caps.effort_levels),
        );
    }
    if caps.supports_legacy_penalties {
        props.insert(
            "seed".into(),
            json!({
                "description": "Random seed for reproducibility.",
                "type": "integer"
            }),
        );
        props.insert(
            "frequency_penalty".into(),
            json!({
                "description": "Frequency penalty (-2.0 to 2.0).",
                "type": "number"
            }),
        );
        props.insert(
            "presence_penalty".into(),
            json!({
                "description": "Presence penalty (-2.0 to 2.0).",
                "type": "number"
            }),
        );
    }

    object_schema(props)
}

// ---------------------------------------------------------------------------
// Gemini
//
// Current hand-written shape:
// - thinking: object { thinking_level, thinking_budget }
// - thinking_level: string enum                       (Gemini 3 authoritative knob)
// - thinking_budget: integer                          (legacy flat alternative)
// - top_k: integer
// - top_p: number
// ---------------------------------------------------------------------------

fn build_gemini_schema(caps: &ModelCapabilities) -> Value {
    let mut props = serde_json::Map::new();

    if caps.thinking != ThinkingSupport::None {
        let thinking_props = match caps.thinking {
            ThinkingSupport::GeminiThinkingLevel => json!({
                "thinking_level": gemini_thinking_level_schema(),
                "thinking_budget": { "type": "integer", "minimum": 0 }
            }),
            _ => json!({
                "thinking_budget": { "type": "integer", "minimum": 0 }
            }),
        };
        props.insert(
            "thinking".into(),
            json!({
                "description": "Thinking configuration.",
                "type": "object",
                "additionalProperties": false,
                "properties": thinking_props
            }),
        );
        if caps.thinking == ThinkingSupport::GeminiThinkingLevel {
            props.insert("thinking_level".into(), gemini_thinking_level_schema());
        }
        if caps.supports_thinking_budget_legacy {
            props.insert(
                "thinking_budget".into(),
                json!({
                    "description": "Legacy flat thinking budget (alternative to thinking.thinking_budget).",
                    "type": "integer",
                    "minimum": 0
                }),
            );
        }
    }
    if caps.supports_top_k {
        props.insert("top_k".into(), integer_nonneg_schema());
    }
    if caps.supports_top_p {
        props.insert(
            "top_p".into(),
            json!({
                "type": "number",
                "minimum": 0.0,
                "maximum": 1.0
            }),
        );
    }

    object_schema(props)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn object_schema(properties: serde_json::Map<String, Value>) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": Value::Object(properties)
    })
}

fn integer_nonneg_schema() -> Value {
    json!({ "type": "integer", "minimum": 0 })
}

fn gemini_thinking_level_schema() -> Value {
    json!({
        "description": "Gemini 3 reasoning level.",
        "type": "string",
        "enum": ["minimal", "low", "medium", "high"]
    })
}

fn string_enum_schema(description: &str, values: &[&str]) -> Value {
    let vs: Vec<Value> = values.iter().map(|s| Value::String((*s).into())).collect();
    json!({
        "description": description,
        "type": "string",
        "enum": vs
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::collapsible_match,
    clippy::redundant_closure,
    clippy::redundant_closure_for_method_calls
)]
mod tests {
    use super::*;
    use crate::model_profile::capabilities::{all_capabilities, capabilities_for};

    /// Extract the set of property names from a params schema.
    fn property_keys(schema: &Value) -> std::collections::BTreeSet<String> {
        schema
            .get("properties")
            .and_then(|p| p.as_object())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Extract the set of enum values for a top-level string-enum property.
    fn enum_values_for(schema: &Value, prop: &str) -> Option<std::collections::BTreeSet<String>> {
        let val = schema
            .get("properties")
            .and_then(|p| p.get(prop))
            .and_then(|v| v.get("enum"))
            .and_then(|e| e.as_array())?;
        Some(
            val.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
        )
    }

    #[test]
    fn builder_emits_properties_for_every_catalog_model() {
        for caps in all_capabilities() {
            let schema = build_params_schema(caps);
            assert!(
                schema.is_object(),
                "schema for {} must be a JSON object",
                caps.id
            );
            assert_eq!(
                schema.get("type").and_then(|t| t.as_str()),
                Some("object"),
                "schema for {} must be type=object",
                caps.id
            );
            assert!(
                schema.get("properties").is_some(),
                "schema for {} must have a properties map",
                caps.id
            );
        }
    }

    /// Parity test: new builder property keys match the current profile_for output.
    ///
    /// This is the regression net for commit 2 — it proves introducing the
    /// capability table does not change what the UI sees. When individual rows
    /// are later corrected (commit 4), the expected keys for those rows diverge
    /// intentionally; at that point the parity test is replaced by golden
    /// schemas per model.
    #[test]
    fn builder_property_keys_match_current_profile() {
        use crate::model_profile::profile_for;

        for caps in all_capabilities() {
            let built = build_params_schema(caps);
            let legacy = profile_for(caps.provider, caps.id)
                .unwrap_or_else(|| panic!("no profile for {}", caps.id))
                .params_schema;

            let built_keys = property_keys(&built);
            let legacy_keys = property_keys(&legacy);
            assert_eq!(
                built_keys, legacy_keys,
                "property keys differ for {} (left=built, right=legacy)",
                caps.id
            );
        }
    }

    /// Parity test: for string-enum properties, value sets match.
    #[test]
    fn builder_enum_values_match_current_profile() {
        use crate::model_profile::profile_for;

        for caps in all_capabilities() {
            let built = build_params_schema(caps);
            let legacy = profile_for(caps.provider, caps.id)
                .unwrap_or_else(|| panic!("no profile for {}", caps.id))
                .params_schema;

            let check_prop = |prop: &str| {
                if let (Some(built_values), legacy_values) = (
                    enum_values_for(&built, prop),
                    enum_values_for(&legacy, prop),
                ) {
                    // The legacy shape wraps the enum in a $ref to $defs; skip
                    // if the legacy schema doesn't expose the enum inline.
                    if let Some(legacy_values) = legacy_values {
                        assert_eq!(
                            built_values, legacy_values,
                            "enum values differ for {}.{}",
                            caps.id, prop
                        );
                    }
                }
            };
            check_prop("effort");
            check_prop("reasoning_effort");
        }
    }

    #[test]
    fn opus_47_effort_includes_xhigh() {
        let caps = capabilities_for("anthropic", "claude-opus-4-7").expect("opus 4.7 row");
        let schema = build_params_schema(caps);
        let values = enum_values_for(&schema, "effort").expect("effort enum");
        assert!(values.contains("xhigh"), "opus 4.7 must advertise xhigh");
        assert!(values.contains("low"));
        assert!(values.contains("max"));
    }

    #[test]
    fn sonnet_45_has_no_effort_property() {
        let caps = capabilities_for("anthropic", "claude-sonnet-4-5").expect("sonnet 4.5 row");
        let schema = build_params_schema(caps);
        let keys = property_keys(&schema);
        assert!(
            !keys.contains("effort"),
            "sonnet 4.5 must not advertise effort"
        );
    }

    #[test]
    fn gemini_3_schema_exposes_thinking_level() {
        let caps =
            capabilities_for("gemini", "gemini-3-flash-preview").expect("gemini 3 flash row");
        let schema = build_params_schema(caps);
        let keys = property_keys(&schema);
        assert!(
            keys.contains("thinking_level"),
            "gemini 3 must advertise thinking_level"
        );

        let values = enum_values_for(&schema, "thinking_level").expect("thinking_level enum");
        let expected: std::collections::BTreeSet<String> = ["high", "low", "medium", "minimal"]
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(values, expected);
    }

    #[test]
    fn gemini_schema_has_no_include_thoughts() {
        for caps in all_capabilities().filter(|c| c.provider == "gemini") {
            let schema = build_params_schema(caps);
            let keys = property_keys(&schema);
            assert!(
                !keys.contains("include_thoughts"),
                "gemini {} must not advertise include_thoughts (client ignores it)",
                caps.id,
            );
            // Inspect nested thinking object too.
            let thinking = schema.get("properties").and_then(|p| p.get("thinking"));
            if let Some(inner_props) = thinking.and_then(|t| t.get("properties")) {
                let obj = inner_props.as_object().expect("inner properties");
                assert!(
                    !obj.contains_key("include_thoughts"),
                    "gemini {} must not advertise thinking.include_thoughts",
                    caps.id,
                );
            }
        }
    }
}
