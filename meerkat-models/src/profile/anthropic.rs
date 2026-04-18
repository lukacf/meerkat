//! Anthropic family detection and legacy fallback for non-catalog model IDs.
//!
//! All capability data for catalog models lives in
//! [`crate::capabilities::anthropic`]. This module is retained only to
//! synthesize a [`ModelCapabilities`] for model IDs that aren't in the static
//! catalog (e.g., dated snapshots such as `claude-haiku-4-5-20251001` or
//! future prefixes that haven't been added yet).

use crate::capabilities::{BetaHeader, ModelCapabilities, ThinkingSupport, capabilities_for};
use crate::catalog::ModelTier;

const BETA_COMPACTION: BetaHeader = BetaHeader {
    feature: "compaction",
    header_name: "anthropic-beta",
    header_value: "compact-2026-01-12",
};

const BETA_STRUCTURED_OUTPUT: BetaHeader = BetaHeader {
    feature: "structured_output",
    header_name: "anthropic-beta",
    header_value: "structured-outputs-2025-11-13",
};

const BETA_INTERLEAVED_THINKING: BetaHeader = BetaHeader {
    feature: "interleaved_thinking",
    header_name: "anthropic-beta",
    header_value: "interleaved-thinking-2025-05-14",
};

const ADAPTIVE_COMPACTION_BETAS: &[BetaHeader] = &[
    BETA_COMPACTION,
    BETA_STRUCTURED_OUTPUT,
    BETA_INTERLEAVED_THINKING,
];
const STANDARD_BETAS: &[BetaHeader] = &[BETA_STRUCTURED_OUTPUT, BETA_INTERLEAVED_THINKING];

/// Fallback effort tiers for non-catalog `claude-opus-4-7*` prefixes.
const OPUS_47_EFFORT: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// Fallback effort tiers for non-catalog `claude-opus-4-6*` prefixes.
const OPUS_46_EFFORT: &[&str] = &["low", "medium", "high", "max"];

/// Detect the model family. Returns `None` for non-Anthropic models.
fn detect_family(model: &str) -> Option<&'static str> {
    let m = model.to_ascii_lowercase();
    if m.starts_with("claude-opus-4") {
        Some("claude-opus-4")
    } else if m.starts_with("claude-sonnet-4") {
        Some("claude-sonnet-4")
    } else if m.starts_with("claude-haiku-4") {
        Some("claude-haiku-4")
    } else if m.starts_with("claude-") {
        Some("claude")
    } else {
        None
    }
}

/// Whether the model accepts a non-default `temperature`.
///
/// Consumed by `meerkat-client` at request-build time to decide whether to
/// forward a caller's temperature to the API. Catalog models project
/// [`ModelCapabilities::supports_temperature`]; non-catalog IDs fall through
/// [`fallback_caps`] so the check tracks the per-prefix heuristic (e.g.,
/// Opus 4.7 snapshots reject temperature).
pub fn supports_temperature(model: &str) -> bool {
    if let Some(caps) = capabilities_for("anthropic", model) {
        return caps.supports_temperature;
    }
    fallback_caps(model).is_none_or(|caps| caps.supports_temperature)
}

/// Per-subfamily shape used by [`fallback_caps`] to produce a
/// `ModelCapabilities` that matches the authoritative catalog row for the
/// nearest known relative.
struct FallbackShape {
    thinking: ThinkingSupport,
    effort: &'static [&'static str],
    temperature: bool,
    top_p: bool,
    top_k: bool,
    inference_geo: bool,
    compaction: bool,
    betas: &'static [BetaHeader],
}

/// Shape for non-catalog `claude-opus-4-7*` prefixes — mirrors the Opus 4.7
/// catalog row: adaptive-only thinking, rejects temperature/top_p/top_k,
/// `xhigh` effort, inference_geo + compaction.
const FALLBACK_OPUS_47: FallbackShape = FallbackShape {
    thinking: ThinkingSupport::AnthropicAdaptiveOnly,
    effort: OPUS_47_EFFORT,
    temperature: false,
    top_p: false,
    top_k: false,
    inference_geo: true,
    compaction: true,
    betas: ADAPTIVE_COMPACTION_BETAS,
};

/// Shape for non-catalog `claude-opus-4-6*` prefixes — mirrors the Opus 4.6
/// catalog row: adaptive + enabled thinking, effort without `xhigh`,
/// inference_geo + compaction.
const FALLBACK_OPUS_46: FallbackShape = FallbackShape {
    thinking: ThinkingSupport::AnthropicAdaptiveAndEnabled,
    effort: OPUS_46_EFFORT,
    temperature: true,
    top_p: true,
    top_k: true,
    inference_geo: true,
    compaction: true,
    betas: ADAPTIVE_COMPACTION_BETAS,
};

/// Shape for other Claude models (Sonnet 4.x, Haiku 4.x, Opus 4.5, catch-all).
/// Enabled-only thinking, no effort, no compaction, no inference_geo. Mirrors
/// the pre-refactor "standard" bucket.
const FALLBACK_STANDARD: FallbackShape = FallbackShape {
    thinking: ThinkingSupport::AnthropicEnabledOnly,
    effort: &[],
    temperature: true,
    top_p: false,
    top_k: true,
    inference_geo: false,
    compaction: false,
    betas: STANDARD_BETAS,
};

fn fallback_shape(model_lower: &str) -> FallbackShape {
    if model_lower.starts_with("claude-opus-4-7") {
        FALLBACK_OPUS_47
    } else if model_lower.starts_with("claude-opus-4-6") {
        FALLBACK_OPUS_46
    } else {
        FALLBACK_STANDARD
    }
}

/// Synthesize capabilities for an Anthropic model ID that isn't in the catalog.
///
/// The shape is chosen by prefix match against known subfamilies so that
/// ad-hoc model IDs (e.g., dated `claude-opus-4-7-20260501` snapshots) get
/// capabilities matching the authoritative catalog row for the nearest
/// relative — not a looser "bucket" shared across subfamilies.
pub fn fallback_caps(model: &str) -> Option<ModelCapabilities> {
    let family = detect_family(model)?;
    let m = model.to_ascii_lowercase();
    let shape = fallback_shape(&m);
    let call_timeout_secs = match family {
        "claude-opus-4" => Some(300),
        "claude-sonnet-4" => Some(120),
        "claude-haiku-4" => Some(60),
        _ => None,
    };
    Some(ModelCapabilities {
        id: "",
        provider: "anthropic",
        display_name: "",
        tier: ModelTier::Supported,
        model_family: family,
        context_window: 200_000,
        max_output_tokens: 16_384,
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: false,
        supports_temperature: shape.temperature,
        supports_top_p: shape.top_p,
        supports_top_k: shape.top_k,
        thinking: shape.thinking,
        supports_reasoning: false,
        effort_levels: shape.effort,
        supports_web_search: true,
        supports_inference_geo: shape.inference_geo,
        supports_compaction: shape.compaction,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: true,
        beta_headers: shape.betas,
        call_timeout_secs,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn detect_family_known_prefixes() {
        assert_eq!(detect_family("claude-opus-4-7"), Some("claude-opus-4"));
        assert_eq!(detect_family("claude-sonnet-4-6"), Some("claude-sonnet-4"));
        assert_eq!(
            detect_family("claude-haiku-4-5-20251001"),
            Some("claude-haiku-4"),
        );
        assert_eq!(detect_family("claude-something-else"), Some("claude"));
        assert_eq!(detect_family("gpt-5.2"), None);
    }

    #[test]
    fn fallback_opus_47_matches_catalog_row() {
        let caps = fallback_caps("claude-opus-4-7-20260501-preview").unwrap();
        assert_eq!(caps.thinking, ThinkingSupport::AnthropicAdaptiveOnly);
        assert!(caps.effort_levels.contains(&"xhigh"));
        assert!(!caps.supports_temperature);
        assert!(!caps.supports_top_p);
        assert!(!caps.supports_top_k);
        assert!(caps.supports_inference_geo);
        assert!(caps.supports_compaction);
    }

    #[test]
    fn fallback_opus_46_matches_catalog_row() {
        let caps = fallback_caps("claude-opus-4-6-future-snapshot").unwrap();
        assert_eq!(caps.thinking, ThinkingSupport::AnthropicAdaptiveAndEnabled);
        assert!(!caps.effort_levels.contains(&"xhigh"));
        assert!(caps.supports_temperature);
        assert!(caps.supports_top_p);
        assert!(caps.supports_top_k);
        assert!(caps.supports_inference_geo);
        assert!(caps.supports_compaction);
    }

    #[test]
    fn fallback_standard_shape_for_older_prefixes() {
        let caps = fallback_caps("claude-haiku-4-5-20251001").unwrap();
        assert_eq!(caps.thinking, ThinkingSupport::AnthropicEnabledOnly);
        assert!(caps.effort_levels.is_empty());
        assert!(!caps.supports_inference_geo);
        assert!(!caps.supports_compaction);

        let caps = fallback_caps("claude-sonnet-4-5").unwrap();
        assert_eq!(caps.thinking, ThinkingSupport::AnthropicEnabledOnly);
        assert!(caps.effort_levels.is_empty());

        let caps = fallback_caps("claude-opus-4-5").unwrap();
        assert_eq!(caps.thinking, ThinkingSupport::AnthropicEnabledOnly);
    }

    #[test]
    fn supports_temperature_catalog_opus_47_false() {
        // Catalog row wins: Opus 4.7 rejects temperature.
        assert!(!supports_temperature("claude-opus-4-7"));
    }

    #[test]
    fn supports_temperature_catalog_opus_46_true() {
        assert!(supports_temperature("claude-opus-4-6"));
    }

    #[test]
    fn supports_temperature_fallback_opus_47_snapshot_false() {
        // Non-catalog Opus 4.7 snapshot still rejects temperature.
        assert!(!supports_temperature("claude-opus-4-7-20260501-preview"));
    }

    #[test]
    fn supports_temperature_fallback_sonnet_true() {
        assert!(supports_temperature("claude-sonnet-4-5"));
    }

    #[test]
    fn supports_temperature_unknown_family_default_true() {
        // Completely unknown claude-* catches the standard shape (temp=true).
        assert!(supports_temperature("claude-future-5"));
    }
}
