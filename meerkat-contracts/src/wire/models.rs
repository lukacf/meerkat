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
    /// JSON Schema describing accepted provider-specific parameters.
    pub params_schema: serde_json::Value,
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

/// Response for `models/catalog` — the compiled-in model catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ModelsCatalogResponse {
    /// Contract version.
    pub contract_version: ContractVersion,
    /// Provider-grouped catalog data.
    pub providers: Vec<ProviderCatalog>,
}
