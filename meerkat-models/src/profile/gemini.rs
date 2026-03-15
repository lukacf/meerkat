//! Gemini model family detection, capabilities, and parameter schemas.

use super::ModelProfile;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Family detection
// ---------------------------------------------------------------------------

/// Returns `true` if the model supports thinking mode.
///
/// Gemini 3+ models support thinking.
pub fn supports_thinking(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.starts_with("gemini-3") || m.starts_with("gemini-4")
}

/// Returns `true` if the model accepts a `temperature` parameter.
///
/// All Gemini models support temperature.
pub fn supports_temperature(_model: &str) -> bool {
    true
}

fn detect_family(model: &str) -> Option<&'static str> {
    let m = model.to_ascii_lowercase();
    if m.starts_with("gemini-3") {
        Some("gemini-3")
    } else if m.starts_with("gemini-2") {
        Some("gemini-2")
    } else if m.starts_with("gemini-1") {
        Some("gemini-1")
    } else if m.starts_with("gemini-") {
        Some("gemini")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Parameter schema
//
// Matches what meerkat-client/src/gemini.rs actually reads from
// provider_params (lines 199-238):
//   - thinking_budget (flat): legacy format
//   - thinking.thinking_budget: nested format
//   - top_k: sampling param
//   - top_p: sampling param
//
// Note: the adapter does NOT read `thinking.include_thoughts`.
// The typed GeminiParams in meerkat-client/src/types.rs has this field,
// but the adapter only extracts thinking_budget from the nested object.
// ---------------------------------------------------------------------------

/// Gemini-specific model parameters accepted via `provider_params`.
///
/// This struct documents what the Gemini adapter actually reads.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GeminiModelParams {
    /// Thinking configuration (nested format).
    /// The adapter reads `thinking.thinking_budget` from this object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<GeminiThinkingParam>,
    /// Legacy flat thinking budget (alternative to `thinking.thinking_budget`).
    /// The adapter checks this first, then falls back to the nested format.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u64>,
    /// Top-K sampling parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Top-P (nucleus) sampling parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
}

/// Nested thinking configuration for Gemini models.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GeminiThinkingParam {
    /// Token budget for thinking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u64>,
}

fn params_schema() -> &'static serde_json::Value {
    static SCHEMA: OnceLock<serde_json::Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let schema = schemars::schema_for!(GeminiModelParams);
        serde_json::to_value(schema).unwrap_or_default()
    })
}

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

/// Build a profile for a Gemini model, or `None` if unrecognized.
pub fn profile(model: &str) -> Option<ModelProfile> {
    let family = detect_family(model)?;
    Some(ModelProfile {
        provider: "gemini".to_string(),
        model_family: family.to_string(),
        supports_temperature: supports_temperature(model),
        supports_thinking: supports_thinking(model),
        supports_reasoning: false,
        params_schema: params_schema().clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini3_supports_thinking() {
        assert!(supports_thinking("gemini-3-flash-preview"));
        assert!(supports_thinking("gemini-3-pro-preview"));
        assert!(supports_thinking("gemini-3.1-pro-preview"));
    }

    #[test]
    fn gemini2_no_thinking() {
        assert!(!supports_thinking("gemini-2.0-flash"));
    }

    #[test]
    fn gemini3_family_detected() {
        assert_eq!(detect_family("gemini-3-flash-preview"), Some("gemini-3"));
        assert_eq!(detect_family("gemini-3.1-pro-preview"), Some("gemini-3"));
    }

    #[test]
    fn non_gemini_not_detected() {
        assert_eq!(detect_family("claude-opus-4-6"), None);
        assert_eq!(detect_family("gpt-5.2"), None);
    }

    #[test]
    fn schema_matches_adapter_params() {
        let schema = params_schema();
        let props = schema.get("properties").and_then(|p| p.as_object());
        assert!(props.is_some(), "schema must have properties");
        let props = props.unwrap_or(&serde_json::Map::new()).clone();
        // Must include what the adapter reads
        assert!(props.contains_key("thinking"), "must include thinking");
        assert!(
            props.contains_key("thinking_budget"),
            "must include thinking_budget (flat)"
        );
        assert!(props.contains_key("top_k"), "must include top_k");
        assert!(props.contains_key("top_p"), "must include top_p");
        // Must NOT include fields the adapter ignores
        assert!(
            !props.contains_key("include_thoughts"),
            "must not include include_thoughts (adapter ignores it)"
        );
    }

    #[test]
    fn schema_is_valid_object() {
        let schema = params_schema();
        assert!(schema.is_object(), "schema must be a JSON object");
    }
}
