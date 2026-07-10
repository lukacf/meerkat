//! OpenAI catalog-backed request-shaping helpers.
//!
//! Capability facts for OpenAI models live in the typed capability catalog
//! (`meerkat-models`). This module only exposes request-shaping helpers for
//! the OpenAI clients; uncatalogued model IDs do not synthesize semantic
//! capabilities from name prefixes or substrings.

use meerkat_core::Provider;
use meerkat_core::lifecycle::run_primitive::{OpenAiPromptCacheRetention, ReasoningEffort};
use meerkat_core::model_profile::capabilities::{
    OpenAiPromptCacheMode, OpenAiPromptCacheTtl, OpenAiReasoningContext, OpenAiReasoningMode,
    OpenAiTextVerbosity,
};

/// Whether the model accepts a non-default `temperature`.
///
/// Catalog rows are authoritative. Unknown model IDs return `false` so callers
/// do not send optional provider parameters based on model-name folklore.
pub(crate) fn supports_temperature(model: &str) -> bool {
    meerkat_models::capabilities_for(Provider::OpenAI, model)
        .is_some_and(|caps| caps.supports_temperature)
}

/// Whether the model supports explicit reasoning effort control.
///
/// Catalog rows are authoritative. Unknown model IDs return `false` so callers
/// do not send reasoning controls based on model-name folklore.
pub(crate) fn supports_reasoning(model: &str) -> bool {
    meerkat_models::capabilities_for(Provider::OpenAI, model)
        .is_some_and(|caps| caps.supports_reasoning)
}

/// Whether a cataloged model accepts a specific reasoning-effort value.
///
/// `None` means the model is not cataloged. Callers with an explicit internal
/// capability override may deliberately preserve passthrough behavior for
/// custom OpenAI-compatible deployments; ordinary catalog-backed paths should
/// treat `None` conservatively.
pub(crate) fn supports_reasoning_effort(model: &str, effort: ReasoningEffort) -> Option<bool> {
    meerkat_models::capabilities_for(Provider::OpenAI, model).map(|caps| {
        caps.supports_reasoning
            && caps
                .effort_levels
                .iter()
                .any(|level| level.as_wire_str() == effort.as_legacy_str())
    })
}

pub(crate) fn supports_reasoning_mode(model: &str, value: OpenAiReasoningMode) -> Option<bool> {
    meerkat_models::capabilities_for(Provider::OpenAI, model).map(|caps| {
        caps.openai_responses_params
            .is_some_and(|params| params.reasoning_modes.contains(&value))
    })
}

pub(crate) fn supports_reasoning_context(
    model: &str,
    value: OpenAiReasoningContext,
) -> Option<bool> {
    meerkat_models::capabilities_for(Provider::OpenAI, model).map(|caps| {
        caps.openai_responses_params
            .is_some_and(|params| params.reasoning_contexts.contains(&value))
    })
}

pub(crate) fn supports_text_verbosity(model: &str, value: OpenAiTextVerbosity) -> Option<bool> {
    meerkat_models::capabilities_for(Provider::OpenAI, model).map(|caps| {
        caps.openai_responses_params
            .is_some_and(|params| params.text_verbosity_levels.contains(&value))
    })
}

pub(crate) fn supports_prompt_cache_mode(
    model: &str,
    value: OpenAiPromptCacheMode,
) -> Option<bool> {
    meerkat_models::capabilities_for(Provider::OpenAI, model).map(|caps| {
        caps.openai_responses_params
            .is_some_and(|params| params.prompt_cache_modes.contains(&value))
    })
}

pub(crate) fn supports_prompt_cache_ttl(model: &str, value: OpenAiPromptCacheTtl) -> Option<bool> {
    meerkat_models::capabilities_for(Provider::OpenAI, model).map(|caps| {
        caps.openai_responses_params
            .is_some_and(|params| params.prompt_cache_ttls.contains(&value))
    })
}

pub(crate) fn supports_prompt_cache_retention(
    model: &str,
    value: OpenAiPromptCacheRetention,
) -> Option<bool> {
    meerkat_models::capabilities_for(Provider::OpenAI, model).map(|caps| {
        let Some(params) = caps.openai_responses_params else {
            // Preserve the existing path for older catalog rows. Their
            // retention matrix predates the GPT-5.6-specific capability set.
            return true;
        };
        match value {
            OpenAiPromptCacheRetention::InMemory => {
                params.supports_in_memory_prompt_cache_retention
            }
            OpenAiPromptCacheRetention::TwentyFourHours => true,
        }
    })
}

pub(crate) fn supports_legacy_penalties(model: &str) -> Option<bool> {
    meerkat_models::capabilities_for(Provider::OpenAI, model)
        .map(|caps| caps.supports_legacy_penalties)
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
    fn reasoning_effort_membership_is_model_specific() {
        assert_eq!(
            supports_reasoning_effort("gpt-5.6-sol", ReasoningEffort::Max),
            Some(true)
        );
        assert_eq!(
            supports_reasoning_effort("gpt-5.5", ReasoningEffort::Max),
            Some(false)
        );
        assert_eq!(
            supports_reasoning_effort("gpt-5.5", ReasoningEffort::XHigh),
            Some(true)
        );
    }

    #[test]
    fn gpt_56_advanced_responses_params_are_catalog_specific() {
        assert_eq!(
            supports_reasoning_mode("gpt-5.6-sol", OpenAiReasoningMode::Pro),
            Some(true)
        );
        assert_eq!(
            supports_reasoning_context("gpt-5.6-terra", OpenAiReasoningContext::AllTurns),
            Some(true)
        );
        assert_eq!(
            supports_text_verbosity("gpt-5.6-luna", OpenAiTextVerbosity::High),
            Some(true)
        );
        assert_eq!(
            supports_prompt_cache_mode("gpt-5.6-sol", OpenAiPromptCacheMode::Explicit),
            Some(true)
        );
        assert_eq!(
            supports_prompt_cache_ttl("gpt-5.6-sol", OpenAiPromptCacheTtl::ThirtyMinutes),
            Some(true)
        );
        assert_eq!(
            supports_prompt_cache_retention("gpt-5.6-sol", OpenAiPromptCacheRetention::InMemory),
            Some(false)
        );
        assert_eq!(
            supports_prompt_cache_retention(
                "gpt-5.6-sol",
                OpenAiPromptCacheRetention::TwentyFourHours
            ),
            Some(true)
        );
        assert_eq!(supports_legacy_penalties("gpt-5.6-sol"), Some(false));

        assert_eq!(
            supports_reasoning_mode("gpt-5.5", OpenAiReasoningMode::Pro),
            Some(false)
        );
        assert_eq!(supports_legacy_penalties("gpt-5.5"), Some(true));
    }

    #[test]
    fn unknown_models_are_conservative() {
        assert!(!supports_temperature("gpt-5.9-future"));
        assert!(!supports_temperature("gpt-4-future"));
        assert!(!supports_reasoning("gpt-5.9-future"));
        assert!(!supports_reasoning("future-codex"));
        assert_eq!(
            supports_reasoning_effort("gpt-5.9-future", ReasoningEffort::Max),
            None
        );
    }
}
