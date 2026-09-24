//! Per-model capability tables — the authoritative, data-driven source of truth.
//!
//! Every catalog model has exactly one
//! [`meerkat_core::model_profile::capabilities::ModelCapabilities`] record
//! describing its accepted parameters, thinking modes, modalities, headers,
//! and operational defaults. `ModelProfile` is derived from this record.
//!
//! Lookups cross the typed [`meerkat_core::Provider`] boundary; uncatalogued
//! model IDs do not receive synthesized semantic capabilities.

mod anthropic;
mod gemini;
mod openai;

use meerkat_core::Provider;
use meerkat_core::model_profile::capabilities::ModelCapabilities;
use std::sync::OnceLock;

/// Lookup a model's capabilities by typed provider + id.
///
/// Returns `None` when the provider/model pair has no capability row.
/// Callers must treat `None` as unknown capability truth; uncatalogued model
/// IDs must not synthesize semantic facts from model-name folklore.
pub fn capabilities_for(provider: Provider, model_id: &str) -> Option<&'static ModelCapabilities> {
    crate::catalog::canonical().capabilities_for(provider, model_id)
}

/// Iterate every known capability record across all providers.
pub fn all_capabilities() -> impl Iterator<Item = &'static ModelCapabilities> {
    capability_table().iter()
}

/// Flat, ordered capability table across all providers (Anthropic, OpenAI,
/// Gemini — matching the historical iteration order).
pub(crate) fn capability_table() -> &'static [ModelCapabilities] {
    static TABLE: OnceLock<Vec<ModelCapabilities>> = OnceLock::new();
    TABLE.get_or_init(|| {
        anthropic::CAPABILITIES
            .iter()
            .chain(openai::CAPABILITIES.iter())
            .chain(gemini::CAPABILITIES.iter())
            .copied()
            .collect()
    })
}
