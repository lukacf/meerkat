//! Per-model capability vocabulary — the typed shape of capability truth.
//!
//! Every catalog model has exactly one [`ModelCapabilities`] record describing
//! its accepted parameters, thinking modes, modalities, headers, and
//! operational defaults. `ModelProfile` is derived from this record;
//! `params_schema` is built from it by [`super::schema_builder`].
//!
//! This module owns only the *types*. The capability rows themselves are
//! provider data and live in the `meerkat-models` crate, injected into core
//! consumers through [`super::ModelCatalog`]. Lookups go through
//! [`super::ModelCatalog::capabilities_for`], which crosses the typed
//! [`crate::Provider`] boundary; uncatalogued model IDs do not receive
//! synthesized semantic capabilities.

use crate::Provider;
use crate::model_profile::catalog::ModelTier;

/// Full per-model capability record.
///
/// Fields group into:
/// - identity (`id`, `provider`, `display_name`, `tier`, `model_family`)
/// - context/output (`context_window`, `max_output_tokens`, plus `_beta` variants)
/// - modalities (`vision`, `image_tool_results`, `inline_video`, `realtime`,
///   `image_generation`)
/// - realtime transport capability facts (`realtime_supports_provider_managed_turns`,
///   `realtime_supports_explicit_commit`, `realtime_interrupt_supported`,
///   `realtime_transcript_supported`, `transcription_companion_model`) — what
///   the model's realtime bidirectional transport can actually do; only
///   meaningful when `realtime` is true
/// - sampling (`supports_temperature`, `supports_top_p`, `supports_top_k`)
/// - reasoning (`thinking`, `supports_reasoning`, `effort_levels`)
/// - features (`supports_web_search`, `supports_inference_geo`,
///   `supports_compaction`, `supports_structured_output`,
///   `supports_legacy_penalties`, `supports_thinking_budget_legacy`)
/// - metadata (`beta_headers`, `call_timeout_secs`)
#[derive(Debug, Clone, Copy)]
pub struct ModelCapabilities {
    // ── Identity ──────────────────────────────────────────────────────
    /// Model identifier.
    pub id: &'static str,
    /// Typed provider that owns this capability row.
    pub provider: Provider,
    /// Human-readable display name.
    pub display_name: &'static str,
    /// Recommendation tier.
    pub tier: ModelTier,
    /// Model family identifier (a stable grouping key for related model ids).
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
    /// Whether the model supports a realtime bidirectional streaming transport.
    pub realtime: bool,
    /// Realtime transport: whether the model's realtime session supports
    /// provider-managed turn detection (server VAD). Only meaningful when
    /// `realtime` is true; `false` on non-realtime rows.
    pub realtime_supports_provider_managed_turns: bool,
    /// Realtime transport: whether the model's realtime session supports
    /// explicit (client-driven) turn commit. Only meaningful when `realtime`
    /// is true; `false` on non-realtime rows.
    pub realtime_supports_explicit_commit: bool,
    /// Realtime transport: whether the model's realtime session supports
    /// interrupting (barge-in) the model's in-flight response. Only meaningful
    /// when `realtime` is true; `false` on non-realtime rows.
    pub realtime_interrupt_supported: bool,
    /// Realtime transport: whether the model's realtime session emits spoken
    /// input/output transcripts. Only meaningful when `realtime` is true;
    /// `false` on non-realtime rows.
    pub realtime_transcript_supported: bool,
    /// Realtime transport: the companion model id used for input audio
    /// transcription (ASR) on this model's realtime session. `None` on
    /// non-realtime rows (and on realtime rows with no companion).
    pub transcription_companion_model: Option<&'static str>,
    /// Whether this specific model can drive Meerkat image generation.
    ///
    /// This is a per-MODEL fact owned by the catalog row, not a per-provider
    /// derivation: a provider may have an image-generation default model while
    /// an individual text row on the same provider cannot itself generate
    /// images (it would require swapping to the provider's native image model).
    pub image_generation: bool,

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
    /// share the typed [`EffortLevel`] vocabulary.
    pub effort_levels: &'static [EffortLevel],

    // ── Features ──────────────────────────────────────────────────────
    /// Provider-native web search tool support.
    pub supports_web_search: bool,
    /// Anthropic data-residency hint via `inference_geo`.
    pub supports_inference_geo: bool,
    /// Anthropic `compaction` / `context_management` (with the compaction beta header).
    pub supports_compaction: bool,
    /// Structured output (`output_config.format` / `text.format` / `responseJsonSchema`).
    pub supports_structured_output: bool,
    /// OpenAI legacy penalty/seed params (`seed`, `frequency_penalty`, `presence_penalty`).
    pub supports_legacy_penalties: bool,
    /// Gemini legacy flat `thinking_budget` param (backward-compat alongside `thinking_level`).
    pub supports_thinking_budget_legacy: bool,

    // ── Beta headers ──────────────────────────────────────────────────
    /// Beta headers the client may set when interacting with this model.
    pub beta_headers: &'static [BetaHeader],

    // ── Runtime ───────────────────────────────────────────────────────
    /// Authoritative default call timeout in seconds for this model.
    /// `None` means the model has no profiled default (unknown family).
    pub call_timeout_secs: Option<u64>,
}

/// A capability value that is only available when a specific beta header is set.
#[derive(Debug, Clone, Copy)]
pub struct BetaValue<T: 'static> {
    /// Full header to send (`"<header-name>: <header-value>"` style).
    pub header: &'static str,
    /// The value enabled by the header (e.g. extended context window size).
    pub value: T,
}

/// Typed semantic feature gated by a beta header.
///
/// The single typed owner of the beta-feature value domain: catalog rows
/// declare which feature each [`BetaHeader`] gates via this enum, and request
/// shaping selects headers by typed feature — never by matching a raw string
/// label. The wire/display label is a derived projection
/// ([`BetaFeature::as_wire_str`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BetaFeature {
    /// Anthropic server-side compaction / context management.
    Compaction,
    /// Structured output (`output_config.format`).
    StructuredOutput,
    /// Interleaved thinking between tool calls.
    InterleavedThinking,
}

impl BetaFeature {
    /// Canonical wire/display label for this feature (derived projection).
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            BetaFeature::Compaction => "compaction",
            BetaFeature::StructuredOutput => "structured_output",
            BetaFeature::InterleavedThinking => "interleaved_thinking",
        }
    }
}

/// A beta header that gates a feature on this model.
#[derive(Debug, Clone, Copy)]
pub struct BetaHeader {
    /// Typed semantic feature this header gates.
    pub feature: BetaFeature,
    /// HTTP header name.
    pub header_name: &'static str,
    /// HTTP header value.
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

/// Reasoning/effort control level, the shared typed vocabulary behind both
/// Anthropic `output_config.effort` and OpenAI `reasoning.effort`.
///
/// The two providers expose effort in different request shapes but draw from
/// the same level vocabulary; modeling it as a typed enum keeps the catalog
/// value-domain compiler-checked instead of relying on raw string literals.
/// Each catalog row declares its accepted subset via `effort_levels`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EffortLevel {
    /// OpenAI `reasoning.effort: "none"` — reasoning disabled.
    None,
    /// OpenAI realtime `reasoning.effort: "minimal"`.
    Minimal,
    /// Lowest active reasoning effort.
    Low,
    /// Medium reasoning effort.
    Medium,
    /// High reasoning effort.
    High,
    /// Extended-high reasoning effort.
    Xhigh,
    /// Maximum reasoning effort (Anthropic-only top tier).
    Max,
}

impl EffortLevel {
    /// The wire string the provider APIs accept for this level.
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            EffortLevel::None => "none",
            EffortLevel::Minimal => "minimal",
            EffortLevel::Low => "low",
            EffortLevel::Medium => "medium",
            EffortLevel::High => "high",
            EffortLevel::Xhigh => "xhigh",
            EffortLevel::Max => "max",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beta_feature_wire_labels_are_pinned_projections() {
        // The semantic beta-feature domain is the typed enum; the wire label is
        // a derived projection. Pin the projection so the wire shape cannot
        // silently drift.
        assert_eq!(BetaFeature::Compaction.as_wire_str(), "compaction");
        assert_eq!(
            BetaFeature::StructuredOutput.as_wire_str(),
            "structured_output"
        );
        assert_eq!(
            BetaFeature::InterleavedThinking.as_wire_str(),
            "interleaved_thinking"
        );
    }

    #[test]
    fn effort_level_wire_values_are_pinned_projections() {
        assert_eq!(EffortLevel::None.as_wire_str(), "none");
        assert_eq!(EffortLevel::Minimal.as_wire_str(), "minimal");
        assert_eq!(EffortLevel::Low.as_wire_str(), "low");
        assert_eq!(EffortLevel::Medium.as_wire_str(), "medium");
        assert_eq!(EffortLevel::High.as_wire_str(), "high");
        assert_eq!(EffortLevel::Xhigh.as_wire_str(), "xhigh");
        assert_eq!(EffortLevel::Max.as_wire_str(), "max");
    }
}
