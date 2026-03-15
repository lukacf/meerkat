//! Anthropic model family detection, capabilities, and parameter schemas.

use super::ModelProfile;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Family detection
// ---------------------------------------------------------------------------

/// Returns `true` for any Claude model that supports extended thinking.
///
/// Currently all Claude models in the 4.x+ family support thinking.
pub fn supports_thinking(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.starts_with("claude-")
}

/// Returns `true` for models that support temperature.
///
/// All Anthropic models support temperature.
pub fn supports_temperature(_model: &str) -> bool {
    true
}

fn detect_family(model: &str) -> Option<&'static str> {
    let m = model.to_ascii_lowercase();
    if m.starts_with("claude-opus-4") {
        Some("claude-opus-4")
    } else if m.starts_with("claude-sonnet-4") {
        Some("claude-sonnet-4")
    } else if m.starts_with("claude-haiku-4") {
        Some("claude-haiku-4")
    } else if m.starts_with("claude-") {
        Some("claude")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Parameter schema
// ---------------------------------------------------------------------------

/// Anthropic-specific model parameters accepted via `provider_params`.
///
/// This struct drives the JSON Schema exposed in the catalog.
/// The actual request-building logic lives in `meerkat-client`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AnthropicModelParams {
    /// Extended thinking configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<AnthropicThinkingParam>,
}

/// Thinking configuration for Anthropic models.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum AnthropicThinkingParam {
    /// Adaptive thinking (Opus 4.6+): model decides thinking budget automatically.
    #[serde(rename = "adaptive")]
    Adaptive,
    /// Explicit thinking budget in tokens.
    #[serde(rename = "enabled")]
    Enabled {
        /// Token budget for thinking (e.g., 1024..128000).
        budget_tokens: u32,
    },
}

fn params_schema() -> &'static serde_json::Value {
    static SCHEMA: OnceLock<serde_json::Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let schema = schemars::schema_for!(AnthropicModelParams);
        serde_json::to_value(schema).unwrap_or_default()
    })
}

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

/// Build a profile for an Anthropic model, or `None` if unrecognized.
pub fn profile(model: &str) -> Option<ModelProfile> {
    let family = detect_family(model)?;
    Some(ModelProfile {
        provider: "anthropic".to_string(),
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
    fn claude_opus_detected() {
        assert_eq!(detect_family("claude-opus-4-6"), Some("claude-opus-4"));
        assert_eq!(detect_family("claude-opus-4-5"), Some("claude-opus-4"));
    }

    #[test]
    fn claude_sonnet_detected() {
        assert_eq!(detect_family("claude-sonnet-4-6"), Some("claude-sonnet-4"));
        assert_eq!(detect_family("claude-sonnet-4-5"), Some("claude-sonnet-4"));
    }

    #[test]
    fn all_claude_support_thinking() {
        assert!(supports_thinking("claude-opus-4-6"));
        assert!(supports_thinking("claude-sonnet-4-5"));
        assert!(supports_thinking("claude-haiku-4-5-20251001"));
    }

    #[test]
    fn non_claude_not_detected() {
        assert_eq!(detect_family("gpt-5.2"), None);
        assert!(!supports_thinking("gpt-5.2"));
    }

    #[test]
    fn schema_is_valid_object() {
        let schema = params_schema();
        assert!(schema.is_object(), "schema must be a JSON object");
    }
}
