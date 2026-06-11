//! Synthetic [`ModelCatalog`] fixture for meerkat-core unit tests.
//!
//! Core owns catalog *mechanics*, not data — so its tests run against an
//! obviously-fake catalog. Tests that pin real model rows live in
//! `meerkat-models`, the crate that owns the canonical data.

use super::ModelCatalog;
use crate::Provider;
use crate::model_profile::capabilities::{ModelCapabilities, ThinkingSupport};
use crate::model_profile::catalog::{
    CatalogEntry, ImageGenerationModelProfile, ImageGenerationModelRoute,
    ImageGenerationSizeParameter, ModelTier, ProviderDefaults,
};
use std::sync::LazyLock;

pub(crate) const ANTHROPIC_MODEL: &str = "test-anthropic-default";
pub(crate) const OPENAI_MODEL: &str = "test-openai-default";
pub(crate) const VIDEO_MODEL: &str = "test-gemini-video";
pub(crate) const GEMINI_IMAGE_MODEL: &str = "test-gemini-image";
pub(crate) const OPENAI_IMAGE_MODEL: &str = "test-openai-image";

const BASE_CAPS: ModelCapabilities = ModelCapabilities {
    id: "",
    provider: Provider::Other,
    display_name: "",
    tier: ModelTier::Supported,
    model_family: "test-family",
    context_window: 200_000,
    max_output_tokens: 64_000,
    context_window_beta: None,
    max_output_tokens_beta: None,
    vision: true,
    image_tool_results: false,
    inline_video: false,
    realtime: false,
    realtime_supports_provider_managed_turns: false,
    realtime_supports_explicit_commit: false,
    realtime_interrupt_supported: false,
    realtime_transcript_supported: false,
    transcription_companion_model: None,
    image_generation: false,
    supports_temperature: true,
    supports_top_p: false,
    supports_top_k: false,
    thinking: ThinkingSupport::None,
    supports_reasoning: false,
    effort_levels: &[],
    supports_web_search: false,
    supports_inference_geo: false,
    supports_compaction: false,
    supports_structured_output: false,
    supports_legacy_penalties: false,
    supports_thinking_budget_legacy: false,
    beta_headers: &[],
    call_timeout_secs: Some(600),
};

const CAPABILITIES: &[ModelCapabilities] = &[
    ModelCapabilities {
        id: ANTHROPIC_MODEL,
        provider: Provider::Anthropic,
        display_name: "Test Anthropic Default",
        tier: ModelTier::Recommended,
        image_tool_results: true,
        thinking: ThinkingSupport::AnthropicAdaptiveOnly,
        supports_temperature: false,
        supports_web_search: true,
        ..BASE_CAPS
    },
    ModelCapabilities {
        id: OPENAI_MODEL,
        provider: Provider::OpenAI,
        display_name: "Test OpenAI Default",
        tier: ModelTier::Recommended,
        supports_reasoning: true,
        image_generation: true,
        supports_web_search: true,
        ..BASE_CAPS
    },
    ModelCapabilities {
        id: VIDEO_MODEL,
        provider: Provider::Gemini,
        display_name: "Test Gemini Video",
        tier: ModelTier::Recommended,
        image_tool_results: true,
        inline_video: true,
        thinking: ThinkingSupport::GeminiThinkingLevel,
        supports_web_search: true,
        ..BASE_CAPS
    },
];

const ENTRIES: &[CatalogEntry] = &[
    CatalogEntry {
        id: ANTHROPIC_MODEL,
        display_name: "Test Anthropic Default",
        provider: "anthropic",
        tier: ModelTier::Recommended,
        context_window: Some(200_000),
        max_output_tokens: Some(64_000),
    },
    CatalogEntry {
        id: OPENAI_MODEL,
        display_name: "Test OpenAI Default",
        provider: "openai",
        tier: ModelTier::Recommended,
        context_window: Some(200_000),
        max_output_tokens: Some(64_000),
    },
    CatalogEntry {
        id: VIDEO_MODEL,
        display_name: "Test Gemini Video",
        provider: "gemini",
        tier: ModelTier::Recommended,
        context_window: Some(200_000),
        max_output_tokens: Some(64_000),
    },
];

const IMAGE_GENERATION_MODELS: &[ImageGenerationModelProfile] = &[
    ImageGenerationModelProfile {
        provider: Provider::OpenAI,
        model_id: OPENAI_IMAGE_MODEL,
        display_name: "Test OpenAI Image",
        tier: ModelTier::Recommended,
        route: ImageGenerationModelRoute::OpenAiHostedResponsesTool {
            provider_call_model_id: Some(OPENAI_MODEL),
            tool_model_id: OPENAI_IMAGE_MODEL,
        },
    },
    ImageGenerationModelProfile {
        provider: Provider::Gemini,
        model_id: GEMINI_IMAGE_MODEL,
        display_name: "Test Gemini Image",
        tier: ModelTier::Recommended,
        route: ImageGenerationModelRoute::GeminiNativeModel {
            image_size_parameter: ImageGenerationSizeParameter::Supported,
        },
    },
];

const PROVIDERS: &[Provider] = &[Provider::Anthropic, Provider::Gemini, Provider::OpenAI];

const DEFAULT_MODELS: &[(Provider, &str)] = &[
    (Provider::Anthropic, ANTHROPIC_MODEL),
    (Provider::OpenAI, OPENAI_MODEL),
    (Provider::Gemini, VIDEO_MODEL),
];

const IMAGE_GENERATION_DEFAULTS: &[(Provider, &str)] = &[
    (Provider::OpenAI, OPENAI_IMAGE_MODEL),
    (Provider::Gemini, GEMINI_IMAGE_MODEL),
];

const PROVIDER_PRIORITY: &[Provider] = &[Provider::Anthropic, Provider::OpenAI, Provider::Gemini];

static PROVIDER_DEFAULTS: LazyLock<Vec<ProviderDefaults>> = LazyLock::new(|| {
    PROVIDERS
        .iter()
        .filter_map(|&provider| {
            let default_id = DEFAULT_MODELS
                .iter()
                .find(|(p, _)| *p == provider)
                .map(|(_, id)| *id)?;
            Some(ProviderDefaults {
                provider: provider.as_str(),
                default_model_id: default_id,
                models: ENTRIES
                    .iter()
                    .filter(|e| e.provider == provider.as_str())
                    .cloned()
                    .collect(),
            })
        })
        .collect()
});

/// Synthetic catalog instance for core unit tests.
pub(crate) static TEST_CATALOG: LazyLock<ModelCatalog> = LazyLock::new(|| ModelCatalog {
    entries: ENTRIES,
    capabilities: CAPABILITIES,
    provider_defaults: PROVIDER_DEFAULTS.as_slice(),
    image_generation_models: IMAGE_GENERATION_MODELS,
    providers: PROVIDERS,
    default_models: DEFAULT_MODELS,
    image_generation_defaults: IMAGE_GENERATION_DEFAULTS,
    global_default_model: ANTHROPIC_MODEL,
    provider_priority: PROVIDER_PRIORITY,
});
