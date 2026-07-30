//! Anthropic catalog-backed request-shaping helpers.
//!
//! Capability facts for Anthropic models live in the typed capability
//! catalog (`meerkat-models`). This module only exposes request-shaping
//! helpers for the Anthropic client; uncatalogued model IDs do not
//! synthesize semantic capabilities from name prefixes.

use meerkat_core::Provider;

/// Whether the model accepts a non-default `temperature`.
///
/// Catalog rows are authoritative. Unknown model IDs return `false` so callers
/// do not send optional provider parameters based on model-name folklore.
pub(crate) fn supports_temperature(model: &str) -> bool {
    meerkat_models::capabilities_for(Provider::Anthropic, model)
        .is_some_and(|caps| caps.supports_temperature)
}

/// Whether the model accepts System messages inside conversation history.
///
/// Catalog rows are authoritative. Unknown and older model IDs return
/// `false`, preserving the leading-system-prefix-only contract.
pub(crate) fn supports_mid_conversation_system_messages(model: &str) -> bool {
    meerkat_models::capabilities_for(Provider::Anthropic, model)
        .is_some_and(|caps| caps.supports_mid_conversation_system_messages)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn supports_temperature_uses_catalog_rows() {
        assert!(!supports_temperature("claude-opus-4-8"));
        assert!(supports_temperature("claude-sonnet-4-6"));
    }

    #[test]
    fn supports_temperature_unknown_model_is_conservative() {
        assert!(!supports_temperature("claude-opus-4-8-20260501-preview"));
        assert!(!supports_temperature("claude-future-5"));
    }

    #[test]
    fn mid_conversation_system_messages_use_catalog_rows() {
        assert!(supports_mid_conversation_system_messages("claude-fable-5"));
        assert!(supports_mid_conversation_system_messages("claude-opus-5"));
        assert!(supports_mid_conversation_system_messages("claude-opus-4-8"));
        assert!(!supports_mid_conversation_system_messages(
            "claude-haiku-4-5-20251001"
        ));
        assert!(!supports_mid_conversation_system_messages(
            "claude-haiku-4-5"
        ));
        assert!(!supports_mid_conversation_system_messages(
            "claude-opus-4-7"
        ));
        assert!(!supports_mid_conversation_system_messages(
            "claude-sonnet-4-6"
        ));
    }

    #[test]
    fn mid_conversation_system_messages_unknown_model_is_conservative() {
        assert!(!supports_mid_conversation_system_messages(
            "claude-opus-4-8-20260501-preview"
        ));
        assert!(!supports_mid_conversation_system_messages(
            "claude-future-5"
        ));
    }
}
