//! Curated model catalog — the canonical source of truth for supported models.
//!
//! All model defaults, allowlists, and display metadata derive from this module.
//! `meerkat-core` reads defaults from here; `config_template.toml` is validated against it.

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

// ---------------------------------------------------------------------------
// Static catalog data
// ---------------------------------------------------------------------------

/// Canonical provider names in alphabetical order.
const PROVIDER_NAMES: &[&str] = &["anthropic", "gemini", "openai"];

const CATALOG_DATA: &[CatalogEntry] = &[
    // ── Anthropic ──────────────────────────────────────────────────────
    CatalogEntry {
        id: "claude-opus-4-6",
        display_name: "Claude Opus 4.6",
        provider: "anthropic",
        tier: ModelTier::Recommended,
        context_window: Some(200_000),
        max_output_tokens: Some(32_768),
    },
    CatalogEntry {
        id: "claude-sonnet-4-6",
        display_name: "Claude Sonnet 4.6",
        provider: "anthropic",
        tier: ModelTier::Recommended,
        context_window: Some(200_000),
        max_output_tokens: Some(16_384),
    },
    CatalogEntry {
        id: "claude-sonnet-4-5",
        display_name: "Claude Sonnet 4.5",
        provider: "anthropic",
        tier: ModelTier::Supported,
        context_window: Some(200_000),
        max_output_tokens: Some(16_384),
    },
    CatalogEntry {
        id: "claude-opus-4-5",
        display_name: "Claude Opus 4.5",
        provider: "anthropic",
        tier: ModelTier::Supported,
        context_window: Some(200_000),
        max_output_tokens: Some(32_768),
    },
    // ── OpenAI ─────────────────────────────────────────────────────────
    CatalogEntry {
        id: "gpt-5.2",
        display_name: "GPT-5.2",
        provider: "openai",
        tier: ModelTier::Recommended,
        context_window: Some(128_000),
        max_output_tokens: Some(16_384),
    },
    CatalogEntry {
        id: "gpt-5.2-pro",
        display_name: "GPT-5.2 Pro",
        provider: "openai",
        tier: ModelTier::Supported,
        context_window: Some(128_000),
        max_output_tokens: Some(16_384),
    },
    // ── Gemini ─────────────────────────────────────────────────────────
    CatalogEntry {
        id: "gemini-3-flash-preview",
        display_name: "Gemini 3 Flash Preview",
        provider: "gemini",
        tier: ModelTier::Recommended,
        context_window: Some(1_000_000),
        max_output_tokens: Some(8_192),
    },
    CatalogEntry {
        id: "gemini-3-pro-preview",
        display_name: "Gemini 3 Pro Preview",
        provider: "gemini",
        tier: ModelTier::Supported,
        context_window: Some(1_000_000),
        max_output_tokens: Some(8_192),
    },
];

/// Default model ID per provider. First recommended model wins.
const DEFAULT_ANTHROPIC: &str = "claude-opus-4-6";
const DEFAULT_OPENAI: &str = "gpt-5.2";
const DEFAULT_GEMINI: &str = "gemini-3-flash-preview";

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Return all catalog entries.
pub fn catalog() -> &'static [CatalogEntry] {
    CATALOG_DATA
}

/// Return canonical provider names.
pub fn provider_names() -> &'static [&'static str] {
    PROVIDER_NAMES
}

/// Return the default model ID for a provider, or `None` if the provider is unknown.
pub fn default_model(provider: &str) -> Option<&'static str> {
    match provider {
        "anthropic" => Some(DEFAULT_ANTHROPIC),
        "openai" => Some(DEFAULT_OPENAI),
        "gemini" => Some(DEFAULT_GEMINI),
        _ => None,
    }
}

/// Return an iterator over model IDs in the catalog for a given provider.
pub fn allowed_models(provider: &str) -> impl Iterator<Item = &'static str> + 'static {
    let provider = provider.to_string();
    CATALOG_DATA
        .iter()
        .filter(move |e| e.provider == provider.as_str())
        .map(|e| e.id)
}

/// Look up a specific catalog entry by provider and model ID.
pub fn entry_for(provider: &str, model_id: &str) -> Option<&'static CatalogEntry> {
    CATALOG_DATA
        .iter()
        .find(|e| e.provider == provider && e.id == model_id)
}

/// Return provider-grouped catalog data with default model IDs.
///
/// Built once via `OnceLock`, returned as a `&'static` slice.
pub fn provider_defaults() -> &'static [ProviderDefaults] {
    static DEFAULTS: OnceLock<Vec<ProviderDefaults>> = OnceLock::new();
    DEFAULTS.get_or_init(|| {
        PROVIDER_NAMES
            .iter()
            .filter_map(|&provider| {
                let default_id = default_model(provider)?;
                let models: Vec<CatalogEntry> = CATALOG_DATA
                    .iter()
                    .filter(|e| e.provider == provider)
                    .cloned()
                    .collect();
                Some(ProviderDefaults {
                    provider,
                    default_model_id: default_id,
                    models,
                })
            })
            .collect()
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn exactly_one_default_per_provider() {
        for &provider in PROVIDER_NAMES {
            let result = default_model(provider);
            assert!(
                result.is_some(),
                "provider '{provider}' must have a default model"
            );
        }
    }

    #[test]
    fn defaults_exist_in_catalog() {
        for &provider in PROVIDER_NAMES {
            let default = default_model(provider);
            assert!(
                default.is_some(),
                "provider '{provider}' must have a default model"
            );
            if let Some(default) = default {
                let entry = entry_for(provider, default);
                assert!(
                    entry.is_some(),
                    "default model '{default}' for provider '{provider}' must exist in catalog"
                );
            }
        }
    }

    #[test]
    fn all_provider_strings_canonical() {
        let canonical: HashSet<&str> = PROVIDER_NAMES.iter().copied().collect();
        for entry in CATALOG_DATA {
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
            let ids: Vec<&str> = CATALOG_DATA
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
        for &provider in PROVIDER_NAMES {
            let allowed: Vec<&str> = allowed_models(provider).collect();
            let from_catalog: Vec<&str> = CATALOG_DATA
                .iter()
                .filter(|e| e.provider == provider)
                .map(|e| e.id)
                .collect();
            assert_eq!(
                allowed, from_catalog,
                "allowed_models('{provider}') must match catalog entries"
            );
        }
    }
}
