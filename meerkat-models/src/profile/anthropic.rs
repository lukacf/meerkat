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

/// Returns `true` for Opus 4.6+ models that support adaptive thinking.
fn supports_adaptive_thinking(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.starts_with("claude-opus-4-6") || m.starts_with("claude-opus-4-7")
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
// Parameter schemas
//
// Two distinct schemas: one for Opus 4.6+ (supports adaptive thinking,
// effort, inference_geo, compaction) and one for other Claude models.
// Both match what meerkat-client/src/anthropic.rs actually reads.
// ---------------------------------------------------------------------------

/// Anthropic parameters for Opus 4.6+ models.
///
/// Includes adaptive thinking, effort, inference_geo, and compaction.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AnthropicOpus46Params {
    /// Extended thinking configuration.
    /// Format: `{"type": "adaptive"}` or `{"type": "enabled", "budget_tokens": N}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<AnthropicThinkingWithAdaptive>,
    /// Legacy flat thinking budget (alternative to `thinking`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u64>,
    /// Top-K sampling parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Output effort level.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<AnthropicEffort>,
    /// Data residency region (e.g., `"us"`, `"global"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inference_geo: Option<String>,
    /// Context compaction. `"auto"` or an object like `{"trigger": 150000}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compaction: Option<serde_json::Value>,
}

/// Anthropic parameters for non-Opus-4.6 models (Sonnet, Haiku, older Opus).
///
/// Supports enabled thinking only (not adaptive), plus top_k.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AnthropicStandardParams {
    /// Extended thinking configuration.
    /// Format: `{"type": "enabled", "budget_tokens": N}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<AnthropicThinkingEnabled>,
    /// Legacy flat thinking budget (alternative to `thinking`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u64>,
    /// Top-K sampling parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
}

/// Thinking configuration for Opus 4.6+ (includes adaptive mode).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum AnthropicThinkingWithAdaptive {
    /// Adaptive thinking: model decides thinking budget automatically.
    #[serde(rename = "adaptive")]
    Adaptive,
    /// Explicit thinking budget in tokens.
    #[serde(rename = "enabled")]
    Enabled {
        /// Token budget for thinking (e.g., 1024..128000).
        budget_tokens: u32,
    },
}

/// Thinking configuration for standard Claude models (enabled only).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AnthropicThinkingEnabled {
    /// Must be `"enabled"`.
    #[serde(rename = "type")]
    pub thinking_type: String,
    /// Token budget for thinking (e.g., 1024..128000).
    pub budget_tokens: u32,
}

/// Effort levels for Anthropic output config.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AnthropicEffort {
    Low,
    Medium,
    High,
    Max,
}

fn opus46_params_schema() -> &'static serde_json::Value {
    static SCHEMA: OnceLock<serde_json::Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let schema = schemars::schema_for!(AnthropicOpus46Params);
        serde_json::to_value(schema).unwrap_or_default()
    })
}

fn standard_params_schema() -> &'static serde_json::Value {
    static SCHEMA: OnceLock<serde_json::Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let schema = schemars::schema_for!(AnthropicStandardParams);
        serde_json::to_value(schema).unwrap_or_default()
    })
}

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

/// Build a profile for an Anthropic model, or `None` if unrecognized.
pub fn profile(model: &str) -> Option<ModelProfile> {
    let family = detect_family(model)?;
    let schema = if supports_adaptive_thinking(model) {
        opus46_params_schema()
    } else {
        standard_params_schema()
    };
    Some(ModelProfile {
        provider: "anthropic".to_string(),
        model_family: family.to_string(),
        supports_temperature: supports_temperature(model),
        supports_thinking: supports_thinking(model),
        supports_reasoning: false,
        params_schema: schema.clone(),
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
    fn adaptive_thinking_only_opus_46() {
        assert!(supports_adaptive_thinking("claude-opus-4-6"));
        assert!(!supports_adaptive_thinking("claude-sonnet-4-6"));
        assert!(!supports_adaptive_thinking("claude-opus-4-5"));
        assert!(!supports_adaptive_thinking("claude-sonnet-4-5"));
    }

    #[test]
    fn opus46_schema_includes_effort_and_compaction() {
        let schema = opus46_params_schema();
        let props = schema.get("properties").and_then(|p| p.as_object());
        assert!(props.is_some(), "schema must have properties");
        let props = props.unwrap_or(&serde_json::Map::new()).clone();
        assert!(props.contains_key("effort"), "must include effort");
        assert!(props.contains_key("compaction"), "must include compaction");
        assert!(
            props.contains_key("inference_geo"),
            "must include inference_geo"
        );
        assert!(
            props.contains_key("thinking_budget"),
            "must include thinking_budget"
        );
        assert!(props.contains_key("top_k"), "must include top_k");
    }

    #[test]
    fn standard_schema_no_adaptive_or_effort() {
        let schema = standard_params_schema();
        let props = schema.get("properties").and_then(|p| p.as_object());
        assert!(props.is_some(), "schema must have properties");
        let props = props.unwrap_or(&serde_json::Map::new()).clone();
        assert!(
            !props.contains_key("effort"),
            "standard must not include effort"
        );
        assert!(
            !props.contains_key("compaction"),
            "standard must not include compaction"
        );
        assert!(
            props.contains_key("thinking_budget"),
            "must include thinking_budget"
        );
        assert!(props.contains_key("top_k"), "must include top_k");
    }

    #[test]
    fn per_model_schema_differs() {
        let opus = profile("claude-opus-4-6");
        let sonnet = profile("claude-sonnet-4-5");
        assert!(opus.is_some());
        assert!(sonnet.is_some());
        // Opus 4.6 schema should be different from sonnet schema
        assert_ne!(
            opus.as_ref().map(|p| &p.params_schema),
            sonnet.as_ref().map(|p| &p.params_schema),
            "opus 4.6 and sonnet must have different param schemas"
        );
    }

    #[test]
    fn non_claude_not_detected() {
        assert_eq!(detect_family("gpt-5.2"), None);
        assert!(!supports_thinking("gpt-5.2"));
    }

    #[test]
    fn schemas_are_valid_objects() {
        assert!(opus46_params_schema().is_object());
        assert!(standard_params_schema().is_object());
    }
}
