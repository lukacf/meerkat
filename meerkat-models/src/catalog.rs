//! Curated model catalog data — the canonical [`ModelCatalog`] instance.
//!
//! The catalog surfaces the small subset of data that is stable across the
//! platform: id, provider, display name, tier, context window, and max output
//! tokens. Richer capability data (effort levels, thinking modes, beta
//! headers) lives in [`crate::capabilities`] and is not re-exposed here.
//!
//! All free functions delegate to [`ModelCatalog`] methods over the
//! [`canonical`] instance, preserving the historical function API.

use crate::capabilities;
use meerkat_core::Provider;
use meerkat_core::model_profile::ModelCatalog;
use meerkat_core::model_profile::catalog::{
    CatalogEntry, ImageGenerationModelProfile, ImageGenerationModelRoute,
    ImageGenerationProviderDefaults, ImageGenerationSizeParameter, ModelTier,
    OpenAiImageGenerationRequestShape, ProviderDefaults,
};
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Static catalog data
// ---------------------------------------------------------------------------

/// Typed catalog providers in alphabetical order. The typed selection owner;
/// [`provider_names`] is its display projection.
const CATALOG_PROVIDERS: &[Provider] = &[Provider::Anthropic, Provider::Gemini, Provider::OpenAI];

/// Canonical provider names in alphabetical order (display projection of
/// [`CATALOG_PROVIDERS`]).
const PROVIDER_NAMES: &[&str] = &["anthropic", "gemini", "openai"];

/// Explicit default model ID per provider. This product policy is independent
/// of [`ModelTier`].
const DEFAULT_ANTHROPIC: &str = "claude-opus-5";
// Deliberate product policy: GPT-5.6 Sol is the OpenAI/global default even
// while OpenAI limits access to preview-enabled API organizations and Codex
// workspaces. Operators without preview access must explicitly pin GPT-5.5.
const DEFAULT_OPENAI: &str = "gpt-5.6-sol";
const DEFAULT_GEMINI: &str = "gemini-3.5-flash";

/// Default model table consumed by [`ModelCatalog::default_model`].
const DEFAULT_MODELS: &[(Provider, &str)] = &[
    (Provider::Anthropic, DEFAULT_ANTHROPIC),
    (Provider::OpenAI, DEFAULT_OPENAI),
    (Provider::Gemini, DEFAULT_GEMINI),
];

/// Image-generation default model ID per provider.
const IMAGE_DEFAULT_OPENAI: &str = "gpt-image-2";
const IMAGE_DEFAULT_GEMINI: &str = "gemini-3.1-flash-image-preview";

/// Image-generation default table consumed by
/// [`ModelCatalog::default_image_generation_model`].
const IMAGE_GENERATION_DEFAULTS: &[(Provider, &str)] = &[
    (Provider::Gemini, IMAGE_DEFAULT_GEMINI),
    (Provider::OpenAI, IMAGE_DEFAULT_OPENAI),
];

/// Canonical cross-provider priority order, owned by the catalog seam. A
/// surface that must pick "the best available provider" (e.g. when several
/// API keys are present) consults this order instead of hand-coding its own.
const PROVIDER_PRIORITY: &[Provider] = &[Provider::Anthropic, Provider::OpenAI, Provider::Gemini];

/// OpenAI hosted image tool host model. This route detail is deliberately
/// catalog-owned so provider executors do not infer policy from model names.
const OPENAI_HOSTED_IMAGE_PROVIDER_CALL_MODEL: &str = "gpt-5.4";

/// Image-generation-only model rows. OpenAI text catalog models are admitted
/// by `image_generation_model()` and route through the same hosted tool.
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
// Canonical catalog instance
// ---------------------------------------------------------------------------

/// The canonical [`ModelCatalog`] instance wiring this crate's static data.
///
/// Pass this to core seams that take an explicit catalog parameter
/// (`ModelRegistry::from_config`, `Config::model_registry`, …).
pub fn canonical() -> ModelCatalog {
    ModelCatalog {
        entries: catalog(),
        capabilities: capabilities::capability_table(),
        provider_defaults: provider_defaults(),
        image_generation_models: IMAGE_GENERATION_MODELS,
        providers: CATALOG_PROVIDERS,
        default_models: DEFAULT_MODELS,
        image_generation_defaults: IMAGE_GENERATION_DEFAULTS,
        // OpenAI owns the global default recommendation, so fresh sessions and
        // provider-scoped selection move together when that recommendation changes.
        global_default_model: DEFAULT_OPENAI,
        provider_priority: PROVIDER_PRIORITY,
    }
}

// ---------------------------------------------------------------------------
// Public API (historical function surface, delegating to the canonical
// catalog)
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
                release_stage: c.release_stage,
                context_window: c.context_window,
                max_output_tokens: c.max_output_tokens,
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
    canonical().default_model(provider)
}

/// Return the default image-generation model profile for a typed provider.
pub fn default_image_generation_model(provider: Provider) -> Option<ImageGenerationModelProfile> {
    canonical().default_image_generation_model(provider)
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
    canonical().image_generation_model(provider, model_id)
}

/// Infer a typed image-generation provider from the catalog.
///
/// This is intentionally catalog-only: unknown image-like names fail closed
/// instead of being accepted by prefix folklore.
pub fn image_generation_provider_for_model(model_id: &str) -> Option<Provider> {
    canonical().image_generation_provider_for_model(model_id)
}

/// Return an iterator over model IDs in the catalog for a typed provider.
pub fn allowed_models(provider: Provider) -> impl Iterator<Item = &'static str> + 'static {
    canonical().allowed_models(provider)
}

/// Look up a specific catalog entry by typed provider and model ID.
pub fn entry_for(provider: Provider, model_id: &str) -> Option<&'static CatalogEntry> {
    canonical().entry_for(provider, model_id)
}

/// Infer a typed provider from the catalog by exact model ID match.
///
/// Returns `None` for uncatalogued models; callers that admit custom models
/// must resolve them through `ModelRegistry`, not name prefixes.
pub fn infer_provider(model: &str) -> Option<Provider> {
    canonical().infer_provider(model)
}

/// Approximate per-provider HTTP request-size cap in bytes.
///
/// This is an API-ENDPOINT property, not a per-model capability: every model
/// behind a provider's messages endpoint shares the same request-body cap,
/// which is why the value lives here as provider data rather than duplicated
/// across the per-model capability rows. It arms the byte-aware compaction
/// trigger (fires at 4/5 of the cap) so a byte-heavy transcript compacts
/// BEFORE the provider starts rejecting every turn — the 2026-07-29
/// household incident grew an inline-media transcript past Anthropic's cap
/// and failed terminally with `request_too_large` while the token trigger
/// never came close.
///
/// All values are APPROXIMATE and deliberately conservative: providers do
/// not contractually pin these limits, and the config knob
/// (`compaction.max_request_bytes`) overrides them.
///
/// - Anthropic: the Messages API rejects requests in the ~9-10 MB class
///   (observed in the incident); 9 MB is the conservative bound.
/// - OpenAI: the vision/Responses guidance documents a ~50 MB total payload
///   limit per request.
/// - Gemini: the API documents a ~20 MB total request size for inline data.
pub fn approximate_request_byte_cap(provider: Provider) -> Option<u64> {
    match provider {
        Provider::Anthropic => Some(9_000_000),
        Provider::OpenAI => Some(50_000_000),
        Provider::Gemini => Some(20_000_000),
        Provider::SelfHosted | Provider::Other => None,
    }
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
                let default_id = DEFAULT_MODELS
                    .iter()
                    .find(|(p, _)| *p == provider)
                    .map(|(_, id)| *id)?;
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
    DEFAULTS.get_or_init(|| canonical().image_generation_provider_defaults())
}

/// The platform-wide default model when no provider/model is otherwise
/// specified. The single global default owned by the catalog seam — surfaces
/// must read this instead of hardcoding a model literal.
pub fn global_default_model() -> &'static str {
    canonical().global_default_model
}

/// Canonical cross-provider priority order, owned by the catalog seam. A
/// surface that must pick "the best available provider" (e.g. when several
/// API keys are present) consults this order instead of hand-coding its own.
pub fn provider_priority() -> &'static [Provider] {
    PROVIDER_PRIORITY
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::ModelReleaseStage;

    #[test]
    fn release_stage_is_projected_from_capability_authority() {
        let entries = catalog();
        for capabilities in capabilities::all_capabilities() {
            let entry = entries.iter().find(|entry| {
                entry.id == capabilities.id && entry.provider == capabilities.provider.as_str()
            });
            if let Some(entry) = entry {
                assert_eq!(entry.release_stage, capabilities.release_stage);
            } else {
                assert!(
                    entry.is_some(),
                    "every capability row must project to the catalog"
                );
            }
        }
        let experimental = entries
            .iter()
            .filter(|entry| entry.release_stage == ModelReleaseStage::Experimental)
            .collect::<Vec<_>>();
        assert_eq!(experimental.len(), 1);
        assert_eq!(experimental[0].id, "gpt-live-1-codex");
        assert_eq!(experimental[0].provider, Provider::OpenAI.as_str());
        assert_eq!(experimental[0].context_window, None);
        assert_eq!(experimental[0].max_output_tokens, None);
    }

    #[test]
    fn approximate_request_byte_caps_cover_the_major_providers() {
        // Every cataloged provider must have an armed byte cap; providers
        // outside the catalog (self-hosted endpoints have operator-defined
        // limits) must stay None so the byte trigger remains opt-in there.
        for &provider in CATALOG_PROVIDERS {
            assert!(
                approximate_request_byte_cap(provider).is_some(),
                "cataloged provider {provider:?} must carry a request-byte cap"
            );
        }
        assert_eq!(approximate_request_byte_cap(Provider::SelfHosted), None);
        assert_eq!(approximate_request_byte_cap(Provider::Other), None);
        // Pin the incident-class value: Anthropic's cap is the one the
        // 2026-07-29 household transcript crossed.
        assert_eq!(
            approximate_request_byte_cap(Provider::Anthropic),
            Some(9_000_000)
        );
    }
}
