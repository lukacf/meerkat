//! Model profile rules — capability detection and parameter schema generation.
//!
//! Each provider submodule owns family detection logic and parameter schemas.
//! The adapter crates (`meerkat-client`) delegate to these functions so that
//! runtime behavior and the public catalog agree on model capabilities.

pub mod anthropic;
pub mod gemini;
pub mod openai;

use serde::{Deserialize, Serialize};

/// Runtime profile for a model, describing its capabilities and accepted parameters.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ModelProfile {
    /// Canonical provider string.
    pub provider: String,
    /// Model family identifier (e.g., `"claude-opus-4"`, `"gpt-5"`, `"gemini-3"`).
    pub model_family: String,
    /// Whether the model accepts a `temperature` parameter.
    pub supports_temperature: bool,
    /// Whether the model supports extended thinking / reasoning budgets.
    pub supports_thinking: bool,
    /// Whether the model supports explicit reasoning effort control.
    pub supports_reasoning: bool,
    /// Whether the model accepts image content in user messages.
    pub vision: bool,
    /// Whether the model can process image blocks in tool results.
    /// When false, `view_image` is hidden from the tool list.
    pub image_tool_results: bool,
    /// JSON Schema describing accepted provider-specific parameters.
    pub params_schema: serde_json::Value,
}

/// Look up the profile for a model by provider string and model ID.
///
/// Returns `None` if the provider is unknown or the model doesn't match
/// any recognized family.
pub fn profile_for(provider: &str, model: &str) -> Option<ModelProfile> {
    match provider {
        "anthropic" => anthropic::profile(model),
        "openai" => openai::profile(model),
        "gemini" => gemini::profile(model),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_for_all_catalog_models() {
        for entry in crate::catalog::catalog() {
            let profile = profile_for(entry.provider, entry.id);
            assert!(
                profile.is_some(),
                "catalog model '{}' (provider '{}') must have a profile",
                entry.id,
                entry.provider
            );
        }
    }

    #[test]
    fn unknown_provider_returns_none() {
        assert!(profile_for("unknown", "some-model").is_none());
    }

    #[test]
    fn claude_profile_vision_and_image_tool_results_true() {
        let profile = profile_for("anthropic", "claude-opus-4-6")
            .expect("claude-opus-4-6 must have a profile");
        assert!(profile.vision, "Anthropic models must support vision");
        assert!(
            profile.image_tool_results,
            "Anthropic models must support image tool results"
        );

        let profile = profile_for("anthropic", "claude-sonnet-4-5")
            .expect("claude-sonnet-4-5 must have a profile");
        assert!(profile.vision);
        assert!(profile.image_tool_results);
    }

    #[test]
    fn gpt_profile_vision_true_image_tool_results_false() {
        let profile = profile_for("openai", "gpt-5.2").expect("gpt-5.2 must have a profile");
        assert!(profile.vision, "OpenAI models must support vision");
        assert!(
            !profile.image_tool_results,
            "OpenAI models must NOT support image tool results"
        );
    }

    #[test]
    fn gemini_profile_vision_and_image_tool_results_true() {
        let profile = profile_for("gemini", "gemini-3-flash-preview")
            .expect("gemini-3-flash-preview must have a profile");
        assert!(profile.vision, "Gemini models must support vision");
        assert!(
            profile.image_tool_results,
            "Gemini models must support image tool results"
        );
    }

    #[test]
    fn params_schema_non_empty_for_all_profiles() {
        for entry in crate::catalog::catalog() {
            let profile = profile_for(entry.provider, entry.id);
            if let Some(p) = profile {
                assert!(
                    p.params_schema.is_object(),
                    "params_schema for '{}' must be a JSON object, got {:?}",
                    entry.id,
                    p.params_schema
                );
            }
        }
    }
}
