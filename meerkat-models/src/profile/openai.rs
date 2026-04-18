//! OpenAI family detection and legacy fallback for non-catalog model IDs.
//!
//! All capability data for catalog models lives in
//! [`crate::capabilities::openai`]. This module retains:
//! - [`fallback_caps`] — synthesizes a [`ModelCapabilities`] for non-catalog
//!   model IDs (e.g., `gpt-5.4-pro`, future prefixes) with heuristic family
//!   detection matching the pre-refactor profile shape.
//! - [`supports_temperature`] / [`supports_reasoning`] — public helpers
//!   consumed by `meerkat-client` for request-shape decisions. They now route
//!   through the capability table when a row exists and fall back to the
//!   heuristic otherwise.

use crate::capabilities::{ModelCapabilities, ThinkingSupport, capabilities_for};
use crate::catalog::ModelTier;

const GPT5_REASONING_EFFORT: &[&str] = &["low", "medium", "high"];

/// Returns `true` if the model is in the GPT-5 family by prefix match.
fn is_gpt5_family(model: &str) -> bool {
    model.to_ascii_lowercase().starts_with("gpt-5")
}

/// Returns `true` if the model is a Codex model by substring match.
fn is_codex_family(model: &str) -> bool {
    model.to_ascii_lowercase().contains("codex")
}

fn detect_family(model: &str) -> Option<&'static str> {
    let m = model.to_ascii_lowercase();
    if m.contains("codex") {
        Some("codex")
    } else if m.starts_with("gpt-5") {
        Some("gpt-5")
    } else if m.starts_with("gpt-") || m.starts_with("chatgpt-") {
        Some("gpt")
    } else {
        None
    }
}

/// Whether the model accepts a non-default `temperature`.
///
/// Consumed by `meerkat-client` to decide whether to forward a caller's
/// temperature request to the API. Catalog models project the
/// [`ModelCapabilities.supports_temperature`] bit; unknown model IDs fall
/// back to the pre-refactor heuristic (GPT-5 family and Codex models reject
/// temperature).
pub fn supports_temperature(model: &str) -> bool {
    if let Some(caps) = capabilities_for("openai", model) {
        return caps.supports_temperature;
    }
    !(is_gpt5_family(model) || is_codex_family(model))
}

/// Whether the model supports explicit reasoning effort control (OpenAI's
/// `reasoning.effort`). Consumed by `meerkat-client` for request shaping.
pub fn supports_reasoning(model: &str) -> bool {
    if let Some(caps) = capabilities_for("openai", model) {
        return caps.supports_reasoning;
    }
    is_gpt5_family(model)
}

/// Synthesize capabilities for an OpenAI model ID that isn't in the catalog.
pub fn fallback_caps(model: &str) -> Option<ModelCapabilities> {
    let family = detect_family(model)?;
    let m = model.to_ascii_lowercase();
    let gpt5 = is_gpt5_family(model);
    let codex = is_codex_family(model);
    let reasoning = gpt5;
    // Timeout: gpt-5 + "-pro" substring = 7200s; gpt-5 or codex = 600s;
    // plain gpt = 90s; anything else in this module = None.
    let call_timeout_secs = if m.contains("-pro") && gpt5 {
        Some(7200)
    } else {
        match family {
            "gpt-5" => Some(600),
            "codex" => Some(600),
            "gpt" => Some(90),
            _ => None,
        }
    };
    // Effort levels: gpt-5 family gets reasoning_effort. Non-gpt5 non-codex
    // gets no effort. Codex-only (no gpt-5 prefix) currently gets no effort
    // either per the pre-refactor standard schema.
    let effort_levels: &'static [&'static str] = if gpt5 { GPT5_REASONING_EFFORT } else { &[] };
    Some(ModelCapabilities {
        id: "",
        provider: "openai",
        display_name: "",
        tier: ModelTier::Supported,
        model_family: family,
        context_window: 128_000,
        max_output_tokens: 16_384,
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: false,
        inline_video: false,
        supports_temperature: !(gpt5 || codex),
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::None,
        supports_reasoning: reasoning,
        effort_levels,
        supports_web_search: true,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: gpt5 || family == "gpt",
        supports_thinking_budget_legacy: false,
        beta_headers: &[],
        call_timeout_secs,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn detect_family_known_prefixes() {
        assert_eq!(detect_family("gpt-5.4-pro"), Some("gpt-5"));
        assert_eq!(detect_family("gpt-5.3-codex"), Some("codex"));
        assert_eq!(detect_family("gpt-4-turbo"), Some("gpt"));
        assert_eq!(detect_family("chatgpt-4o"), Some("gpt"));
        assert_eq!(detect_family("claude-opus-4-6"), None);
    }

    #[test]
    fn helpers_reject_non_openai() {
        assert!(!is_gpt5_family("claude-opus-4-6"));
        assert!(!is_codex_family("gpt-5.2"));
    }

    #[test]
    fn supports_temperature_matches_catalog() {
        // catalog row says false for gpt-5.4
        assert!(!supports_temperature("gpt-5.4"));
        // non-catalog fallback for "gpt-5.9" also says false (gpt-5 family)
        assert!(!supports_temperature("gpt-5.9"));
        // non-catalog fallback for "gpt-4" says true
        assert!(supports_temperature("gpt-4"));
    }

    #[test]
    fn supports_reasoning_matches_catalog() {
        assert!(supports_reasoning("gpt-5.4"));
        assert!(supports_reasoning("gpt-5.9-future"));
        assert!(!supports_reasoning("gpt-4"));
    }

    #[test]
    fn fallback_pro_timeout_is_very_long() {
        let caps = fallback_caps("gpt-5.4-pro").unwrap();
        assert_eq!(caps.call_timeout_secs, Some(7200));
    }
}
