//! Wire types for model catalog responses.

use crate::version::ContractVersion;
use serde::{Deserialize, Serialize};

/// Model recommendation tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireModelTier {
    /// Tested and recommended for production use.
    Recommended,
    /// Supported but not the primary recommendation.
    Supported,
}

/// Runtime profile for a model — capabilities and parameter schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireModelProfile {
    /// Model family identifier (e.g., `"claude-opus-4"`, `"gpt-5"`).
    pub model_family: String,
    /// Whether the model accepts a `temperature` parameter.
    pub supports_temperature: bool,
    /// Whether the model supports extended thinking.
    pub supports_thinking: bool,
    /// Whether the model supports explicit reasoning effort control.
    pub supports_reasoning: bool,
    /// Whether the model supports provider-native web search tools.
    #[serde(default)]
    pub supports_web_search: bool,
    /// Whether the model can reason over visual content.
    #[serde(default)]
    pub vision: bool,
    /// Whether user messages may include image input blocks.
    #[serde(default)]
    pub image_input: bool,
    /// Whether tool results may include image blocks for this model.
    #[serde(default)]
    pub image_tool_results: bool,
    /// Whether the model accepts inline video content in user messages.
    #[serde(default)]
    pub inline_video: bool,
    /// Whether the model supports realtime bidirectional transport.
    #[serde(default)]
    pub realtime: bool,
    /// Whether the resolved provider/model can use image generation.
    #[serde(default)]
    pub image_generation: bool,
    /// JSON Schema describing accepted provider-specific parameters.
    pub params_schema: serde_json::Value,
    /// Beta headers authorized by the model capability catalog.
    #[serde(default)]
    pub beta_headers: Vec<WireModelBetaHeader>,
}

/// Catalog-owned beta header metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireModelBetaHeader {
    pub feature: String,
    pub header_name: String,
    pub header_value: String,
}

/// Stable resolved model/session/member capability projection.
///
/// This is the UI-facing capability shape for already-resolved identities.
/// Static catalog entries can expose the same bits, but callers should read
/// this projection from session/member surfaces when identity can change at
/// runtime.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireResolvedModelCapabilities {
    /// Whether the model can reason over visual content.
    #[serde(default)]
    pub vision: bool,
    /// Whether user messages may include image input blocks.
    #[serde(default)]
    pub image_input: bool,
    /// Whether tool results may include image blocks for this model.
    #[serde(default)]
    pub image_tool_results: bool,
    /// Whether user messages may include inline video blocks.
    #[serde(default)]
    pub inline_video: bool,
    /// Whether the resolved model can drive realtime transport.
    #[serde(default)]
    pub realtime: bool,
    /// Whether provider-native web search is available for this model.
    #[serde(default)]
    pub web_search: bool,
    /// Whether the resolved provider/session can use image generation.
    #[serde(default)]
    pub image_generation: bool,
}

/// A single model entry in the catalog response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CatalogModelEntry {
    /// Model identifier (e.g., `"claude-sonnet-4-6"`).
    pub id: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Recommendation tier.
    pub tier: WireModelTier,
    /// Maximum input context window in tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    /// Maximum output tokens per response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// Backing self-hosted server ID for configured aliases.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    /// Model profile with capability flags and parameter schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<WireModelProfile>,
}

/// Provider-level grouping in the catalog response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderCatalog {
    /// Canonical provider name.
    pub provider: String,
    /// Default model ID for this provider.
    pub default_model_id: String,
    /// All catalog models for this provider.
    pub models: Vec<CatalogModelEntry>,
}

/// Response for `models/catalog` — the resolved model catalog.
///
/// This is **not** a pure compiled-in snapshot. Surfaces build it from the
/// config-backed `ModelRegistry`, which combines the compiled-in
/// `meerkat-models` catalog with any config-declared self-hosted aliases
/// (entries that carry a [`CatalogModelEntry::server_id`]). Two responses for
/// the same binary can therefore differ when the active config declares
/// different self-hosted models.
///
/// Provider resolution for these entries is exact-catalog-match, not name
/// prefix inference: an entry's provider is the one recorded for its catalog
/// id (or its self-hosted alias config), never inferred from a `claude-*` /
/// `gpt-*` / `gemini-*` name prefix. Uncatalogued model names resolve through
/// the registry, not by prefix guessing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ModelsCatalogResponse {
    /// Contract version.
    pub contract_version: ContractVersion,
    /// Provider-grouped catalog data.
    pub providers: Vec<ProviderCatalog>,
}
