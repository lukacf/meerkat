//! Gemini family detection and legacy fallback for non-catalog model IDs.
//!
//! All capability data for catalog models lives in
//! [`crate::capabilities::gemini`]. This module retains only a minimal
//! family detection + [`fallback_caps`] helper for synthesizing a
//! [`ModelCapabilities`] for non-catalog model IDs (e.g., `gemini-3.1-flash-lite`).

use crate::capabilities::{ModelCapabilities, ThinkingSupport};
use crate::catalog::ModelTier;

fn detect_family(model: &str) -> Option<&'static str> {
    let m = model.to_ascii_lowercase();
    if m.starts_with("gemini-3") {
        Some("gemini-3")
    } else if m.starts_with("gemini-2") {
        Some("gemini-2")
    } else if m.starts_with("gemini-1") {
        Some("gemini-1")
    } else if m.starts_with("gemini-") {
        Some("gemini")
    } else {
        None
    }
}

/// Synthesize capabilities for a Gemini model ID that isn't in the catalog.
///
/// Matches the pre-refactor shape: single schema for all Gemini models
/// (`thinking`, `thinking_budget`, `top_k`, `top_p`), with thinking support
/// only for the `gemini-3*` family per the current heuristic.
pub fn fallback_caps(model: &str) -> Option<ModelCapabilities> {
    let family = detect_family(model)?;
    let m = model.to_ascii_lowercase();
    let is_flash = m.contains("flash");
    let call_timeout_secs = if is_flash {
        Some(120)
    } else {
        match family {
            "gemini-3" => Some(600),
            "gemini-2" => Some(180),
            "gemini-1" => Some(180),
            _ => None,
        }
    };
    let thinking = if family == "gemini-3" {
        ThinkingSupport::GeminiThinkingLevel
    } else {
        ThinkingSupport::None
    };
    Some(ModelCapabilities {
        id: "",
        provider: "gemini",
        display_name: "",
        tier: ModelTier::Supported,
        model_family: family,
        context_window: 1_000_000,
        max_output_tokens: 8_192,
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: true,
        supports_temperature: true,
        supports_top_p: true,
        supports_top_k: true,
        thinking,
        supports_reasoning: false,
        effort_levels: &[],
        supports_web_search: true,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: true,
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
        assert_eq!(detect_family("gemini-3-flash-preview"), Some("gemini-3"));
        assert_eq!(detect_family("gemini-3.1-pro-preview"), Some("gemini-3"));
        assert_eq!(detect_family("gemini-2.0-flash"), Some("gemini-2"));
        assert_eq!(detect_family("claude-opus-4-6"), None);
    }

    #[test]
    fn fallback_thinking_gated_on_gemini_3() {
        let caps = fallback_caps("gemini-3.1-flash-lite").unwrap();
        assert_eq!(caps.thinking, ThinkingSupport::GeminiThinkingLevel);
        let caps = fallback_caps("gemini-2.0-flash").unwrap();
        assert_eq!(caps.thinking, ThinkingSupport::None);
    }

    #[test]
    fn fallback_flash_timeout_short() {
        let caps = fallback_caps("gemini-3-flash-preview").unwrap();
        assert_eq!(caps.call_timeout_secs, Some(120));
        let caps = fallback_caps("gemini-3.1-pro-preview").unwrap();
        assert_eq!(caps.call_timeout_secs, Some(600));
    }
}
