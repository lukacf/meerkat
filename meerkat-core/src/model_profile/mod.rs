//! Model-profile vocabulary and catalog mechanics.
//!
//! `meerkat-core` owns the *shape* of the model catalog: the vocabulary types
//! ([`ModelProfile`], [`crate::model_profile::capabilities::ModelCapabilities`],
//! [`crate::model_profile::catalog::CatalogEntry`], …), the pure projection
//! from capability rows to profiles, and the provider-neutral lookup
//! mechanics ([`ModelCatalog`]).
//!
//! The *data* — which models exist, their capability rows, defaults, and
//! priorities — deliberately lives outside core. The `meerkat-models` crate
//! owns the canonical instance and hands it to core consumers as an explicit
//! [`ModelCatalog`] parameter. Core must never embed provider data: no static
//! model rows, no model-name literals, no registration points.

pub mod capabilities;
pub mod catalog;
pub mod schema_builder;

use crate::Provider;
use crate::model_profile::capabilities::{BetaHeader, ModelCapabilities, ThinkingSupport};
use crate::model_profile::catalog::{
    CatalogEntry, ImageGenerationModelProfile, ImageGenerationModelRoute,
    ImageGenerationProviderDefaults, ProviderDefaults,
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
    /// Provider that serves this model.
    pub provider: Provider,
    /// Model family identifier (a stable grouping key for related model ids).
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
    /// Whether user messages may include image input blocks.
    pub image_input: bool,
    /// Whether the model can process image blocks in tool results.
    /// When false, `view_image` is hidden from the tool list.
    pub image_tool_results: bool,
    /// Whether the model supports a realtime bidirectional streaming transport.
    /// Drives capability-based realtime transport attach/detach in the runtime.
    pub realtime: bool,
    /// Whether the model supports provider-native web search tools.
    pub supports_web_search: bool,
    /// Whether the provider/model can use Meerkat image generation.
    pub image_generation: bool,
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
            // Wire/display projection of the typed `BetaFeature` owner.
            feature: value.feature.as_wire_str().to_string(),
            header_name: value.header_name.to_string(),
            header_value: value.header_value.to_string(),
        }
    }
}

/// Project a capability record into the [`ModelProfile`] surface.
///
/// Pure mechanics over the vocabulary types: no data lookup is involved.
pub fn project_to_profile(caps: &ModelCapabilities) -> ModelProfile {
    ModelProfile {
        provider: caps.provider,
        model_family: caps.model_family.to_string(),
        supports_temperature: caps.supports_temperature,
        supports_thinking: caps.thinking != ThinkingSupport::None,
        supports_reasoning: caps.supports_reasoning,
        supports_web_search: caps.supports_web_search,
        inline_video: caps.inline_video,
        vision: caps.vision,
        image_input: caps.vision,
        image_tool_results: caps.image_tool_results,
        realtime: caps.realtime,
        image_generation: caps.image_generation,
        params_schema: schema_builder::build_params_schema(caps),
        beta_headers: caps
            .beta_headers
            .iter()
            .map(ModelBetaHeader::from)
            .collect(),
        call_timeout_secs: caps.call_timeout_secs,
    }
}

/// Injected provider-model catalog. meerkat-core defines the shape and
/// mechanics; the data lives outside core (`meerkat-models` provides the
/// canonical instance). Core must never embed provider data.
#[derive(Debug, Clone, Copy)]
pub struct ModelCatalog {
    /// Curated catalog entries (projection of the capability rows).
    pub entries: &'static [CatalogEntry],
    /// Full per-model capability rows across all providers.
    pub capabilities: &'static [ModelCapabilities],
    /// Provider-grouped catalog data with default model IDs.
    pub provider_defaults: &'static [ProviderDefaults],
    /// Image-generation-only model rows.
    pub image_generation_models: &'static [ImageGenerationModelProfile],
    /// Typed catalog providers (the typed selection owner).
    pub providers: &'static [Provider],
    /// Default model ID per provider.
    pub default_models: &'static [(Provider, &'static str)],
    /// Default image-generation model ID per provider.
    pub image_generation_defaults: &'static [(Provider, &'static str)],
    /// The platform-wide default model when no provider/model is otherwise
    /// specified.
    pub global_default_model: &'static str,
    /// Canonical cross-provider priority order. A surface that must pick "the
    /// best available provider" consults this order instead of hand-coding
    /// its own.
    pub provider_priority: &'static [Provider],
}

impl ModelCatalog {
    /// Look up a specific catalog entry by typed provider and model ID.
    pub fn entry_for(self, provider: Provider, model_id: &str) -> Option<&'static CatalogEntry> {
        self.entries
            .iter()
            .find(|e| e.provider == provider.as_str() && e.id == model_id)
    }

    /// Return the default model ID for a typed provider, or `None` if the
    /// provider has no catalog defaults.
    pub fn default_model(self, provider: Provider) -> Option<&'static str> {
        self.default_models
            .iter()
            .find(|(p, _)| *p == provider)
            .map(|(_, id)| *id)
    }

    /// Return an iterator over model IDs in the catalog for a typed provider.
    pub fn allowed_models(
        self,
        provider: Provider,
    ) -> impl Iterator<Item = &'static str> + 'static {
        self.entries
            .iter()
            .filter(move |e| e.provider == provider.as_str())
            .map(|e| e.id)
    }

    /// Lookup a model's capabilities by typed provider + id.
    ///
    /// Returns `None` when the provider/model pair has no capability row.
    /// Callers must treat `None` as unknown capability truth; uncatalogued
    /// model IDs must not synthesize semantic facts from model-name folklore.
    pub fn capabilities_for(
        self,
        provider: Provider,
        model_id: &str,
    ) -> Option<&'static ModelCapabilities> {
        self.capabilities
            .iter()
            .find(|c| c.provider == provider && c.id == model_id)
    }

    /// Look up the profile for a model by typed provider and model ID.
    ///
    /// Catalog models project directly from their capability row. Uncatalogued
    /// model IDs return `None`; semantic capability facts must come from the
    /// capability catalog, not model-name prefixes.
    pub fn profile_for(self, provider: Provider, model: &str) -> Option<ModelProfile> {
        self.capabilities_for(provider, model)
            .map(project_to_profile)
    }

    /// Look up whether a model accepts inline video by typed provider and model ID.
    ///
    /// Returns `None` when the provider/model pair has no capability row.
    pub fn inline_video_support_for(self, provider: Provider, model: &str) -> Option<bool> {
        self.capabilities_for(provider, model)
            .map(|caps| caps.inline_video)
    }

    /// Infer a typed provider from the catalog by exact model ID match.
    ///
    /// Returns `None` for uncatalogued models; callers that admit custom
    /// models must resolve them through `ModelRegistry`, not name prefixes.
    pub fn infer_provider(self, model: &str) -> Option<Provider> {
        self.entries
            .iter()
            .find(|entry| entry.id == model)
            .and_then(|entry| Provider::parse_strict(entry.provider))
    }

    /// Return the default image-generation model ID for a typed provider.
    pub fn default_image_generation_model_id(self, provider: Provider) -> Option<&'static str> {
        self.image_generation_defaults
            .iter()
            .find(|(p, _)| *p == provider)
            .map(|(_, id)| *id)
    }

    /// Return the default image-generation model profile for a typed provider.
    pub fn default_image_generation_model(
        self,
        provider: Provider,
    ) -> Option<ImageGenerationModelProfile> {
        let default = self.default_image_generation_model_id(provider)?;
        self.image_generation_model(provider, default)
    }

    /// Return a catalog-owned image-generation model profile for a typed
    /// provider/model pair.
    ///
    /// Returns `None` for unknown providers, unknown model IDs, and
    /// provider/model mismatches. Text catalog models of a provider whose
    /// default image-generation route is the hosted Responses image tool are
    /// admitted through that hosted route; other providers must have explicit
    /// image model rows.
    pub fn image_generation_model(
        self,
        provider: Provider,
        model_id: &str,
    ) -> Option<ImageGenerationModelProfile> {
        if let Some(profile) = self
            .image_generation_models
            .iter()
            .copied()
            .find(|profile| profile.provider == provider && profile.model_id == model_id)
        {
            return Some(profile);
        }

        let tool_model_id = self.hosted_responses_tool_model_id(provider)?;
        self.capabilities_for(provider, model_id)
            .map(|caps| ImageGenerationModelProfile {
                provider,
                model_id: caps.id,
                display_name: caps.display_name,
                tier: caps.tier,
                route: ImageGenerationModelRoute::OpenAiHostedResponsesTool {
                    provider_call_model_id: None,
                    tool_model_id,
                },
            })
    }

    /// Infer a typed image-generation provider from the catalog.
    ///
    /// This is intentionally catalog-only: unknown image-like names fail
    /// closed instead of being accepted by prefix folklore.
    pub fn image_generation_provider_for_model(self, model_id: &str) -> Option<Provider> {
        self.image_generation_models
            .iter()
            .find(|profile| profile.model_id == model_id)
            .map(|profile| profile.provider)
            .or_else(|| {
                self.providers.iter().copied().find(|&provider| {
                    self.hosted_responses_tool_model_id(provider).is_some()
                        && self.capabilities_for(provider, model_id).is_some()
                })
            })
    }

    /// Compute provider-grouped image-generation catalog data with default
    /// model IDs.
    pub fn image_generation_provider_defaults(self) -> Vec<ImageGenerationProviderDefaults> {
        self.image_generation_defaults
            .iter()
            .filter_map(|&(provider, _)| {
                let default_model = self.default_image_generation_model(provider)?;
                let mut models: Vec<ImageGenerationModelProfile> = self
                    .image_generation_models
                    .iter()
                    .copied()
                    .filter(|profile| profile.provider == provider)
                    .collect();
                if self.hosted_responses_tool_model_id(provider).is_some() {
                    models.extend(
                        self.entries
                            .iter()
                            .filter(|entry| entry.provider == provider.as_str())
                            .filter_map(|entry| self.image_generation_model(provider, entry.id)),
                    );
                }
                Some(ImageGenerationProviderDefaults {
                    provider,
                    default_model_id: default_model.model_id,
                    models,
                })
            })
            .collect()
    }

    /// The hosted Responses image-tool model ID for a provider whose default
    /// image-generation route is the hosted tool, if any.
    fn hosted_responses_tool_model_id(self, provider: Provider) -> Option<&'static str> {
        let default_id = self.default_image_generation_model_id(provider)?;
        let default_row = self
            .image_generation_models
            .iter()
            .find(|profile| profile.provider == provider && profile.model_id == default_id)?;
        match default_row.route {
            ImageGenerationModelRoute::OpenAiHostedResponsesTool { tool_model_id, .. } => {
                Some(tool_model_id)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
pub(crate) mod test_catalog;

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::test_catalog::TEST_CATALOG;
    use super::*;

    /// Fields that `ModelProfile` must expose, mirrored by `WireModelProfile`
    /// in `meerkat-contracts::wire::models`. Adding a field here is a
    /// wire-contract change that needs parallel updates in `meerkat-contracts`
    /// and a schema regeneration.
    const EXPECTED_MODEL_PROFILE_FIELDS: &[&str] = &[
        "provider",
        "model_family",
        "supports_temperature",
        "supports_thinking",
        "supports_reasoning",
        "supports_web_search",
        "inline_video",
        "vision",
        "image_input",
        "image_tool_results",
        "realtime",
        "image_generation",
        "params_schema",
        "call_timeout_secs",
        "beta_headers",
    ];

    #[test]
    fn model_profile_wire_field_parity() {
        let schema = schemars::schema_for!(ModelProfile);
        let value = serde_json::to_value(&schema).expect("schema serializes to JSON");
        let props = value
            .get("properties")
            .and_then(|p| p.as_object())
            .expect("ModelProfile schema has a properties map")
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let expected: std::collections::BTreeSet<String> = EXPECTED_MODEL_PROFILE_FIELDS
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        assert_eq!(
            props, expected,
            "ModelProfile field set drift — update meerkat-contracts::WireModelProfile \
             in lockstep, regenerate artifact schemas (make regen-schemas), and update \
             EXPECTED_MODEL_PROFILE_FIELDS if this change is intentional."
        );
    }

    #[test]
    fn profile_for_projects_capability_rows() {
        for entry in TEST_CATALOG.entries {
            let provider = Provider::parse_strict(entry.provider)
                .unwrap_or_else(|| panic!("test catalog provider '{}' must parse", entry.provider));
            let profile = TEST_CATALOG.profile_for(provider, entry.id);
            assert!(
                profile.is_some(),
                "test catalog model '{}' must project a profile",
                entry.id
            );
        }
    }

    #[test]
    fn unknown_provider_or_model_fails_closed() {
        assert!(
            TEST_CATALOG
                .profile_for(Provider::Other, "some-model")
                .is_none()
        );
        assert!(
            TEST_CATALOG
                .profile_for(Provider::Anthropic, "uncatalogued-model")
                .is_none()
        );
        assert!(
            TEST_CATALOG
                .inline_video_support_for(Provider::Other, "some-model")
                .is_none()
        );
    }

    #[test]
    fn wrong_typed_provider_for_known_model_fails_closed() {
        // The test catalog registers fake-video-model under Gemini only.
        assert!(
            TEST_CATALOG
                .profile_for(Provider::Gemini, test_catalog::VIDEO_MODEL)
                .is_some()
        );
        assert!(
            TEST_CATALOG
                .profile_for(Provider::OpenAI, test_catalog::VIDEO_MODEL)
                .is_none()
        );
    }

    #[test]
    fn inline_video_support_reads_capability_truth() {
        assert_eq!(
            TEST_CATALOG.inline_video_support_for(Provider::Gemini, test_catalog::VIDEO_MODEL),
            Some(true)
        );
        assert_eq!(
            TEST_CATALOG.inline_video_support_for(Provider::OpenAI, test_catalog::OPENAI_MODEL),
            Some(false)
        );
    }

    #[test]
    fn infer_provider_is_exact_catalog_match_only() {
        assert_eq!(
            TEST_CATALOG.infer_provider(test_catalog::ANTHROPIC_MODEL),
            Some(Provider::Anthropic)
        );
        assert_eq!(TEST_CATALOG.infer_provider("uncatalogued-model"), None);
        assert_eq!(TEST_CATALOG.infer_provider(""), None);
    }

    #[test]
    fn default_models_resolve_through_typed_providers() {
        assert_eq!(
            TEST_CATALOG.default_model(Provider::Anthropic),
            Some(test_catalog::ANTHROPIC_MODEL)
        );
        assert!(TEST_CATALOG.default_model(Provider::Other).is_none());
        assert!(TEST_CATALOG.default_model(Provider::SelfHosted).is_none());
    }

    #[test]
    fn image_generation_lookup_is_catalog_owned_and_fails_closed() {
        let default = TEST_CATALOG
            .default_image_generation_model(Provider::Gemini)
            .expect("test catalog Gemini image default");
        assert_eq!(default.model_id, test_catalog::GEMINI_IMAGE_MODEL);
        assert!(
            TEST_CATALOG
                .default_image_generation_model(Provider::Anthropic)
                .is_none()
        );
        assert!(
            TEST_CATALOG
                .image_generation_model(Provider::Gemini, "unknown-image-model")
                .is_none()
        );
        assert_eq!(
            TEST_CATALOG.image_generation_provider_for_model(test_catalog::GEMINI_IMAGE_MODEL),
            Some(Provider::Gemini)
        );
        assert_eq!(
            TEST_CATALOG.image_generation_provider_for_model("unknown-image-model"),
            None
        );
    }

    #[test]
    fn hosted_tool_route_admits_text_models_of_hosted_provider_only() {
        // OpenAI's test image default routes through the hosted Responses
        // tool, so its text rows are admitted through the same route.
        let via_text = TEST_CATALOG
            .image_generation_model(Provider::OpenAI, test_catalog::OPENAI_MODEL)
            .expect("hosted-tool provider text model admitted");
        assert!(matches!(
            via_text.route,
            ImageGenerationModelRoute::OpenAiHostedResponsesTool {
                provider_call_model_id: None,
                ..
            }
        ));
        // Gemini's default route is the native model: its text rows are NOT
        // admitted as image-generation models.
        assert!(
            TEST_CATALOG
                .image_generation_model(Provider::Gemini, test_catalog::VIDEO_MODEL)
                .is_none()
        );
    }

    #[test]
    fn image_generation_provider_defaults_cover_declared_defaults() {
        let defaults = TEST_CATALOG.image_generation_provider_defaults();
        assert_eq!(defaults.len(), TEST_CATALOG.image_generation_defaults.len());
        for d in &defaults {
            assert!(
                d.models.iter().any(|m| m.model_id == d.default_model_id),
                "image-generation default must be present in provider models"
            );
        }
    }
}
