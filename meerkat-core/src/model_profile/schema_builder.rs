//! Build the JSON Schema advertised for the provider-specific payload inside a
//! model's canonical `provider_params.provider_tag` from a [`ModelCapabilities`]
//! row. The outer `ProviderParamsOverride` envelope is owned by the public wire
//! contract; this builder describes only the selected provider variant's knobs.
//!
//! The schema here is the single source of truth for UI-facing param shapes.
//! It is pure mechanics over the capability vocabulary: the function maps a
//! capability row to a schema and consults no catalog data. The canonical
//! rows live in `meerkat-models`, whose tests pin the emitted schema for
//! every real model.

use crate::Provider;
use crate::model_profile::capabilities::{EffortLevel, ModelCapabilities, ThinkingSupport};
use serde_json::{Value, json};

/// Build the JSON Schema for a model's `provider_params`.
pub fn build_params_schema(caps: &ModelCapabilities) -> Value {
    match caps.provider {
        Provider::Anthropic => build_anthropic_schema(caps),
        Provider::OpenAI => build_openai_schema(caps),
        Provider::Gemini => build_gemini_schema(caps),
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
            effort_enum_schema("Output effort level.", caps.effort_levels),
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
// Provider-tag payload shape:
// - reasoning_effort: string enum  (only on reasoning models)
// - reasoning_mode: string enum    (GPT-5.6 Responses)
// - reasoning_context: string enum (GPT-5.6 Responses)
// - text_verbosity: string enum    (GPT-5.6 Responses)
// - prompt_cache_options: object   (GPT-5.6 Responses)
// - seed: integer
// - frequency_penalty: number
// - presence_penalty: number
// ---------------------------------------------------------------------------

fn build_openai_schema(caps: &ModelCapabilities) -> Value {
    let mut props = serde_json::Map::new();

    if caps.supports_reasoning && !caps.effort_levels.is_empty() {
        props.insert(
            "reasoning_effort".into(),
            effort_enum_schema("Reasoning effort level.", caps.effort_levels),
        );
    }
    if let Some(advanced) = caps.openai_responses_params {
        if !advanced.reasoning_modes.is_empty() {
            props.insert(
                "reasoning_mode".into(),
                string_enum_schema(
                    "Responses reasoning execution mode.",
                    advanced
                        .reasoning_modes
                        .iter()
                        .map(|mode| mode.as_wire_str()),
                ),
            );
        }
        if !advanced.reasoning_contexts.is_empty() {
            props.insert(
                "reasoning_context".into(),
                string_enum_schema(
                    "Reasoning items made available to the next sample.",
                    advanced
                        .reasoning_contexts
                        .iter()
                        .map(|context| context.as_wire_str()),
                ),
            );
        }
        if !advanced.text_verbosity_levels.is_empty() {
            props.insert(
                "text_verbosity".into(),
                string_enum_schema(
                    "Default detail level for text output.",
                    advanced
                        .text_verbosity_levels
                        .iter()
                        .map(|level| level.as_wire_str()),
                ),
            );
        }

        let mut cache_props = serde_json::Map::new();
        if !advanced.prompt_cache_modes.is_empty() {
            cache_props.insert(
                "mode".into(),
                string_enum_schema(
                    "Request-wide prompt-cache breakpoint policy.",
                    advanced
                        .prompt_cache_modes
                        .iter()
                        .map(|mode| mode.as_wire_str()),
                ),
            );
        }
        if !advanced.prompt_cache_ttls.is_empty() {
            cache_props.insert(
                "ttl".into(),
                string_enum_schema(
                    "Minimum lifetime for prompt-cache entries.",
                    advanced
                        .prompt_cache_ttls
                        .iter()
                        .map(|ttl| ttl.as_wire_str()),
                ),
            );
        }
        if !cache_props.is_empty() {
            props.insert(
                "prompt_cache_options".into(),
                json!({
                    "description": "GPT-5.6 prompt-cache controls.",
                    "type": "object",
                    "additionalProperties": false,
                    "properties": Value::Object(cache_props)
                }),
            );
        }
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

/// Project a typed [`EffortLevel`] set into a JSON Schema string-enum.
///
/// The enum values derive from the typed vocabulary via
/// [`EffortLevel::as_wire_str`], not from inline string literals, so the
/// advertised schema cannot drift from the catalog's declared levels.
fn effort_enum_schema(description: &str, levels: &[EffortLevel]) -> Value {
    let vs: Vec<Value> = levels
        .iter()
        .map(|level| Value::String(level.as_wire_str().into()))
        .collect();
    json!({
        "description": description,
        "type": "string",
        "enum": vs
    })
}

fn string_enum_schema(description: &str, values: impl Iterator<Item = &'static str>) -> Value {
    let values: Vec<Value> = values.map(|value| Value::String(value.into())).collect();
    json!({
        "description": description,
        "type": "string",
        "enum": values
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::model_profile::test_catalog::TEST_CATALOG;

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
    fn builder_emits_object_schema_for_every_capability_row() {
        for caps in TEST_CATALOG.capabilities {
            let schema = build_params_schema(caps);
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

    #[test]
    fn effort_enum_values_derive_from_typed_effort_levels() {
        use crate::model_profile::capabilities::{EffortLevel, ModelCapabilities, ThinkingSupport};

        let base = TEST_CATALOG
            .capabilities_for(crate::Provider::Anthropic, "test-anthropic-default")
            .expect("test anthropic row");
        let caps = ModelCapabilities {
            thinking: ThinkingSupport::AnthropicAdaptiveAndEnabled,
            effort_levels: &[EffortLevel::Low, EffortLevel::High, EffortLevel::Max],
            ..*base
        };
        let schema = build_params_schema(&caps);
        let values = enum_values_for(&schema, "effort").expect("effort enum");
        let declared: std::collections::BTreeSet<String> = caps
            .effort_levels
            .iter()
            .map(|level| level.as_wire_str().to_string())
            .collect();
        assert_eq!(
            values, declared,
            "effort enum must equal the row's typed effort_levels"
        );
    }

    #[test]
    fn empty_effort_levels_emit_no_effort_property() {
        let caps = TEST_CATALOG
            .capabilities_for(crate::Provider::Anthropic, "test-anthropic-default")
            .expect("test anthropic row");
        assert!(caps.effort_levels.is_empty());
        let schema = build_params_schema(caps);
        assert!(!property_keys(&schema).contains("effort"));
    }

    #[test]
    fn gemini_thinking_level_rows_expose_thinking_level() {
        let caps = TEST_CATALOG
            .capabilities_for(crate::Provider::Gemini, "test-gemini-video")
            .expect("test gemini row");
        let schema = build_params_schema(caps);
        let keys = property_keys(&schema);
        assert!(
            keys.contains("thinking_level"),
            "GeminiThinkingLevel rows must advertise thinking_level"
        );
        let values = enum_values_for(&schema, "thinking_level").expect("thinking_level enum");
        let expected: std::collections::BTreeSet<String> = ["high", "low", "medium", "minimal"]
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(values, expected);
    }

    #[test]
    fn gemini_rows_have_no_include_thoughts() {
        let caps = TEST_CATALOG
            .capabilities_for(crate::Provider::Gemini, "test-gemini-video")
            .expect("test gemini row");
        let schema = build_params_schema(caps);
        assert!(!property_keys(&schema).contains("include_thoughts"));
        let thinking = schema.get("properties").and_then(|p| p.get("thinking"));
        if let Some(inner_props) = thinking.and_then(|t| t.get("properties")) {
            let obj = inner_props.as_object().expect("inner properties");
            assert!(!obj.contains_key("include_thoughts"));
        }
    }
}
