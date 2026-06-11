//! Curated model catalog vocabulary — the typed shape of catalog entries.
//!
//! The catalog surfaces the small subset of data that is stable across the
//! platform: id, provider, display name, tier, context window, and max output
//! tokens. Richer capability data (effort levels, thinking modes, beta
//! headers) lives in [`super::capabilities`].
//!
//! This module owns only the *types*. The rows themselves are provider data
//! and live in the `meerkat-models` crate; core consumers receive them as an
//! explicit [`super::ModelCatalog`] parameter.

use crate::Provider;
use serde::{Deserialize, Serialize};

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
    /// Model identifier.
    pub id: &'static str,
    /// Human-readable display name.
    pub display_name: &'static str,
    /// Canonical provider string (a [`Provider::as_str`] projection).
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
