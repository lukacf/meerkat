//! OpenAI model family detection, capabilities, and parameter schemas.

use super::ModelProfile;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Family detection
// ---------------------------------------------------------------------------

/// Returns `true` if the model belongs to the GPT-5 family.
pub fn is_gpt5_family(model: &str) -> bool {
    model.to_ascii_lowercase().starts_with("gpt-5")
}

/// Returns `true` if the model is a Codex model.
pub fn is_codex_family(model: &str) -> bool {
    model.to_ascii_lowercase().contains("codex")
}

/// Returns `true` if the model accepts a `temperature` parameter.
///
/// GPT-5 and Codex models reject temperature.
/// Extracted from `meerkat-client/src/openai.rs:26-30`.
pub fn supports_temperature(model: &str) -> bool {
    !(is_gpt5_family(model) || is_codex_family(model))
}

/// Returns `true` if the model supports explicit reasoning payload (reasoning_effort).
///
/// Only GPT-5 family models support this.
/// Extracted from `meerkat-client/src/openai.rs:32-37`.
pub fn supports_reasoning(model: &str) -> bool {
    is_gpt5_family(model)
}

/// Returns `true` if the model supports thinking/reasoning.
///
/// GPT-5 family has built-in reasoning.
pub fn supports_thinking(model: &str) -> bool {
    is_gpt5_family(model)
}

fn detect_family(model: &str) -> Option<&'static str> {
    let m = model.to_ascii_lowercase();
    if m.contains("codex") {
        Some("codex")
    } else if m.starts_with("gpt-5") {
        Some("gpt-5")
    } else if m.starts_with("gpt-") || m.starts_with("chatgpt-") {
        Some("gpt")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Parameter schemas
//
// Two schemas: GPT-5 family (has reasoning_effort, no temperature) and
// standard GPT (has seed/penalties only, temperature handled separately).
// Matches what meerkat-client/src/openai.rs actually reads from
// provider_params (lines 117-156).
// ---------------------------------------------------------------------------

/// OpenAI parameters for GPT-5 family models.
///
/// These models support reasoning_effort but reject temperature.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OpenAiGpt5Params {
    /// Reasoning effort level. Only applied when model supports reasoning.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<OpenAiReasoningEffort>,
    /// Random seed for reproducibility.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// Frequency penalty (-2.0 to 2.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    /// Presence penalty (-2.0 to 2.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
}

/// OpenAI parameters for non-GPT-5 models.
///
/// These models do not support reasoning_effort.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OpenAiStandardParams {
    /// Random seed for reproducibility.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// Frequency penalty (-2.0 to 2.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    /// Presence penalty (-2.0 to 2.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
}

/// Reasoning effort levels for OpenAI models.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OpenAiReasoningEffort {
    Low,
    Medium,
    High,
}

fn gpt5_params_schema() -> &'static serde_json::Value {
    static SCHEMA: OnceLock<serde_json::Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let schema = schemars::schema_for!(OpenAiGpt5Params);
        serde_json::to_value(schema).unwrap_or_default()
    })
}

fn standard_params_schema() -> &'static serde_json::Value {
    static SCHEMA: OnceLock<serde_json::Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let schema = schemars::schema_for!(OpenAiStandardParams);
        serde_json::to_value(schema).unwrap_or_default()
    })
}

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

/// Build a profile for an OpenAI model, or `None` if unrecognized.
pub fn profile(model: &str) -> Option<ModelProfile> {
    let family = detect_family(model)?;
    let schema = if is_gpt5_family(model) {
        gpt5_params_schema()
    } else {
        standard_params_schema()
    };
    // GPT-5 family models have reasoning and may take much longer per call.
    // Pro variants (gpt-5.4-pro, gpt-5.2-pro) are heavy reasoning models that
    // can legitimately take very long for a single call.
    // Codex models are coding-oriented with extended reasoning.
    let m_lower = model.to_ascii_lowercase();
    let call_timeout_secs = if m_lower.contains("-pro") && is_gpt5_family(model) {
        Some(7200) // 2 hours: heavy reasoning pro model
    } else {
        match family {
            "gpt-5" => Some(600), // 10 minutes: standard reasoning model
            "codex" => Some(600), // 10 minutes: coding-oriented with reasoning
            "gpt" => Some(90),    // 90 seconds: standard chat model
            _ => None,
        }
    };
    Some(ModelProfile {
        provider: "openai".to_string(),
        model_family: family.to_string(),
        supports_temperature: supports_temperature(model),
        supports_thinking: supports_thinking(model),
        supports_reasoning: supports_reasoning(model),
        vision: true,
        image_tool_results: false,
        params_schema: schema.clone(),
        call_timeout_secs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpt5_family_detected() {
        assert!(is_gpt5_family("gpt-5.2"));
        assert!(is_gpt5_family("gpt-5.2-pro"));
        assert!(is_gpt5_family("GPT-5.4"));
    }

    #[test]
    fn codex_family_detected() {
        assert!(is_codex_family("gpt-5.3-codex"));
        assert!(!is_codex_family("gpt-5.2"));
    }

    #[test]
    fn gpt5_rejects_temperature() {
        assert!(!supports_temperature("gpt-5.2"));
        assert!(!supports_temperature("gpt-5.2-pro"));
    }

    #[test]
    fn codex_rejects_temperature() {
        assert!(!supports_temperature("gpt-5.3-codex"));
    }

    #[test]
    fn gpt5_supports_reasoning() {
        assert!(supports_reasoning("gpt-5.2"));
        assert!(supports_reasoning("gpt-5.4"));
    }

    #[test]
    fn gpt5_schema_includes_reasoning_effort() {
        let schema = gpt5_params_schema();
        let props = schema.get("properties").and_then(|p| p.as_object());
        assert!(props.is_some(), "schema must have properties");
        let props = props.unwrap_or(&serde_json::Map::new()).clone();
        assert!(
            props.contains_key("reasoning_effort"),
            "must include reasoning_effort"
        );
        assert!(props.contains_key("seed"), "must include seed");
        assert!(
            props.contains_key("frequency_penalty"),
            "must include frequency_penalty"
        );
        assert!(
            props.contains_key("presence_penalty"),
            "must include presence_penalty"
        );
    }

    #[test]
    fn standard_schema_no_reasoning_effort() {
        let schema = standard_params_schema();
        let props = schema.get("properties").and_then(|p| p.as_object());
        assert!(props.is_some(), "schema must have properties");
        let props = props.unwrap_or(&serde_json::Map::new()).clone();
        assert!(
            !props.contains_key("reasoning_effort"),
            "standard must not include reasoning_effort"
        );
        assert!(props.contains_key("seed"), "must include seed");
    }

    #[test]
    fn per_model_schema_differs() {
        let gpt5 = profile("gpt-5.2");
        // No non-gpt5 models in catalog, but test the function directly
        let standard_schema = standard_params_schema();
        let gpt5_schema = gpt5_params_schema();
        assert_ne!(
            standard_schema, gpt5_schema,
            "gpt-5 and standard schemas must differ"
        );
        assert!(gpt5.is_some());
    }

    #[test]
    fn non_openai_not_detected() {
        assert_eq!(detect_family("claude-opus-4-6"), None);
        assert!(!is_gpt5_family("claude-opus-4-6"));
    }

    #[test]
    fn schemas_are_valid_objects() {
        assert!(gpt5_params_schema().is_object());
        assert!(standard_params_schema().is_object());
    }
}
