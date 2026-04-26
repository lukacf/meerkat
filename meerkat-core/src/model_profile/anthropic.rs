//! Anthropic catalog-backed request helpers.
//!
//! Capability facts for Anthropic models live in
//! [`crate::model_profile::capabilities::anthropic`]. This module only exposes
//! request-shaping helpers for provider clients; uncatalogued model IDs do not
//! synthesize semantic capabilities from name prefixes.

use crate::model_profile::capabilities::capabilities_for;

/// Whether the model accepts a non-default `temperature`.
///
/// Catalog rows are authoritative. Unknown model IDs return `false` so callers
/// do not send optional provider parameters based on model-name folklore.
pub fn supports_temperature(model: &str) -> bool {
    capabilities_for("anthropic", model).is_some_and(|caps| caps.supports_temperature)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn supports_temperature_uses_catalog_rows() {
        assert!(!supports_temperature("claude-opus-4-7"));
        assert!(supports_temperature("claude-opus-4-6"));
    }

    #[test]
    fn supports_temperature_unknown_model_is_conservative() {
        assert!(!supports_temperature("claude-opus-4-7-20260501-preview"));
        assert!(!supports_temperature("claude-future-5"));
    }
}
