//! Per-model capability rows for Anthropic models.
//!
//! All values are cited against authoritative Anthropic primary sources:
//! `platform.claude.com/docs/en/about-claude/models/*` and the
//! feature pages (`effort`, `extended-thinking`, `adaptive-thinking`,
//! `context-windows`, `compaction`, `structured-outputs`).

use super::{BetaHeader, BetaValue, ModelCapabilities, ThinkingSupport};
use crate::model_profile::catalog::ModelTier;

// ── Beta headers ──────────────────────────────────────────────────────────

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

/// Headers advertised for models with adaptive thinking + compaction
/// (Opus 4.7, Opus 4.6, Sonnet 4.6).
const ADAPTIVE_COMPACTION_BETAS: &[BetaHeader] = &[
    BETA_COMPACTION,
    BETA_STRUCTURED_OUTPUT,
    BETA_INTERLEAVED_THINKING,
];

/// Headers advertised for models with interleaved thinking but no compaction
/// (Opus 4.5, Sonnet 4.5).
const LEGACY_THINKING_BETAS: &[BetaHeader] = &[BETA_STRUCTURED_OUTPUT, BETA_INTERLEAVED_THINKING];

/// Batch API extended-output beta (300k cap). Applies to Opus 4.7 / 4.6 /
/// Sonnet 4.6 per the official Models overview.
const BETA_OUTPUT_300K: BetaValue<u32> = BetaValue {
    header: "anthropic-beta: output-300k-2026-03-24",
    value: 300_000,
};

// ── Effort tiers ──────────────────────────────────────────────────────────

/// Effort tiers accepted by Opus 4.7 only; `xhigh` sits between `high` and `max`.
const OPUS_47_EFFORT: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// Effort tiers accepted by Opus 4.6 and Sonnet 4.6.
const CLAUDE_46_EFFORT: &[&str] = &["low", "medium", "high", "max"];

/// Effort tiers accepted by Opus 4.5 (verified against the live API:
/// "This model does not support effort level 'max'. Supported levels: high,
/// low, medium." — so `max` is rejected on 4.5 despite the docs listing the
/// model on the effort page.)
const OPUS_45_EFFORT: &[&str] = &["low", "medium", "high"];

// ── Catalog rows ─────────────────────────────────────────────────────────

/// Capability rows for Anthropic catalog models.
pub const CAPABILITIES: &[ModelCapabilities] = &[
    // Claude Opus 4.7
    //
    // Sources:
    //   - Models overview:
    //     https://platform.claude.com/docs/en/about-claude/models/overview
    //     (1M context, 128k sync output)
    //   - Migration guide:
    //     https://platform.claude.com/docs/en/about-claude/models/migration-guide
    //     (rejects non-default temperature/top_p/top_k; manual thinking returns 400)
    //   - Adaptive thinking:
    //     https://platform.claude.com/docs/en/build-with-claude/adaptive-thinking
    //     (adaptive-only; off by default unless thinking: {type: "adaptive"})
    //   - Effort:
    //     https://platform.claude.com/docs/en/build-with-claude/effort
    //     (supported, values low/medium/high/xhigh/max, xhigh is Opus-4.7-only, no beta header)
    //   - Compaction:
    //     https://platform.claude.com/docs/en/build-with-claude/compaction
    //     (supported; compact-2026-01-12 beta header)
    ModelCapabilities {
        id: "claude-opus-4-7",
        provider: "anthropic",
        display_name: "Claude Opus 4.7",
        tier: ModelTier::Recommended,
        model_family: "claude-opus-4",
        context_window: 1_000_000,
        max_output_tokens: 128_000,
        context_window_beta: None,
        max_output_tokens_beta: Some(BETA_OUTPUT_300K),
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::AnthropicAdaptiveOnly,
        supports_reasoning: false,
        effort_levels: OPUS_47_EFFORT,
        supports_web_search: true,
        supports_inference_geo: true,
        supports_compaction: true,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: ADAPTIVE_COMPACTION_BETAS,
        call_timeout_secs: Some(300),
    },
    // Claude Opus 4.6
    //
    // Sources:
    //   - Models overview:
    //     https://platform.claude.com/docs/en/about-claude/models/overview
    //     (1M context, 128k sync output, output-300k-2026-03-24 batch beta)
    //   - Extended thinking:
    //     https://platform.claude.com/docs/en/build-with-claude/extended-thinking
    //     (both adaptive and enabled accepted; enabled deprecated but functional)
    //   - Effort:
    //     https://platform.claude.com/docs/en/build-with-claude/effort
    //     (supported, low/medium/high/max; no xhigh)
    ModelCapabilities {
        id: "claude-opus-4-6",
        provider: "anthropic",
        display_name: "Claude Opus 4.6",
        tier: ModelTier::Supported,
        model_family: "claude-opus-4",
        context_window: 1_000_000,
        max_output_tokens: 128_000,
        context_window_beta: None,
        max_output_tokens_beta: Some(BETA_OUTPUT_300K),
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        supports_temperature: true,
        supports_top_p: true,
        supports_top_k: true,
        thinking: ThinkingSupport::AnthropicAdaptiveAndEnabled,
        supports_reasoning: false,
        effort_levels: CLAUDE_46_EFFORT,
        supports_web_search: true,
        supports_inference_geo: true,
        supports_compaction: true,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: true,
        beta_headers: ADAPTIVE_COMPACTION_BETAS,
        call_timeout_secs: Some(300),
    },
    // Claude Sonnet 4.6
    //
    // Sources:
    //   - Models overview:
    //     https://platform.claude.com/docs/en/about-claude/models/overview
    //     (1M context, 64k sync output, output-300k-2026-03-24 batch beta)
    //   - Extended thinking:
    //     https://platform.claude.com/docs/en/build-with-claude/extended-thinking
    //     (both adaptive and enabled accepted)
    //   - Effort:
    //     https://platform.claude.com/docs/en/build-with-claude/effort
    //     (supported, low/medium/high/max)
    //   - Compaction:
    //     https://platform.claude.com/docs/en/build-with-claude/compaction
    //     (supported via compact-2026-01-12)
    ModelCapabilities {
        id: "claude-sonnet-4-6",
        provider: "anthropic",
        display_name: "Claude Sonnet 4.6",
        tier: ModelTier::Recommended,
        model_family: "claude-sonnet-4",
        context_window: 1_000_000,
        max_output_tokens: 64_000,
        context_window_beta: None,
        max_output_tokens_beta: Some(BETA_OUTPUT_300K),
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        supports_temperature: true,
        supports_top_p: true,
        supports_top_k: true,
        thinking: ThinkingSupport::AnthropicAdaptiveAndEnabled,
        supports_reasoning: false,
        effort_levels: CLAUDE_46_EFFORT,
        supports_web_search: true,
        supports_inference_geo: true,
        supports_compaction: true,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: true,
        beta_headers: ADAPTIVE_COMPACTION_BETAS,
        call_timeout_secs: Some(120),
    },
    // Claude Sonnet 4.5
    //
    // Sources:
    //   - Context windows:
    //     https://platform.claude.com/docs/en/build-with-claude/context-windows
    //     (200k only; no 1M beta documented for Sonnet 4.5)
    //   - Models overview:
    //     https://platform.claude.com/docs/en/about-claude/models/overview
    //     (64k sync output)
    //   - Adaptive thinking:
    //     https://platform.claude.com/docs/en/build-with-claude/adaptive-thinking
    //     ("Older models (Sonnet 4.5, Opus 4.5) do not support adaptive thinking
    //      and require thinking.type: enabled with budget_tokens.")
    //   - Effort:
    //     https://platform.claude.com/docs/en/build-with-claude/effort
    //     (NOT in the supported-models list — Sonnet 4.5 does not support effort)
    //   - Structured outputs:
    //     https://platform.claude.com/docs/en/build-with-claude/structured-outputs
    //     (explicitly lists claude-sonnet-4-5)
    ModelCapabilities {
        id: "claude-sonnet-4-5",
        provider: "anthropic",
        display_name: "Claude Sonnet 4.5",
        tier: ModelTier::Supported,
        model_family: "claude-sonnet-4",
        context_window: 200_000,
        max_output_tokens: 64_000,
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        supports_temperature: true,
        supports_top_p: true,
        supports_top_k: true,
        thinking: ThinkingSupport::AnthropicEnabledOnly,
        supports_reasoning: false,
        effort_levels: &[],
        supports_web_search: true,
        supports_inference_geo: true,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: true,
        beta_headers: LEGACY_THINKING_BETAS,
        call_timeout_secs: Some(120),
    },
    // Claude Haiku 4.5
    //
    // Sources:
    //   - Models overview:
    //     https://platform.claude.com/docs/en/about-claude/models/overview
    //     (API ID claude-haiku-4-5-20251001, alias claude-haiku-4-5,
    //      200k context, 64k sync output, text/image input, text output)
    //   - Extended thinking:
    //     https://platform.claude.com/docs/en/build-with-claude/extended-thinking
    //     (manual thinking supported; Claude Haiku 4.5 supports up to 64k output)
    ModelCapabilities {
        id: "claude-haiku-4-5-20251001",
        provider: "anthropic",
        display_name: "Claude Haiku 4.5",
        tier: ModelTier::Recommended,
        model_family: "claude-haiku-4",
        context_window: 200_000,
        max_output_tokens: 64_000,
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        supports_temperature: true,
        supports_top_p: true,
        supports_top_k: true,
        thinking: ThinkingSupport::AnthropicEnabledOnly,
        supports_reasoning: false,
        effort_levels: &[],
        supports_web_search: true,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: true,
        beta_headers: LEGACY_THINKING_BETAS,
        call_timeout_secs: Some(60),
    },
    // Claude Haiku 4.5 alias
    //
    // Keep the official alias as a first-class catalog row so model/provider
    // resolution stays registry-owned instead of falling back to prefix folklore.
    ModelCapabilities {
        id: "claude-haiku-4-5",
        provider: "anthropic",
        display_name: "Claude Haiku 4.5",
        tier: ModelTier::Recommended,
        model_family: "claude-haiku-4",
        context_window: 200_000,
        max_output_tokens: 64_000,
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        supports_temperature: true,
        supports_top_p: true,
        supports_top_k: true,
        thinking: ThinkingSupport::AnthropicEnabledOnly,
        supports_reasoning: false,
        effort_levels: &[],
        supports_web_search: true,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: true,
        beta_headers: LEGACY_THINKING_BETAS,
        call_timeout_secs: Some(60),
    },
    // Claude Opus 4.5
    //
    // Sources:
    //   - Models overview:
    //     https://platform.claude.com/docs/en/about-claude/models/overview
    //     (200k context, 64k sync output)
    //   - Adaptive thinking:
    //     https://platform.claude.com/docs/en/build-with-claude/adaptive-thinking
    //     (Opus 4.5 requires manual enabled mode; adaptive not supported)
    //   - Effort:
    //     https://platform.claude.com/docs/en/build-with-claude/effort
    //     (supported — in the Mythos/Opus 4.7/Opus 4.6/Sonnet 4.6/Opus 4.5 list;
    //      values low/medium/high/max)
    //   - Compaction:
    //     https://platform.claude.com/docs/en/build-with-claude/compaction
    //     (NOT supported — Opus 4.5 is not in the supported-models list)
    ModelCapabilities {
        id: "claude-opus-4-5",
        provider: "anthropic",
        display_name: "Claude Opus 4.5",
        tier: ModelTier::Supported,
        model_family: "claude-opus-4",
        context_window: 200_000,
        max_output_tokens: 64_000,
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        supports_temperature: true,
        supports_top_p: true,
        supports_top_k: true,
        thinking: ThinkingSupport::AnthropicEnabledOnly,
        supports_reasoning: false,
        effort_levels: OPUS_45_EFFORT,
        supports_web_search: true,
        supports_inference_geo: true,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: true,
        beta_headers: LEGACY_THINKING_BETAS,
        call_timeout_secs: Some(300),
    },
];
