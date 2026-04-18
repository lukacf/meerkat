//! Curated model catalog — a narrow projection over [`crate::capabilities`].
//!
//! The catalog surfaces the small subset of data that is stable across the
//! platform: id, provider, display name, tier, context window, and max output
//! tokens. Richer capability data (effort levels, thinking modes, beta
//! headers) lives in [`crate::capabilities`] and is not re-exposed here.
//!
//! `meerkat-core` reads defaults from this module; `config_template.toml` is
//! validated against it.

use crate::capabilities;
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

/// Default model ID per provider. First recommended model wins.
const DEFAULT_ANTHROPIC: &str = "claude-opus-4-7";
const DEFAULT_OPENAI: &str = "gpt-5.4";
const DEFAULT_GEMINI: &str = "gemini-3-flash-preview";

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
                provider: c.provider,
                tier: c.tier,
                context_window: Some(c.context_window),
                max_output_tokens: Some(c.max_output_tokens),
            })
            .collect()
    })
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
    catalog()
        .iter()
        .filter(move |e| e.provider == provider.as_str())
        .map(|e| e.id)
}

/// Look up a specific catalog entry by provider and model ID.
pub fn entry_for(provider: &str, model_id: &str) -> Option<&'static CatalogEntry> {
    catalog()
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
                let models: Vec<CatalogEntry> = catalog()
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
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
        for &provider in PROVIDER_NAMES {
            let allowed: Vec<&str> = allowed_models(provider).collect();
            let from_catalog: Vec<&str> = catalog()
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

    #[test]
    fn catalog_matches_capability_table() {
        for entry in catalog() {
            let caps = capabilities::capabilities_for(entry.provider, entry.id)
                .expect("catalog entry must have a capability row");
            assert_eq!(entry.id, caps.id);
            assert_eq!(entry.provider, caps.provider);
            assert_eq!(entry.display_name, caps.display_name);
            assert_eq!(entry.tier, caps.tier);
            assert_eq!(entry.context_window, Some(caps.context_window));
            assert_eq!(entry.max_output_tokens, Some(caps.max_output_tokens));
        }
    }
}
