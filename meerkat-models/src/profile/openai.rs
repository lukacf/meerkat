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
// Parameter schema
// ---------------------------------------------------------------------------

/// OpenAI-specific model parameters accepted via `provider_params`.
///
/// This struct drives the JSON Schema exposed in the catalog.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OpenAiModelParams {
    /// Reasoning effort level for GPT-5 models.
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

/// Reasoning effort levels for OpenAI models.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OpenAiReasoningEffort {
    Low,
    Medium,
    High,
}

fn params_schema() -> &'static serde_json::Value {
    static SCHEMA: OnceLock<serde_json::Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let schema = schemars::schema_for!(OpenAiModelParams);
        serde_json::to_value(schema).unwrap_or_default()
    })
}

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

/// Build a profile for an OpenAI model, or `None` if unrecognized.
pub fn profile(model: &str) -> Option<ModelProfile> {
    let family = detect_family(model)?;
    Some(ModelProfile {
        provider: "openai".to_string(),
        model_family: family.to_string(),
        supports_temperature: supports_temperature(model),
        supports_thinking: supports_thinking(model),
        supports_reasoning: supports_reasoning(model),
        params_schema: params_schema().clone(),
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
    fn non_openai_not_detected() {
        assert_eq!(detect_family("claude-opus-4-6"), None);
        assert!(!is_gpt5_family("claude-opus-4-6"));
    }

    #[test]
    fn schema_is_valid_object() {
        let schema = params_schema();
        assert!(schema.is_object(), "schema must be a JSON object");
    }
}
