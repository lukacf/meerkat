//! OpenAI catalog-backed request helpers.
//!
//! Capability facts for OpenAI models live in
//! [`crate::model_profile::capabilities::openai`]. This module only exposes
//! request-shaping helpers for provider clients; uncatalogued model IDs do not
//! synthesize semantic capabilities from name prefixes or substrings.

use crate::model_profile::capabilities::capabilities_for;

/// Whether the model accepts a non-default `temperature`.
///
/// Catalog rows are authoritative. Unknown model IDs return `false` so callers
/// do not send optional provider parameters based on model-name folklore.
pub fn supports_temperature(model: &str) -> bool {
    capabilities_for("openai", model).is_some_and(|caps| caps.supports_temperature)
}

/// Whether the model supports explicit reasoning effort control.
///
/// Catalog rows are authoritative. Unknown model IDs return `false` so callers
/// do not send reasoning controls based on model-name folklore.
pub fn supports_reasoning(model: &str) -> bool {
    capabilities_for("openai", model).is_some_and(|caps| caps.supports_reasoning)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn supports_temperature_uses_catalog_rows() {
        assert!(!supports_temperature("gpt-5.4"));
        assert!(!supports_temperature("gpt-5.3-codex"));
    }

    #[test]
    fn supports_reasoning_uses_catalog_rows() {
        assert!(supports_reasoning("gpt-5.4"));
        assert!(supports_reasoning("gpt-5.3-codex"));
    }

    #[test]
    fn unknown_models_are_conservative() {
        assert!(!supports_temperature("gpt-5.9-future"));
        assert!(!supports_temperature("gpt-4-future"));
        assert!(!supports_reasoning("gpt-5.9-future"));
        assert!(!supports_reasoning("future-codex"));
    }
}
