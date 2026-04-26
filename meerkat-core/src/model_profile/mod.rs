//! Model profile — projects a [`crate::capabilities::ModelCapabilities`] row
//! into the narrower [`ModelProfile`] surface consumed by the rest of the
//! platform.
//!
//! Before the per-model capability refactor, this module owned the full
//! capability definitions via hand-written struct-per-bucket types. That
//! lived in `anthropic.rs`, `openai.rs`, `gemini.rs` — each providing a
//! `profile(model)` function, a fixed set of JSON Schema buckets, and
//! heuristic helpers (`supports_adaptive_thinking`, `is_gpt5_family`, …).
//!
//! After the refactor, capability data lives in
//! [`crate::capabilities`] as a per-model table, and the JSON Schema is
//! derived from it by [`schema_builder::build_params_schema`]. The
//! per-provider modules now hold only request-shaping helpers that read the
//! same catalog. Uncatalogued model IDs do not receive synthesized semantic
//! capabilities.

pub mod anthropic;
pub mod capabilities;
pub mod catalog;
pub mod gemini;
pub mod openai;
pub mod schema_builder;

use crate::model_profile::capabilities::{
    BetaHeader, ModelCapabilities, ThinkingSupport, capabilities_for,
};
use serde::{Deserialize, Serialize};

/// Runtime profile for a model, describing its capabilities and operational defaults.
///
/// This is a **capability-plus-operational-defaults catalog**: it owns both model
/// capability flags (vision, thinking, temperature) and authoritative model-specific
/// operational defaults (call timeout) that the factory composes into effective
/// runtime policy. This ownership expansion is deliberate — see dogma rule §11.
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
    /// Whether the model accepts inline video content in user messages.
    pub inline_video: bool,
    /// Whether the model accepts image content in user messages.
    pub vision: bool,
    /// Whether the model can process image blocks in tool results.
    /// When false, `view_image` is hidden from the tool list.
    pub image_tool_results: bool,
    /// Whether the model supports a realtime bidirectional streaming transport
    /// (e.g. OpenAI `*-realtime*` endpoints, Gemini `*-live*` endpoints). Drives
    /// capability-based realtime transport attach/detach in the runtime.
    pub realtime: bool,
    /// Whether the model supports provider-native web search tools.
    pub supports_web_search: bool,
    /// JSON Schema describing accepted provider-specific parameters.
    pub params_schema: serde_json::Value,
    /// Beta headers authorized by the model capability catalog.
    #[serde(default)]
    pub beta_headers: Vec<ModelBetaHeader>,
    /// Authoritative default call timeout in seconds for this model family.
    ///
    /// `None` means the model family has no profiled default timeout.
    /// This is the canonical source for model-specific call timeout defaults,
    /// consumed by the factory/agent-loop resolver trait at call time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_timeout_secs: Option<u64>,
}

/// Catalog-owned beta header metadata for a model.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ModelBetaHeader {
    pub feature: String,
    pub header_name: String,
    pub header_value: String,
}

impl From<&BetaHeader> for ModelBetaHeader {
    fn from(value: &BetaHeader) -> Self {
        Self {
            feature: value.feature.to_string(),
            header_name: value.header_name.to_string(),
            header_value: value.header_value.to_string(),
        }
    }
}

/// Look up the profile for a model by provider string and model ID.
///
/// Catalog models project directly from their capability row. Uncatalogued
/// model IDs return `None`; semantic capability facts must come from the
/// capability catalog, not model-name prefixes.
///
/// Returns `None` if the provider is unknown or the model doesn't match any
/// recognized family.
pub fn profile_for(provider: &str, model: &str) -> Option<ModelProfile> {
    capabilities_for(provider, model).map(project_to_profile)
}

/// Project a capability record into the [`ModelProfile`] surface.
pub(crate) fn project_to_profile(caps: &ModelCapabilities) -> ModelProfile {
    ModelProfile {
        provider: caps.provider.to_string(),
        model_family: caps.model_family.to_string(),
        supports_temperature: caps.supports_temperature,
        supports_thinking: caps.thinking != ThinkingSupport::None,
        supports_reasoning: caps.supports_reasoning,
        supports_web_search: caps.supports_web_search,
        inline_video: caps.inline_video,
        vision: caps.vision,
        image_tool_results: caps.image_tool_results,
        realtime: caps.realtime,
        params_schema: schema_builder::build_params_schema(caps),
        beta_headers: caps
            .beta_headers
            .iter()
            .map(ModelBetaHeader::from)
            .collect(),
        call_timeout_secs: caps.call_timeout_secs,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn profile_for_all_catalog_models() {
        for entry in crate::model_profile::catalog::catalog() {
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
    fn uncatalogued_model_returns_none_for_known_provider() {
        assert!(profile_for("openai", "gpt-5.9-future").is_none());
        assert!(profile_for("anthropic", "claude-opus-4-7-20260501-preview").is_none());
        assert!(profile_for("gemini", "gemini-4-future").is_none());
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
        assert!(
            !profile.inline_video,
            "Anthropic models must NOT support inline video"
        );

        let profile = profile_for("anthropic", "claude-sonnet-4-5")
            .expect("claude-sonnet-4-5 must have a profile");
        assert!(profile.vision);
        assert!(profile.image_tool_results);
    }

    #[test]
    fn gpt_profile_vision_true_image_tool_results_false() {
        let profile = profile_for("openai", "gpt-5.4").expect("gpt-5.4 must have a profile");
        assert!(profile.vision, "OpenAI models must support vision");
        assert!(
            !profile.image_tool_results,
            "OpenAI models must NOT support image tool results"
        );
        assert!(
            !profile.inline_video,
            "OpenAI models must NOT support inline video"
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
        assert!(
            profile.inline_video,
            "Gemini models must support inline video"
        );
    }

    #[test]
    fn params_schema_non_empty_for_all_profiles() {
        for entry in crate::model_profile::catalog::catalog() {
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

    #[test]
    fn call_timeout_secs_populated_for_known_models() {
        for entry in crate::model_profile::catalog::catalog() {
            let profile = profile_for(entry.provider, entry.id);
            if let Some(p) = profile {
                assert!(
                    p.call_timeout_secs.is_some(),
                    "catalog model '{}' (provider '{}', family '{}') must have call_timeout_secs",
                    entry.id,
                    entry.provider,
                    p.model_family
                );
            }
        }
    }

    #[test]
    fn anthropic_opus_has_longer_timeout_than_haiku() {
        let opus = profile_for("anthropic", "claude-opus-4-6").unwrap();
        let haiku = profile_for("anthropic", "claude-haiku-4-5-20251001").unwrap();
        assert!(
            opus.call_timeout_secs.unwrap() > haiku.call_timeout_secs.unwrap(),
            "Opus should have a longer default timeout than Haiku"
        );
    }

    #[test]
    fn openai_pro_has_longer_timeout_than_standard_gpt5() {
        let pro = profile_for("openai", "gpt-5.5-pro").unwrap();
        let standard = profile_for("openai", "gpt-5.5").unwrap();
        assert!(
            pro.call_timeout_secs.unwrap() > standard.call_timeout_secs.unwrap(),
            "gpt-5.5-pro ({}) should have a much longer timeout than gpt-5.5 ({})",
            pro.call_timeout_secs.unwrap(),
            standard.call_timeout_secs.unwrap(),
        );
    }

    #[test]
    fn gemini_flash_has_shorter_timeout_than_pro() {
        let flash = profile_for("gemini", "gemini-3.1-flash-lite-preview").unwrap();
        let pro = profile_for("gemini", "gemini-3.1-pro-preview").unwrap();
        assert!(
            flash.call_timeout_secs.unwrap() < pro.call_timeout_secs.unwrap(),
            "gemini flash ({}) should have shorter timeout than gemini pro ({})",
            flash.call_timeout_secs.unwrap(),
            pro.call_timeout_secs.unwrap(),
        );
    }

    #[test]
    fn unknown_provider_call_timeout_is_none() {
        assert!(profile_for("unknown", "model").is_none());
    }

    #[test]
    fn web_search_flag_populated_for_all_catalog_models() {
        for entry in crate::model_profile::catalog::catalog() {
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
    fn anthropic_supports_web_search() {
        let profile = profile_for("anthropic", "claude-opus-4-6").unwrap();
        assert!(profile.supports_web_search);
    }

    #[test]
    fn openai_supports_web_search() {
        let profile = profile_for("openai", "gpt-5.4").unwrap();
        assert!(profile.supports_web_search);
    }

    #[test]
    fn gemini_supports_web_search() {
        let profile = profile_for("gemini", "gemini-3-flash-preview").unwrap();
        assert!(profile.supports_web_search);
    }
}
