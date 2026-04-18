//! Per-model capability table — the authoritative, data-driven source of truth.
//!
//! Every catalog model has exactly one `ModelCapabilities` record describing its
//! accepted parameters, thinking modes, modalities, headers, and operational
//! defaults. `ModelProfile` is derived from this record; `params_schema` is
//! built from it by [`super::profile::schema_builder`].
//!
//! This replaces the previous heuristic-driven approach (struct-per-bucket with
//! `schemars::schema_for!`). The old structs advertised capabilities across
//! broad model groups (e.g. "opus 4.6 bucket" served both Opus 4.6 and 4.7;
//! "standard" served everything else), flattening real differences such as
//! Opus 4.7's `xhigh` effort tier or Sonnet 4.5's 1M context beta.

pub mod anthropic;
pub mod gemini;
pub mod openai;

use crate::catalog::ModelTier;

/// Full per-model capability record.
///
/// Fields group into:
/// - identity (`id`, `provider`, `display_name`, `tier`, `model_family`)
/// - context/output (`context_window`, `max_output_tokens`, plus `_beta` variants)
/// - modalities (`vision`, `image_tool_results`, `inline_video`)
/// - sampling (`supports_temperature`, `supports_top_p`, `supports_top_k`)
/// - reasoning (`thinking`, `supports_reasoning`, `effort_levels`)
/// - features (`supports_web_search`, `supports_inference_geo`,
///   `supports_compaction`, `supports_structured_output`,
///   `supports_legacy_penalties`, `supports_thinking_budget_legacy`)
/// - metadata (`beta_headers`, `call_timeout_secs`)
#[derive(Debug, Clone, Copy)]
pub struct ModelCapabilities {
    // ── Identity ──────────────────────────────────────────────────────
    /// Model identifier (e.g. `"claude-opus-4-7"`).
    pub id: &'static str,
    /// Canonical provider string (`"anthropic"`, `"openai"`, `"gemini"`).
    pub provider: &'static str,
    /// Human-readable display name.
    pub display_name: &'static str,
    /// Recommendation tier.
    pub tier: ModelTier,
    /// Model family identifier (e.g. `"claude-opus-4"`, `"gpt-5"`, `"gemini-3"`).
    pub model_family: &'static str,

    // ── Context / output ──────────────────────────────────────────────
    /// Maximum input context window in tokens (default, no beta).
    pub context_window: u32,
    /// Maximum output tokens per response (default, no beta).
    pub max_output_tokens: u32,
    /// Extended context window via beta header, if available.
    pub context_window_beta: Option<BetaValue<u32>>,
    /// Extended max output tokens via beta header, if available.
    pub max_output_tokens_beta: Option<BetaValue<u32>>,

    // ── Modalities ────────────────────────────────────────────────────
    /// Whether the model accepts image content in user messages.
    pub vision: bool,
    /// Whether the model can process image blocks in tool results.
    pub image_tool_results: bool,
    /// Whether the model accepts inline video content in user messages.
    pub inline_video: bool,

    // ── Sampling ──────────────────────────────────────────────────────
    /// Whether the API accepts a non-default `temperature` on this model.
    pub supports_temperature: bool,
    /// Whether the API accepts `top_p`.
    pub supports_top_p: bool,
    /// Whether the API accepts `top_k`.
    pub supports_top_k: bool,

    // ── Reasoning ─────────────────────────────────────────────────────
    /// Thinking/reasoning support mode, provider-specific.
    pub thinking: ThinkingSupport,
    /// Whether the model supports explicit reasoning effort control
    /// (OpenAI's `reasoning.effort`, distinct from Anthropic `output_config.effort`).
    pub supports_reasoning: bool,
    /// Accepted values for effort control. Empty slice = unsupported.
    /// Applies to both Anthropic `output_config.effort` and OpenAI
    /// `reasoning.effort`; the two schemas live in different request shapes but
    /// share the enum here since the levels are all strings.
    pub effort_levels: &'static [&'static str],

    // ── Features ──────────────────────────────────────────────────────
    /// Provider-native web search tool support.
    pub supports_web_search: bool,
    /// Anthropic data-residency hint via `inference_geo`.
    pub supports_inference_geo: bool,
    /// Anthropic `compaction` / `context_management` (with `compact-2026-01-12` beta header).
    pub supports_compaction: bool,
    /// Structured output (`output_config.format` / `text.format` / `responseJsonSchema`).
    pub supports_structured_output: bool,
    /// OpenAI legacy penalty/seed params (`seed`, `frequency_penalty`, `presence_penalty`).
    pub supports_legacy_penalties: bool,
    /// Gemini legacy flat `thinking_budget` param (backward-compat alongside `thinking_level`).
    pub supports_thinking_budget_legacy: bool,

    // ── Beta headers ──────────────────────────────────────────────────
    /// Beta headers the client may set when interacting with this model.
    /// Captured here so the wire layer can surface them in a later PR.
    pub beta_headers: &'static [BetaHeader],

    // ── Runtime ───────────────────────────────────────────────────────
    /// Authoritative default call timeout in seconds for this model.
    /// `None` means the model has no profiled default (unknown family).
    pub call_timeout_secs: Option<u64>,
}

/// A capability value that is only available when a specific beta header is set.
#[derive(Debug, Clone, Copy)]
pub struct BetaValue<T: 'static> {
    /// Full header to send (`"anthropic-beta: context-1m-2025-08-07"` style).
    pub header: &'static str,
    /// The value enabled by the header (e.g. extended context window size).
    pub value: T,
}

/// A beta header that gates a feature on this model.
#[derive(Debug, Clone, Copy)]
pub struct BetaHeader {
    /// Short feature identifier (e.g. `"compaction"`).
    pub feature: &'static str,
    /// HTTP header name (usually `"anthropic-beta"`).
    pub header_name: &'static str,
    /// HTTP header value (e.g. `"compact-2026-01-12"`).
    pub header_value: &'static str,
}

/// Thinking/reasoning support mode, provider-specific because the wire shapes differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingSupport {
    /// Model does not expose a thinking/reasoning configuration.
    None,
    /// Anthropic: only `thinking: {type: "enabled", budget_tokens: N}` is accepted.
    AnthropicEnabledOnly,
    /// Anthropic: only `thinking: {type: "adaptive"}` is accepted; enabled mode returns 400.
    AnthropicAdaptiveOnly,
    /// Anthropic: both `{type: "adaptive"}` and `{type: "enabled", budget_tokens: N}` are accepted.
    AnthropicAdaptiveAndEnabled,
    /// Gemini 3.x: `generationConfig.thinkingConfig.thinking_level`.
    /// Legacy `thinking_budget` is also accepted when
    /// `supports_thinking_budget_legacy = true`.
    GeminiThinkingLevel,
}

/// Lookup a model's capabilities by provider + id.
///
/// Returns `None` when the provider/model pair has no capability row.
/// Callers that still need a `ModelProfile` should route through
/// [`crate::profile::profile_for`], which handles legacy fallback for
/// models without a capability row.
pub fn capabilities_for(provider: &str, model_id: &str) -> Option<&'static ModelCapabilities> {
    let table: &'static [ModelCapabilities] = match provider {
        "anthropic" => anthropic::CAPABILITIES,
        "openai" => openai::CAPABILITIES,
        "gemini" => gemini::CAPABILITIES,
        _ => return None,
    };
    table.iter().find(|c| c.id == model_id)
}

/// Iterate every known capability record across all providers.
pub fn all_capabilities() -> impl Iterator<Item = &'static ModelCapabilities> {
    anthropic::CAPABILITIES
        .iter()
        .chain(openai::CAPABILITIES.iter())
        .chain(gemini::CAPABILITIES.iter())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn every_capability_matches_a_catalog_entry() {
        for caps in all_capabilities() {
            let entry = crate::catalog::entry_for(caps.provider, caps.id);
            assert!(
                entry.is_some(),
                "capability row '{}' (provider '{}') has no catalog entry",
                caps.id,
                caps.provider,
            );
        }
    }

    #[test]
    fn no_duplicate_capability_ids_within_provider() {
        for provider in crate::catalog::provider_names() {
            let ids: Vec<&str> = all_capabilities()
                .filter(|c| c.provider == *provider)
                .map(|c| c.id)
                .collect();
            let mut unique: Vec<&str> = ids.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(ids.len(), unique.len(), "duplicate ids in {provider}");
        }
    }

    #[test]
    fn every_catalog_entry_has_capabilities() {
        for entry in crate::catalog::catalog() {
            let caps = capabilities_for(entry.provider, entry.id);
            assert!(
                caps.is_some(),
                "catalog model '{}' (provider '{}') has no capability row",
                entry.id,
                entry.provider,
            );
        }
    }

    #[test]
    fn tier_matches_catalog_entry() {
        for caps in all_capabilities() {
            let entry = crate::catalog::entry_for(caps.provider, caps.id)
                .unwrap_or_else(|| panic!("missing catalog entry for {}", caps.id));
            assert_eq!(caps.tier, entry.tier, "tier mismatch for {}", caps.id);
        }
    }
}
