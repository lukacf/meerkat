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
//!
//! Public callers must cross the typed [`crate::Provider`] boundary before
//! reading capability truth; raw row iteration is intentionally crate-private:
//!
//! ```compile_fail
//! let _ = meerkat_core::model_profile::capabilities::all_capabilities();
//! ```
//!
//! ```compile_fail
//! let _ = meerkat_core::model_profile::capabilities::gemini::CAPABILITIES;
//! ```

mod anthropic;
mod gemini;
mod openai;

use crate::Provider;
use crate::model_profile::catalog::ModelTier;

/// Full per-model capability record.
///
/// Fields group into:
/// - identity (`id`, `provider`, `display_name`, `tier`, `model_family`)
/// - context/output (`context_window`, `max_output_tokens`, plus `_beta` variants)
/// - modalities (`vision`, `image_tool_results`, `inline_video`, `realtime`)
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
    /// Typed provider that owns this capability row.
    pub provider: Provider,
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
    /// Whether the model supports a realtime bidirectional streaming transport
    /// (e.g. OpenAI `*-realtime*` endpoints, Gemini `*-live*` endpoints).
    pub realtime: bool,

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
    /// share the same closed capability domain.
    pub effort_levels: &'static [ModelEffortLevel],

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
    /// Typed beta header that enables this value.
    pub header: BetaHeader,
    /// The value enabled by the header (e.g. extended context window size).
    pub value: T,
}

/// Closed effort domain owned by the model capability catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelEffortLevel {
    None,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl ModelEffortLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// Closed beta feature domain owned by the model capability catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelBetaFeature {
    Compaction,
    StructuredOutput,
    InterleavedThinking,
    ExtendedOutput300k,
}

impl ModelBetaFeature {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Compaction => "compaction",
            Self::StructuredOutput => "structured_output",
            Self::InterleavedThinking => "interleaved_thinking",
            Self::ExtendedOutput300k => "extended_output_300k",
        }
    }
}

/// A beta header that gates a feature on this model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BetaHeader {
    AnthropicCompaction20260112,
    AnthropicStructuredOutputs20251113,
    AnthropicInterleavedThinking20250514,
    AnthropicOutput300k20260324,
}

impl BetaHeader {
    pub const fn feature(self) -> ModelBetaFeature {
        match self {
            Self::AnthropicCompaction20260112 => ModelBetaFeature::Compaction,
            Self::AnthropicStructuredOutputs20251113 => ModelBetaFeature::StructuredOutput,
            Self::AnthropicInterleavedThinking20250514 => ModelBetaFeature::InterleavedThinking,
            Self::AnthropicOutput300k20260324 => ModelBetaFeature::ExtendedOutput300k,
        }
    }

    pub const fn header_name(self) -> &'static str {
        match self {
            Self::AnthropicCompaction20260112
            | Self::AnthropicStructuredOutputs20251113
            | Self::AnthropicInterleavedThinking20250514
            | Self::AnthropicOutput300k20260324 => "anthropic-beta",
        }
    }

    pub const fn header_value(self) -> &'static str {
        match self {
            Self::AnthropicCompaction20260112 => "compact-2026-01-12",
            Self::AnthropicStructuredOutputs20251113 => "structured-outputs-2025-11-13",
            Self::AnthropicInterleavedThinking20250514 => "interleaved-thinking-2025-05-14",
            Self::AnthropicOutput300k20260324 => "output-300k-2026-03-24",
        }
    }
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

/// Lookup a model's capabilities by typed provider + id.
///
/// Returns `None` when the provider/model pair has no capability row.
/// Callers must treat `None` as unknown capability truth; uncatalogued model
/// IDs must not synthesize semantic facts from model-name folklore.
///
/// Provider/display strings are intentionally rejected at compile time:
///
/// ```compile_fail
/// let _ = meerkat_core::model_profile::capabilities::capabilities_for(
///     "gemini",
///     "gemini-3-flash-preview",
/// );
/// ```
pub fn capabilities_for(provider: Provider, model_id: &str) -> Option<&'static ModelCapabilities> {
    let table: &'static [ModelCapabilities] = match provider {
        Provider::Anthropic => anthropic::CAPABILITIES,
        Provider::OpenAI => openai::CAPABILITIES,
        Provider::Gemini => gemini::CAPABILITIES,
        _ => return None,
    };
    table.iter().find(|c| c.id == model_id)
}

/// Iterate every known capability record across all providers.
///
/// This is crate-internal so public callers cannot obtain semantic capability
/// rows through provider/model string filtering or model-only row scans.
pub(crate) fn all_capabilities() -> impl Iterator<Item = &'static ModelCapabilities> {
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
            let entry = crate::model_profile::catalog::entry_for(caps.provider, caps.id);
            assert!(
                entry.is_some(),
                "capability row '{}' (provider '{}') has no catalog entry",
                caps.id,
                caps.provider.as_str(),
            );
        }
    }

    #[test]
    fn no_duplicate_capability_ids_within_provider() {
        for &provider in crate::model_profile::catalog::providers() {
            let ids: Vec<&str> = all_capabilities()
                .filter(|c| c.provider == provider)
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
        for entry in crate::model_profile::catalog::catalog() {
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
            let entry = crate::model_profile::catalog::entry_for(caps.provider, caps.id)
                .unwrap_or_else(|| panic!("missing catalog entry for {}", caps.id));
            assert_eq!(caps.tier, entry.tier, "tier mismatch for {}", caps.id);
        }
    }

    #[test]
    fn claude_haiku_45_is_cataloged_with_official_limits() {
        for model in ["claude-haiku-4-5-20251001", "claude-haiku-4-5"] {
            let caps = capabilities_for(Provider::Anthropic, model)
                .unwrap_or_else(|| panic!("{model} must be in the Anthropic catalog"));
            assert_eq!(caps.model_family, "claude-haiku-4");
            assert_eq!(caps.context_window, 200_000);
            assert_eq!(caps.max_output_tokens, 64_000);
            assert_eq!(caps.thinking, ThinkingSupport::AnthropicEnabledOnly);
            assert!(!caps.supports_compaction);
        }
    }

    #[test]
    fn typed_provider_mismatch_fails_closed() {
        assert!(capabilities_for(Provider::Anthropic, "gpt-5.4").is_none());
        assert!(capabilities_for(Provider::OpenAI, "gemini-3-flash-preview").is_none());
        assert!(capabilities_for(Provider::Other, "gpt-5.4").is_none());
    }

    #[test]
    fn display_provider_string_cannot_be_promoted_to_capability_owner() {
        let display_provider = Provider::parse_strict("Gemini").unwrap_or(Provider::Other);
        assert_eq!(display_provider, Provider::Other);
        assert!(
            capabilities_for(display_provider, "gemini-3-flash-preview").is_none(),
            "display provider strings must fail closed at the typed capability boundary"
        );
    }

    #[test]
    fn capability_value_domains_are_typed_before_projection() {
        let opus = capabilities_for(Provider::Anthropic, "claude-opus-4-7")
            .expect("opus 4.7 capability row");
        assert_eq!(
            opus.effort_levels,
            &[
                ModelEffortLevel::Low,
                ModelEffortLevel::Medium,
                ModelEffortLevel::High,
                ModelEffortLevel::XHigh,
                ModelEffortLevel::Max,
            ]
        );
        assert!(
            opus.beta_headers
                .iter()
                .all(|header| header.header_name() == "anthropic-beta")
        );
        assert!(
            opus.beta_headers
                .iter()
                .any(|header| header.feature() == ModelBetaFeature::Compaction)
        );
        assert_eq!(
            opus.max_output_tokens_beta.expect("300k beta").header,
            BetaHeader::AnthropicOutput300k20260324
        );
    }
}
