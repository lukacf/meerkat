//! Curated model catalog — a narrow projection over [`crate::capabilities`].
//!
//! The catalog surfaces the small subset of data that is stable across the
//! platform: id, provider, display name, tier, context window, and max output
//! tokens. Richer capability data (effort levels, thinking modes, beta
//! headers) lives in [`crate::capabilities`] and is not re-exposed here.
//!
//! `meerkat-core` reads defaults from this module; `config_template.toml` is
//! validated against it.

use crate::Provider;
use crate::model_profile::capabilities;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// Model recommendation tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelTier {
    /// Tested and recommended for production use with Meerkat.
    Recommended,
    /// Supported but not the primary recommendation.
    Supported,
}

/// A curated model entry in the catalog.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CatalogEntry {
    /// Model identifier (e.g., `"claude-sonnet-4-6"`).
    pub id: &'static str,
    /// Human-readable display name (e.g., `"Claude Sonnet 4.6"`).
    pub display_name: &'static str,
    /// Canonical provider string (`"anthropic"`, `"openai"`, or `"gemini"`).
    pub provider: &'static str,
    /// Recommendation tier.
    pub tier: ModelTier,
    /// Maximum input context window in tokens, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    /// Maximum output tokens per response, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

/// Provider-level grouping with a default model.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProviderDefaults {
    /// Canonical provider string.
    pub provider: &'static str,
    /// The default model ID for this provider.
    pub default_model_id: &'static str,
    /// All catalog models for this provider.
    pub models: Vec<CatalogEntry>,
}

/// Catalog-owned OpenAI Images API request shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiImageGenerationRequestShape {
    GptImage,
    DallE,
}

/// Whether a native image-generation model accepts an explicit image-size field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImageGenerationSizeParameter {
    Supported,
    Unsupported,
}

/// Catalog-owned execution route for an image-generation model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "route", rename_all = "snake_case")]
pub enum ImageGenerationModelRoute {
    /// OpenAI Responses-hosted `image_generation` tool route.
    OpenAiHostedResponsesTool {
        /// `None` means use the requested model as the provider-call model.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_call_model_id: Option<&'static str>,
        /// Tool model passed inside the hosted Responses image tool.
        tool_model_id: &'static str,
    },
    /// OpenAI Images API route.
    OpenAiImagesApi {
        request_shape: OpenAiImageGenerationRequestShape,
    },
    /// Gemini native image model route.
    GeminiNativeModel {
        image_size_parameter: ImageGenerationSizeParameter,
    },
}

/// Shared image-generation catalog entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ImageGenerationModelProfile {
    /// Typed provider owner.
    pub provider: Provider,
    /// Model identifier.
    pub model_id: &'static str,
    /// Human-readable display name.
    pub display_name: &'static str,
    /// Recommendation tier.
    pub tier: ModelTier,
    /// Catalog-owned execution route.
    pub route: ImageGenerationModelRoute,
}

/// Provider-level grouping with a default image-generation model.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ImageGenerationProviderDefaults {
    /// Typed provider owner.
    pub provider: Provider,
    /// Default image-generation model ID for this provider.
    pub default_model_id: &'static str,
    /// All catalog-supported image-generation models for this provider.
    pub models: Vec<ImageGenerationModelProfile>,
}

// ---------------------------------------------------------------------------
// Static catalog data
// ---------------------------------------------------------------------------

/// Typed catalog providers in alphabetical order. The typed selection owner;
/// [`provider_names`] is its display projection.
const CATALOG_PROVIDERS: &[Provider] = &[Provider::Anthropic, Provider::Gemini, Provider::OpenAI];

/// Canonical provider names in alphabetical order (display projection of
/// [`CATALOG_PROVIDERS`]).
const PROVIDER_NAMES: &[&str] = &["anthropic", "gemini", "openai"];

/// Default model ID per provider. First recommended model wins.
const DEFAULT_ANTHROPIC: &str = "claude-opus-4-8";
const DEFAULT_OPENAI: &str = "gpt-5.5";
const DEFAULT_GEMINI: &str = "gemini-3.5-flash";

/// Image-generation default model ID per provider.
const IMAGE_DEFAULT_OPENAI: &str = "gpt-image-2";
const IMAGE_DEFAULT_GEMINI: &str = "gemini-3.1-flash-image-preview";

/// OpenAI hosted image tool host model. This route detail is deliberately
/// catalog-owned so provider executors do not infer policy from model names.
const OPENAI_HOSTED_IMAGE_PROVIDER_CALL_MODEL: &str = "gpt-5.4";

/// Image-generation-only model rows. OpenAI text catalog models are admitted by
/// `image_generation_model()` below and route through the same hosted tool.
const IMAGE_GENERATION_MODELS: &[ImageGenerationModelProfile] = &[
    ImageGenerationModelProfile {
        provider: Provider::OpenAI,
        model_id: IMAGE_DEFAULT_OPENAI,
        display_name: "GPT Image 2",
        tier: ModelTier::Recommended,
        route: ImageGenerationModelRoute::OpenAiHostedResponsesTool {
            provider_call_model_id: Some(OPENAI_HOSTED_IMAGE_PROVIDER_CALL_MODEL),
            tool_model_id: IMAGE_DEFAULT_OPENAI,
        },
    },
    ImageGenerationModelProfile {
        provider: Provider::OpenAI,
        model_id: "gpt-image-1",
        display_name: "GPT Image 1",
        tier: ModelTier::Supported,
        route: ImageGenerationModelRoute::OpenAiImagesApi {
            request_shape: OpenAiImageGenerationRequestShape::GptImage,
        },
    },
    ImageGenerationModelProfile {
        provider: Provider::OpenAI,
        model_id: "dall-e-3",
        display_name: "DALL-E 3",
        tier: ModelTier::Supported,
        route: ImageGenerationModelRoute::OpenAiImagesApi {
            request_shape: OpenAiImageGenerationRequestShape::DallE,
        },
    },
    ImageGenerationModelProfile {
        provider: Provider::OpenAI,
        model_id: "dall-e-2",
        display_name: "DALL-E 2",
        tier: ModelTier::Supported,
        route: ImageGenerationModelRoute::OpenAiImagesApi {
            request_shape: OpenAiImageGenerationRequestShape::DallE,
        },
    },
    ImageGenerationModelProfile {
        provider: Provider::Gemini,
        model_id: IMAGE_DEFAULT_GEMINI,
        display_name: "Gemini 3.1 Flash Image Preview",
        tier: ModelTier::Recommended,
        route: ImageGenerationModelRoute::GeminiNativeModel {
            image_size_parameter: ImageGenerationSizeParameter::Supported,
        },
    },
    ImageGenerationModelProfile {
        provider: Provider::Gemini,
        model_id: "gemini-3-pro-image-preview",
        display_name: "Gemini 3 Pro Image Preview",
        tier: ModelTier::Supported,
        route: ImageGenerationModelRoute::GeminiNativeModel {
            image_size_parameter: ImageGenerationSizeParameter::Supported,
        },
    },
    ImageGenerationModelProfile {
        provider: Provider::Gemini,
        model_id: "gemini-2.5-flash-image",
        display_name: "Gemini 2.5 Flash Image",
        tier: ModelTier::Supported,
        route: ImageGenerationModelRoute::GeminiNativeModel {
            image_size_parameter: ImageGenerationSizeParameter::Unsupported,
        },
    },
];

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Return all catalog entries, derived from [`crate::capabilities`].
pub fn catalog() -> &'static [CatalogEntry] {
    static CATALOG_DATA: OnceLock<Vec<CatalogEntry>> = OnceLock::new();
    CATALOG_DATA.get_or_init(|| {
        capabilities::all_capabilities()
            .map(|c| CatalogEntry {
                id: c.id,
                display_name: c.display_name,
                provider: c.provider.as_str(),
                tier: c.tier,
                context_window: Some(c.context_window),
                max_output_tokens: Some(c.max_output_tokens),
            })
            .collect()
    })
}

/// Typed catalog providers (the typed selection owner).
pub fn catalog_providers() -> &'static [Provider] {
    CATALOG_PROVIDERS
}

/// Return canonical provider names (display projection of
/// [`catalog_providers`]; not a selection key — semantic lookups go through
/// the typed [`Provider`] boundary).
pub fn provider_names() -> &'static [&'static str] {
    PROVIDER_NAMES
}

/// Return the default model ID for a typed provider, or `None` if the
/// provider has no catalog defaults.
pub fn default_model(provider: Provider) -> Option<&'static str> {
    match provider {
        Provider::Anthropic => Some(DEFAULT_ANTHROPIC),
        Provider::OpenAI => Some(DEFAULT_OPENAI),
        Provider::Gemini => Some(DEFAULT_GEMINI),
        Provider::SelfHosted | Provider::Other => None,
    }
}

/// Return the default image-generation model profile for a typed provider.
pub fn default_image_generation_model(provider: Provider) -> Option<ImageGenerationModelProfile> {
    let default = match provider {
        Provider::OpenAI => IMAGE_DEFAULT_OPENAI,
        Provider::Gemini => IMAGE_DEFAULT_GEMINI,
        _ => return None,
    };
    image_generation_model(provider, default)
}

/// Return a catalog-owned image-generation model profile for a typed provider/model pair.
///
/// Returns `None` for unknown providers, unknown model IDs, and provider/model
/// mismatches. OpenAI text catalog models are supported through the hosted
/// Responses image tool; other providers must have explicit image model rows.
pub fn image_generation_model(
    provider: Provider,
    model_id: &str,
) -> Option<ImageGenerationModelProfile> {
    if let Some(profile) = IMAGE_GENERATION_MODELS
        .iter()
        .copied()
        .find(|profile| profile.provider == provider && profile.model_id == model_id)
    {
        return Some(profile);
    }

    match provider {
        Provider::OpenAI => {
            capabilities::capabilities_for(Provider::OpenAI, model_id).map(|caps| {
                ImageGenerationModelProfile {
                    provider,
                    model_id: caps.id,
                    display_name: caps.display_name,
                    tier: caps.tier,
                    route: ImageGenerationModelRoute::OpenAiHostedResponsesTool {
                        provider_call_model_id: None,
                        tool_model_id: IMAGE_DEFAULT_OPENAI,
                    },
                }
            })
        }
        _ => None,
    }
}

/// Infer a typed image-generation provider from the catalog.
///
/// This is intentionally catalog-only: unknown image-like names fail closed
/// instead of being accepted by prefix folklore.
pub fn image_generation_provider_for_model(model_id: &str) -> Option<Provider> {
    IMAGE_GENERATION_MODELS
        .iter()
        .find(|profile| profile.model_id == model_id)
        .map(|profile| profile.provider)
        .or_else(|| {
            capabilities::capabilities_for(Provider::OpenAI, model_id).map(|_| Provider::OpenAI)
        })
}

/// Return an iterator over model IDs in the catalog for a typed provider.
pub fn allowed_models(provider: Provider) -> impl Iterator<Item = &'static str> + 'static {
    catalog()
        .iter()
        .filter(move |e| e.provider == provider.as_str())
        .map(|e| e.id)
}

/// Look up a specific catalog entry by typed provider and model ID.
pub fn entry_for(provider: Provider, model_id: &str) -> Option<&'static CatalogEntry> {
    catalog()
        .iter()
        .find(|e| e.provider == provider.as_str() && e.id == model_id)
}

/// Return provider-grouped catalog data with default model IDs.
///
/// Built once via `OnceLock`, returned as a `&'static` slice.
pub fn provider_defaults() -> &'static [ProviderDefaults] {
    static DEFAULTS: OnceLock<Vec<ProviderDefaults>> = OnceLock::new();
    DEFAULTS.get_or_init(|| {
        CATALOG_PROVIDERS
            .iter()
            .filter_map(|&provider| {
                let default_id = default_model(provider)?;
                let models: Vec<CatalogEntry> = catalog()
                    .iter()
                    .filter(|e| e.provider == provider.as_str())
                    .cloned()
                    .collect();
                Some(ProviderDefaults {
                    provider: provider.as_str(),
                    default_model_id: default_id,
                    models,
                })
            })
            .collect()
    })
}

/// Return provider-grouped image-generation catalog data with default model IDs.
pub fn image_generation_provider_defaults() -> &'static [ImageGenerationProviderDefaults] {
    static DEFAULTS: OnceLock<Vec<ImageGenerationProviderDefaults>> = OnceLock::new();
    DEFAULTS.get_or_init(|| {
        [Provider::Gemini, Provider::OpenAI]
            .into_iter()
            .filter_map(|provider| {
                let default_model = default_image_generation_model(provider)?;
                let mut models: Vec<ImageGenerationModelProfile> = IMAGE_GENERATION_MODELS
                    .iter()
                    .copied()
                    .filter(|profile| profile.provider == provider)
                    .collect();
                if provider == Provider::OpenAI {
                    models.extend(
                        catalog()
                            .iter()
                            .filter(|entry| entry.provider == Provider::OpenAI.as_str())
                            .filter_map(|entry| image_generation_model(provider, entry.id)),
                    );
                }
                Some(ImageGenerationProviderDefaults {
                    provider,
                    default_model_id: default_model.model_id,
                    models,
                })
            })
            .collect()
    })
}

/// The platform-wide default model when no provider/model is otherwise
/// specified. The single global default owned by the catalog seam — surfaces
/// must read this instead of hardcoding a model literal.
pub fn global_default_model() -> &'static str {
    DEFAULT_ANTHROPIC
}

/// Canonical cross-provider priority order, owned by the catalog seam. A
/// surface that must pick "the best available provider" (e.g. when several
/// API keys are present) consults this order instead of hand-coding its own.
pub fn provider_priority() -> &'static [crate::Provider] {
    const PRIORITY: &[crate::Provider] = &[
        crate::Provider::Anthropic,
        crate::Provider::OpenAI,
        crate::Provider::Gemini,
    ];
    PRIORITY
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn exactly_one_default_per_provider() {
        for &provider in CATALOG_PROVIDERS {
            let result = default_model(provider);
            assert!(
                result.is_some(),
                "provider '{provider:?}' must have a default model"
            );
        }
    }

    #[test]
    fn provider_names_is_projection_of_typed_catalog_providers() {
        let projected: Vec<&str> = CATALOG_PROVIDERS.iter().map(|p| p.as_str()).collect();
        assert_eq!(
            provider_names(),
            projected.as_slice(),
            "provider_names() must be a divergence-free projection of the typed providers"
        );
    }

    #[test]
    fn defaults_exist_in_catalog() {
        for &provider in CATALOG_PROVIDERS {
            let default = default_model(provider);
            assert!(
                default.is_some(),
                "provider '{provider:?}' must have a default model"
            );
            if let Some(default) = default {
                let entry = entry_for(provider, default);
                assert!(
                    entry.is_some(),
                    "default model '{default}' for provider '{provider:?}' must exist in catalog"
                );
            }
        }
    }

    #[test]
    fn untyped_providers_fail_closed_at_typed_selection_boundary() {
        assert!(default_model(Provider::Other).is_none());
        assert!(default_model(Provider::SelfHosted).is_none());
        assert!(entry_for(Provider::Other, DEFAULT_ANTHROPIC).is_none());
        assert_eq!(allowed_models(Provider::Other).count(), 0);
    }

    #[test]
    fn all_provider_strings_canonical() {
        let canonical: HashSet<&str> = PROVIDER_NAMES.iter().copied().collect();
        for entry in catalog() {
            assert!(
                canonical.contains(entry.provider),
                "catalog entry '{}' has non-canonical provider '{}'",
                entry.id,
                entry.provider
            );
        }
    }

    #[test]
    fn no_duplicate_model_ids_within_provider() {
        for &provider in PROVIDER_NAMES {
            let ids: Vec<&str> = catalog()
                .iter()
                .filter(|e| e.provider == provider)
                .map(|e| e.id)
                .collect();
            let unique: HashSet<&str> = ids.iter().copied().collect();
            assert_eq!(
                ids.len(),
                unique.len(),
                "provider '{provider}' has duplicate model IDs"
            );
        }
    }

    #[test]
    fn provider_defaults_complete() {
        let defaults = provider_defaults();
        assert_eq!(
            defaults.len(),
            PROVIDER_NAMES.len(),
            "provider_defaults() must cover all providers"
        );
        for pd in defaults {
            assert!(
                !pd.models.is_empty(),
                "provider '{}' must have at least one model",
                pd.provider
            );
            let has_default = pd.models.iter().any(|m| m.id == pd.default_model_id);
            assert!(
                has_default,
                "default model '{}' for provider '{}' must be in the models list",
                pd.default_model_id, pd.provider
            );
        }
    }

    #[test]
    fn allowed_models_matches_catalog() {
        for &provider in CATALOG_PROVIDERS {
            let allowed: Vec<&str> = allowed_models(provider).collect();
            let from_catalog: Vec<&str> = catalog()
                .iter()
                .filter(|e| e.provider == provider.as_str())
                .map(|e| e.id)
                .collect();
            assert_eq!(
                allowed, from_catalog,
                "allowed_models({provider:?}) must match catalog entries"
            );
        }
    }

    #[test]
    fn catalog_matches_capability_table() {
        for entry in catalog() {
            let provider = crate::Provider::parse_strict(entry.provider)
                .unwrap_or_else(|| panic!("catalog provider '{}' must parse", entry.provider));
            let caps = capabilities::capabilities_for(provider, entry.id)
                .expect("catalog entry must have a capability row");
            assert_eq!(entry.id, caps.id);
            assert_eq!(entry.provider, caps.provider.as_str());
            assert_eq!(entry.display_name, caps.display_name);
            assert_eq!(entry.tier, caps.tier);
            assert_eq!(entry.context_window, Some(caps.context_window));
            assert_eq!(entry.max_output_tokens, Some(caps.max_output_tokens));
        }
    }

    #[test]
    fn image_generation_defaults_are_catalog_owned_and_typed() {
        let openai = default_image_generation_model(Provider::OpenAI)
            .expect("OpenAI must have an image-generation default");
        assert_eq!(openai.model_id, IMAGE_DEFAULT_OPENAI);
        assert_eq!(openai.provider, Provider::OpenAI);
        assert!(matches!(
            openai.route,
            ImageGenerationModelRoute::OpenAiHostedResponsesTool { .. }
        ));

        let gemini = default_image_generation_model(Provider::Gemini)
            .expect("Gemini must have an image-generation default");
        assert_eq!(gemini.model_id, IMAGE_DEFAULT_GEMINI);
        assert_eq!(gemini.provider, Provider::Gemini);
        assert!(matches!(
            gemini.route,
            ImageGenerationModelRoute::GeminiNativeModel { .. }
        ));

        assert!(default_image_generation_model(Provider::Anthropic).is_none());
        assert!(default_image_generation_model(Provider::Other).is_none());
    }

    #[test]
    fn image_generation_lookup_fails_closed_for_unknown_or_mismatched_models() {
        assert!(image_generation_model(Provider::Gemini, "gemini-unknown-image-preview").is_none());
        assert!(image_generation_model(Provider::OpenAI, "gpt-image-unknown").is_none());
        assert!(image_generation_model(Provider::Gemini, IMAGE_DEFAULT_OPENAI).is_none());
        assert!(image_generation_model(Provider::OpenAI, IMAGE_DEFAULT_GEMINI).is_none());
        assert!(image_generation_model(Provider::Other, IMAGE_DEFAULT_OPENAI).is_none());
    }

    #[test]
    fn image_generation_provider_inference_is_catalog_only() {
        assert_eq!(
            image_generation_provider_for_model(IMAGE_DEFAULT_OPENAI),
            Some(Provider::OpenAI)
        );
        assert_eq!(
            image_generation_provider_for_model(IMAGE_DEFAULT_GEMINI),
            Some(Provider::Gemini)
        );
        assert_eq!(
            image_generation_provider_for_model("gpt-5.4"),
            Some(Provider::OpenAI)
        );
        assert_eq!(
            image_generation_provider_for_model("gpt-image-future"),
            None
        );
        assert_eq!(
            image_generation_provider_for_model("gemini-3.5-flash"),
            None,
            "Gemini text catalog rows are not image-generation model authority"
        );
    }

    #[test]
    fn global_default_and_provider_priority_are_catalog_owned() {
        assert_eq!(global_default_model(), DEFAULT_ANTHROPIC);

        let priority = provider_priority();
        assert_eq!(
            priority.len(),
            3,
            "provider priority must cover three providers"
        );
        assert_eq!(
            priority.first(),
            Some(&Provider::Anthropic),
            "Anthropic must be the highest-priority provider"
        );
        assert!(
            priority.contains(&Provider::OpenAI),
            "provider priority must contain OpenAI"
        );
        assert!(
            priority.contains(&Provider::Gemini),
            "provider priority must contain Gemini"
        );
    }

    #[test]
    fn image_generation_provider_defaults_are_complete() {
        let defaults = image_generation_provider_defaults();
        assert_eq!(defaults.len(), 2);
        for defaults in defaults {
            let default_profile = default_image_generation_model(defaults.provider)
                .expect("provider default must resolve");
            assert_eq!(defaults.default_model_id, default_profile.model_id);
            assert!(
                defaults
                    .models
                    .iter()
                    .any(|model| model.model_id == defaults.default_model_id),
                "image-generation default must be present in provider models"
            );
        }
    }
}
