//! Per-model capability rows for Anthropic models.
//!
//! All values are cited against authoritative Anthropic primary sources:
//! `platform.claude.com/docs/en/about-claude/models/*` and the
//! feature pages (`effort`, `extended-thinking`, `adaptive-thinking`,
//! `context-windows`, `compaction`, `structured-outputs`).

use meerkat_core::Provider;
use meerkat_core::model_profile::capabilities::{
    AnthropicDisabledThinking, BetaFeature, BetaHeader, BetaValue, EffortLevel, ModelCapabilities,
    ThinkingSupport,
};
use meerkat_core::model_profile::catalog::{ModelReleaseStage, ModelTier};

// ── Beta headers ──────────────────────────────────────────────────────────

const BETA_COMPACTION: BetaHeader = BetaHeader {
    feature: BetaFeature::Compaction,
    header_name: "anthropic-beta",
    header_value: "compact-2026-01-12",
};

const BETA_STRUCTURED_OUTPUT: BetaHeader = BetaHeader {
    feature: BetaFeature::StructuredOutput,
    header_name: "anthropic-beta",
    header_value: "structured-outputs-2025-11-13",
};

const BETA_INTERLEAVED_THINKING: BetaHeader = BetaHeader {
    feature: BetaFeature::InterleavedThinking,
    header_name: "anthropic-beta",
    header_value: "interleaved-thinking-2025-05-14",
};

/// Thinking-block binding controls: `thinking.block_binding` and the
/// `input_transformations` report. The catalog rows that list the beta carry
/// this header; a client sends the same one for a model id with no catalog
/// row, so the value is defined here once.
/// https://platform.claude.com/docs/en/build-with-claude/preserved-thinking
pub const ANTHROPIC_THINKING_BINDING_CONTROLS_BETA: BetaHeader = BetaHeader {
    feature: BetaFeature::ThinkingBindingControls,
    header_name: "anthropic-beta",
    header_value: "thinking-binding-controls-2026-08-01",
};

/// Headers advertised for models with adaptive thinking + compaction
/// (Fable 5, Opus 5, Opus 4.8, Opus 4.7, Sonnet 5, Sonnet 4.6).
const ADAPTIVE_COMPACTION_BETAS: &[BetaHeader] = &[
    BETA_COMPACTION,
    BETA_STRUCTURED_OUTPUT,
    BETA_INTERLEAVED_THINKING,
];

/// [`ADAPTIVE_COMPACTION_BETAS`] plus the thinking-binding controls, for the
/// catalogued models that check replayed thinking blocks against their
/// request prefix (Fable 5.1, Opus 5.5, Sonnet 5.5, Haiku 5.5; Mythos 5.1 is
/// not a catalog row). Each of them thinks by default when `thinking` is
/// omitted: Fable 5.1 and Opus 5.5 "Always on", Sonnet 5.5 and Haiku 5.5
/// "On" in the per-model table, and "Omit the `thinking` parameter and the
/// request runs with adaptive thinking"; the client names that default beside
/// the control. A model whose default is no thinking never carries this set.
/// Sources:
///   https://platform.claude.com/docs/en/build-with-claude/preserved-thinking
///   (featureMetadata.supportedModels)
///   https://platform.claude.com/docs/en/build-with-claude/thinking-troubleshooting
///   (Thinking support, defaults, and rejected configurations by model)
///   https://platform.claude.com/docs/en/api/errors (Thinking cannot be disabled)
const PRESERVED_THINKING_BETAS: &[BetaHeader] = &[
    BETA_COMPACTION,
    BETA_STRUCTURED_OUTPUT,
    BETA_INTERLEAVED_THINKING,
    ANTHROPIC_THINKING_BINDING_CONTROLS_BETA,
];

/// Headers advertised for models with interleaved thinking but no compaction
/// (Opus 4.5, Sonnet 4.5).
const LEGACY_THINKING_BETAS: &[BetaHeader] = &[BETA_STRUCTURED_OUTPUT, BETA_INTERLEAVED_THINKING];

/// Batch API extended-output beta (300k cap). Applies to Opus 4.8 / 4.7 /
/// Sonnet 5.5 / Sonnet 5 / Sonnet 4.6 per the official Models overview, and
/// to Haiku 5.5 per its model page.
const BETA_OUTPUT_300K: BetaValue<u32> = BetaValue {
    header: "anthropic-beta: output-300k-2026-03-24",
    value: 300_000,
};

// ── Effort tiers ──────────────────────────────────────────────────────────

/// Effort tiers accepted by the Claude 5 family (Fable 5.1, Opus 5.5, Opus 5,
/// Fable 5, Sonnet 5.5, Sonnet 5, Haiku 5.5) and Opus 4.8/4.7; `xhigh` sits
/// between `high` and `max`.
const CLAUDE_5_OPUS_48_47_EFFORT: &[EffortLevel] = &[
    EffortLevel::Low,
    EffortLevel::Medium,
    EffortLevel::High,
    EffortLevel::Xhigh,
    EffortLevel::Max,
];

/// Effort tiers accepted by Sonnet 4.6.
const SONNET_46_EFFORT: &[EffortLevel] = &[
    EffortLevel::Low,
    EffortLevel::Medium,
    EffortLevel::High,
    EffortLevel::Max,
];

/// Effort tiers accepted by Opus 4.5 (verified against the live API:
/// "This model does not support effort level 'max'. Supported levels: high,
/// low, medium." — so `max` is rejected on 4.5 despite the docs listing the
/// model on the effort page.)
const OPUS_45_EFFORT: &[EffortLevel] = &[EffortLevel::Low, EffortLevel::Medium, EffortLevel::High];

// ── Catalog rows ─────────────────────────────────────────────────────────

// Disabled thinking: `anthropic_disabled_thinking` on every row follows the
// per-model table "Thinking support, defaults, and rejected configurations by
// model" (a thinking type a row does not list as rejected is accepted):
// https://platform.claude.com/docs/en/build-with-claude/thinking-troubleshooting
//   - Refused: Fable 5.1, Fable 5 and Opus 5.5 (thinking is always on), and
//     Sonnet 5.5 (`between_tools` replaces `disabled`).
//   - AtOrBelowHighEffort: Opus 5 and Haiku 5.5 (`disabled` with effort
//     `xhigh` or `max` returns 400; their default efforts, `high` and
//     `medium`, are within that ceiling).
//   - AnyEffort: Opus 4.8, Opus 4.7, Sonnet 5, Sonnet 4.6, Sonnet 4.5,
//     Haiku 4.5 and Opus 4.5.

/// Capability rows for Anthropic catalog models.
pub const CAPABILITIES: &[ModelCapabilities] = &[
    // Claude Fable 5.1
    //
    // Sources:
    //   - Model page:
    //     https://platform.claude.com/docs/en/models/fable-5-1/overview
    //     (API ID claude-fable-5-1; 1M context; 128k max output; vision;
    //      adaptive thinking always on; GA beginning September 1, 2026)
    //   - What's new:
    //     https://platform.claude.com/docs/en/models/fable-5-1/whats-new-fable-5-1
    //     (successor to Fable 5 with the same core capability surface)
    //   - Effort:
    //     https://platform.claude.com/docs/en/build-with-claude/effort
    //     (low/medium/high/xhigh/max)
    //   - Compaction:
    //     https://platform.claude.com/docs/en/build-with-claude/compaction
    //   - Structured outputs:
    //     https://platform.claude.com/docs/en/build-with-claude/structured-outputs
    //   - Forced tool use ("Claude Opus 5.5, Claude Sonnet 5.5, Claude Fable
    //     5.1, and Claude Mythos 5.1 don't support forced tool use": a
    //     `tool_choice` of `any` or a named tool returns 400):
    //     https://platform.claude.com/docs/en/api/errors
    //     https://platform.claude.com/docs/en/agents-and-tools/tool-use/define-tools
    //
    // ASSUMED from the claude-fable-5 row (family-conservative defaults):
    //   - image_tool_results
    //   - supports_inference_geo
    //   - call_timeout_secs (Meerkat-owned operational default, Opus-tier 300s)
    ModelCapabilities {
        id: "claude-fable-5-1",
        provider: Provider::Anthropic,
        display_name: "Claude Fable 5.1",
        tier: ModelTier::Recommended,
        release_stage: ModelReleaseStage::Stable,
        model_family: "claude-fable-5-1",
        context_window: Some(1_000_000),
        max_input_tokens: None,
        max_output_tokens: Some(128_000),
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        realtime_supports_provider_managed_turns: false,
        realtime_supports_explicit_commit: false,
        realtime_interrupt_supported: false,
        realtime_transcript_supported: false,
        transcription_companion_model: None,
        image_generation: false,
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::AnthropicAdaptiveOnly,
        anthropic_disabled_thinking: AnthropicDisabledThinking::Refused,
        supports_reasoning: false,
        effort_levels: CLAUDE_5_OPUS_48_47_EFFORT,
        openai_responses_params: None,
        supports_web_search: true,
        supports_mid_conversation_system_messages: true,
        supports_inference_geo: true,
        supports_compaction: true,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: PRESERVED_THINKING_BETAS,
        call_timeout_secs: Some(300),
        stream_inactivity_timeout_secs: None,
        // Forced `tool_choice` (`any`/`tool`) returns 400 on Fable 5.1 (see
        // the forced tool use sources above); refuse it locally, as for
        // claude-opus-5-5 and claude-sonnet-5-5.
        supports_forced_tool_choice: false,
    },
    // Claude Fable 5
    //
    // Sources (verified):
    //   - Models overview:
    //     https://platform.claude.com/docs/en/about-claude/models/overview
    //     (API ID claude-fable-5; 1M context; 128k max output; vision/text+image
    //      input; adaptive thinking only — extended thinking "No"; GA on the
    //      Claude API beginning June 9, 2026. Fable 5 is NOT listed in the
    //      output-300k-2026-03-24 batch-beta note, hence no extended-output
    //      beta on this row.)
    //   - Introducing Claude Fable 5 and Claude Mythos 5:
    //     https://platform.claude.com/docs/en/about-claude/models/introducing-claude-fable-5-and-claude-mythos-5
    //     (adaptive thinking is the only mode and applies whenever `thinking`
    //      is unset; an explicit thinking: {"type": "disabled"} is NOT
    //      supported — unlike Opus 4.8/4.7, which accept it. Meerkat's
    //      catalog row records it as `anthropic_disabled_thinking: Refused`
    //      and refuses it locally. Supported at launch: effort, compaction,
    //      vision, memory tool. Refusals surface as stop_reason: "refusal".)
    //   - Effort:
    //     https://platform.claude.com/docs/en/build-with-claude/effort
    //     (supported; low/medium/high/xhigh/max — same ladder as Opus 4.8/4.7)
    //   - Compaction:
    //     https://platform.claude.com/docs/en/build-with-claude/compaction
    //     (supported; compact-2026-01-12 beta header)
    //   - Structured outputs:
    //     https://platform.claude.com/docs/en/build-with-claude/structured-outputs
    //     (supported on Claude Fable 5)
    //   - Web search:
    //     https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool
    //     (web_search_20260209 with dynamic filtering lists Fable 5)
    //   - Sampling: temperature/top_p/top_k removed (400), same request
    //     surface as Opus 4.8/4.7 per the migration guide; budget_tokens
    //     fully removed, so no legacy thinking_budget either.
    //
    // ASSUMED from the claude-opus-4-8 row (family-conservative defaults; the
    // launch docs do not state these facts either way):
    //   - image_tool_results (image blocks inside tool results)
    //   - supports_inference_geo (data-residency hint)
    //   - call_timeout_secs (Meerkat-owned operational default, Opus-tier 300s)
    ModelCapabilities {
        id: "claude-fable-5",
        provider: Provider::Anthropic,
        display_name: "Claude Fable 5",
        tier: ModelTier::Recommended,
        release_stage: ModelReleaseStage::Stable,
        model_family: "claude-fable-5",
        context_window: Some(1_000_000),
        max_input_tokens: None,
        max_output_tokens: Some(128_000),
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        realtime_supports_provider_managed_turns: false,
        realtime_supports_explicit_commit: false,
        realtime_interrupt_supported: false,
        realtime_transcript_supported: false,
        transcription_companion_model: None,
        image_generation: false,
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::AnthropicAdaptiveOnly,
        anthropic_disabled_thinking: AnthropicDisabledThinking::Refused,
        supports_reasoning: false,
        effort_levels: CLAUDE_5_OPUS_48_47_EFFORT,
        openai_responses_params: None,
        supports_web_search: true,
        supports_mid_conversation_system_messages: true,
        supports_inference_geo: true,
        supports_compaction: true,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: ADAPTIVE_COMPACTION_BETAS,
        call_timeout_secs: Some(300),
        stream_inactivity_timeout_secs: None,
        supports_forced_tool_choice: true,
    },
    // Claude Opus 5.5
    //
    // Sources:
    //   - Model overview (API ID claude-opus-5-5, 1M context, 128k output,
    //     $4 / $20 per MTok, cache reads $0.20; same tokenizer and feature
    //     surface as Claude Opus 5):
    //     https://platform.claude.com/docs/en/about-claude/models/overview
    //   - What's new (thinking cannot be disabled: `{type: "disabled"}` and
    //     `budget_tokens` return 400 at every effort level; effort default is
    //     `medium`; forced `tool_choice` `any`/`tool` returns 400; computer
    //     use only through `computer_toolset_20260801`):
    //     https://platform.claude.com/docs/en/models/opus-5-5/whats-new-opus-5-5
    //   - Effort: https://platform.claude.com/docs/en/build-with-claude/effort
    //     (low/medium/high/xhigh/max)
    //
    // The row mirrors claude-opus-5 except for disabled thinking, which it
    // refuses at every effort (Opus 5 accepts it at `high` or below), so
    // Meerkat refuses it locally: adaptive-only thinking, the full
    // low..max effort ladder, compaction +
    // structured outputs + web search, sampling parameters removed,
    // budget_tokens removed. Meerkat-owned operational default: Opus-tier
    // 300s call timeout. Default Anthropic model since 0.8.42.
    ModelCapabilities {
        id: "claude-opus-5-5",
        provider: Provider::Anthropic,
        display_name: "Claude Opus 5.5",
        tier: ModelTier::Recommended,
        release_stage: ModelReleaseStage::Stable,
        model_family: "claude-opus-5",
        context_window: Some(1_000_000),
        max_input_tokens: None,
        max_output_tokens: Some(128_000),
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        realtime_supports_provider_managed_turns: false,
        realtime_supports_explicit_commit: false,
        realtime_interrupt_supported: false,
        realtime_transcript_supported: false,
        transcription_companion_model: None,
        image_generation: false,
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::AnthropicAdaptiveOnly,
        anthropic_disabled_thinking: AnthropicDisabledThinking::Refused,
        supports_reasoning: false,
        effort_levels: CLAUDE_5_OPUS_48_47_EFFORT,
        openai_responses_params: None,
        supports_web_search: true,
        supports_mid_conversation_system_messages: true,
        supports_inference_geo: true,
        supports_compaction: true,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: PRESERVED_THINKING_BETAS,
        call_timeout_secs: Some(300),
        stream_inactivity_timeout_secs: None,
        // Live probe 2026-10-01: tool_choice any/tool answers 400 "not
        // supported for this model" with and without thinking.
        supports_forced_tool_choice: false,
    },
    // Claude Opus 5
    //
    // Claude 5 family sibling of Fable 5 (API ID claude-opus-5, announced
    // alongside Fable 5/Mythos 5:
    // https://www.anthropic.com/news/claude-fable-5-mythos-5). The row
    // mirrors claude-fable-5 — same generation, same request surface: 1M
    // context, 128k max output, vision, adaptive-only thinking, the full
    // low..max effort ladder, compaction + structured outputs + web search,
    // sampling parameters removed, budget_tokens removed. Like Fable 5 it is
    // not listed in the output-300k-2026-03-24 batch-beta note, hence no
    // extended-output beta. Meerkat-owned operational default: Opus-tier
    // 300s call timeout. Unlike Fable 5 it accepts `{type: "disabled"}`, at
    // `high` effort or below (see the disabled-thinking note above
    // CAPABILITIES).
    ModelCapabilities {
        id: "claude-opus-5",
        provider: Provider::Anthropic,
        display_name: "Claude Opus 5",
        tier: ModelTier::Recommended,
        release_stage: ModelReleaseStage::Stable,
        model_family: "claude-opus-5",
        context_window: Some(1_000_000),
        max_input_tokens: None,
        max_output_tokens: Some(128_000),
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        realtime_supports_provider_managed_turns: false,
        realtime_supports_explicit_commit: false,
        realtime_interrupt_supported: false,
        realtime_transcript_supported: false,
        transcription_companion_model: None,
        image_generation: false,
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::AnthropicAdaptiveOnly,
        anthropic_disabled_thinking: AnthropicDisabledThinking::AtOrBelowHighEffort,
        supports_reasoning: false,
        effort_levels: CLAUDE_5_OPUS_48_47_EFFORT,
        openai_responses_params: None,
        supports_web_search: true,
        supports_mid_conversation_system_messages: true,
        supports_inference_geo: true,
        supports_compaction: true,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: ADAPTIVE_COMPACTION_BETAS,
        call_timeout_secs: Some(300),
        stream_inactivity_timeout_secs: None,
        supports_forced_tool_choice: true,
    },
    // Claude Opus 4.8
    //
    // Sources:
    //   - Models overview:
    //     https://platform.claude.com/docs/en/about-claude/models/overview
    //     (1M context, 128k sync output, output-300k-2026-03-24 batch beta)
    //   - What's new in Claude Opus 4.8:
    //     https://platform.claude.com/docs/en/about-claude/models/whats-new-claude-4-8
    //     (same API constraints as Opus 4.7; adaptive thinking; fast mode;
    //      effort default high)
    //   - Adaptive thinking:
    //     https://platform.claude.com/docs/en/build-with-claude/adaptive-thinking
    //     (adaptive-only; off by default unless thinking: {type: "adaptive"})
    //   - Effort:
    //     https://platform.claude.com/docs/en/build-with-claude/effort
    //     (supported, values low/medium/high/xhigh/max, xhigh is Opus 4.8/4.7 only)
    //   - Compaction:
    //     https://platform.claude.com/docs/en/build-with-claude/compaction
    //     (supported; compact-2026-01-12 beta header)
    ModelCapabilities {
        id: "claude-opus-4-8",
        provider: Provider::Anthropic,
        display_name: "Claude Opus 4.8",
        tier: ModelTier::Recommended,
        release_stage: ModelReleaseStage::Stable,
        model_family: "claude-opus-4",
        context_window: Some(1_000_000),
        max_input_tokens: None,
        max_output_tokens: Some(128_000),
        context_window_beta: None,
        max_output_tokens_beta: Some(BETA_OUTPUT_300K),
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        realtime_supports_provider_managed_turns: false,
        realtime_supports_explicit_commit: false,
        realtime_interrupt_supported: false,
        realtime_transcript_supported: false,
        transcription_companion_model: None,
        image_generation: false,
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::AnthropicAdaptiveOnly,
        anthropic_disabled_thinking: AnthropicDisabledThinking::AnyEffort,
        supports_reasoning: false,
        effort_levels: CLAUDE_5_OPUS_48_47_EFFORT,
        openai_responses_params: None,
        supports_web_search: true,
        supports_mid_conversation_system_messages: true,
        supports_inference_geo: true,
        supports_compaction: true,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: ADAPTIVE_COMPACTION_BETAS,
        call_timeout_secs: Some(300),
        stream_inactivity_timeout_secs: None,
        supports_forced_tool_choice: true,
    },
    // Claude Opus 4.7
    //
    // Sources:
    //   - Models overview:
    //     https://platform.claude.com/docs/en/about-claude/models/overview
    //     (1M context, 128k sync output, output-300k-2026-03-24 batch beta)
    //   - Migration guide:
    //     https://platform.claude.com/docs/en/about-claude/models/migration-guide
    //     (rejects non-default temperature/top_p/top_k; manual thinking returns 400)
    //   - Adaptive thinking:
    //     https://platform.claude.com/docs/en/build-with-claude/adaptive-thinking
    //     (adaptive-only; off by default unless thinking: {type: "adaptive"})
    //   - Effort:
    //     https://platform.claude.com/docs/en/build-with-claude/effort
    //     (supported, values low/medium/high/xhigh/max)
    //   - Compaction:
    //     https://platform.claude.com/docs/en/build-with-claude/compaction
    //     (supported; compact-2026-01-12 beta header)
    ModelCapabilities {
        id: "claude-opus-4-7",
        provider: Provider::Anthropic,
        display_name: "Claude Opus 4.7",
        tier: ModelTier::Supported,
        release_stage: ModelReleaseStage::Stable,
        model_family: "claude-opus-4",
        context_window: Some(1_000_000),
        max_input_tokens: None,
        max_output_tokens: Some(128_000),
        context_window_beta: None,
        max_output_tokens_beta: Some(BETA_OUTPUT_300K),
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        realtime_supports_provider_managed_turns: false,
        realtime_supports_explicit_commit: false,
        realtime_interrupt_supported: false,
        realtime_transcript_supported: false,
        transcription_companion_model: None,
        image_generation: false,
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::AnthropicAdaptiveOnly,
        anthropic_disabled_thinking: AnthropicDisabledThinking::AnyEffort,
        supports_reasoning: false,
        effort_levels: CLAUDE_5_OPUS_48_47_EFFORT,
        openai_responses_params: None,
        supports_web_search: true,
        supports_mid_conversation_system_messages: false,
        supports_inference_geo: true,
        supports_compaction: true,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: ADAPTIVE_COMPACTION_BETAS,
        call_timeout_secs: Some(300),
        stream_inactivity_timeout_secs: None,
        supports_forced_tool_choice: true,
    },
    // Claude Sonnet 5.5
    //
    // Sources:
    //   - Model page (API ID claude-sonnet-5-5, dateless pinned snapshot;
    //     released September 28, 2026; 1M context; 128k max output; 300k on
    //     the Batch API with output-300k-2026-03-24; text and image input;
    //     adaptive thinking on by default; default effort `high`; non-default
    //     temperature/top_p/top_k return 400):
    //     https://platform.claude.com/docs/en/models/sonnet-5-5/overview
    //   - What's new (`{type: "disabled"}` returns 400, the lowest setting is
    //     `between_tools`; `budget_tokens` returns 400; `between_tools` is
    //     refused at `xhigh`/`max`, so the ladder is low..max; mid-conversation
    //     system messages supported; forced `tool_choice` returns 400):
    //     https://platform.claude.com/docs/en/models/sonnet-5-5/whats-new-sonnet-5-5
    //   - Pricing ($2 / $10 per MTok, 5m cache write $2.50, 1h $4, cache read
    //     $0.20; `inference_geo` applies to Claude 4.6 and later models):
    //     https://platform.claude.com/docs/en/about-claude/pricing
    //   - Compaction (threshold compaction lists Claude Sonnet 5.5 alongside
    //     Fable 5.1 and Opus 5.5):
    //     https://platform.claude.com/docs/en/build-with-claude/compaction
    //
    // Thinking is `AnthropicAdaptiveOrBetweenTools`: adaptive (the default)
    // or `between_tools`; `disabled` and `enabled` budgets are refused
    // locally before the provider call. ASSUMED from the claude-opus-5-5
    // row:
    //   - image_tool_results (vision and tool use are documented; image tool
    //     results are not called out separately)
    //   - supports_structured_output (the forced-tool-use migration points at
    //     structured outputs; the feature page is not re-verified here)
    // Meerkat-owned operational default: Sonnet-tier 120s call timeout.
    ModelCapabilities {
        id: "claude-sonnet-5-5",
        provider: Provider::Anthropic,
        display_name: "Claude Sonnet 5.5",
        tier: ModelTier::Recommended,
        release_stage: ModelReleaseStage::Stable,
        model_family: "claude-sonnet-5",
        context_window: Some(1_000_000),
        max_input_tokens: None,
        max_output_tokens: Some(128_000),
        context_window_beta: None,
        max_output_tokens_beta: Some(BETA_OUTPUT_300K),
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        realtime_supports_provider_managed_turns: false,
        realtime_supports_explicit_commit: false,
        realtime_interrupt_supported: false,
        realtime_transcript_supported: false,
        transcription_companion_model: None,
        image_generation: false,
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::AnthropicAdaptiveOrBetweenTools,
        anthropic_disabled_thinking: AnthropicDisabledThinking::Refused,
        supports_reasoning: false,
        effort_levels: CLAUDE_5_OPUS_48_47_EFFORT,
        openai_responses_params: None,
        supports_web_search: true,
        supports_mid_conversation_system_messages: true,
        supports_inference_geo: true,
        supports_compaction: true,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: PRESERVED_THINKING_BETAS,
        call_timeout_secs: Some(120),
        stream_inactivity_timeout_secs: None,
        // Forced `tool_choice` (`any`/`tool`) returns 400 on Sonnet 5.5,
        // including on token counting (what's new and the migration guide,
        // "Forced tool use is not supported"); refuse it locally, as for
        // claude-opus-5-5.
        supports_forced_tool_choice: false,
    },
    // Claude Sonnet 5
    //
    // Legacy but still available upstream (superseded by Sonnet 5.5, not
    // retired), so it stays a supported catalog row.
    //
    // Sources:
    //   - Model page (API ID claude-sonnet-5; "Active (legacy)"; released
    //     June 30, 2026; retirement not sooner than June 30, 2027; 1M
    //     context; 128k max output; 300k on the Batch API with
    //     output-300k-2026-03-24; text and image input; adaptive thinking;
    //     default effort `high`; non-default temperature/top_p/top_k return
    //     400; $2 / $10 per MTok):
    //     https://platform.claude.com/docs/en/models/sonnet-5/overview
    //   - Models overview ("Legacy models (still available)" lists Claude
    //     Sonnet 5; the batch output-300k note lists it):
    //     https://platform.claude.com/docs/en/about-claude/models/overview
    //   - Sonnet 5.5 migration guide, "from Claude Sonnet 5" (Sonnet 5 accepts
    //     thinking `adaptive` and `disabled`, not `between_tools`; it accepts
    //     forced `tool_choice` `any`/`tool`, which Sonnet 5.5 refuses):
    //     https://platform.claude.com/docs/en/models/sonnet-5-5/migration-guide
    //   - Mid-conversation system messages ("not available on Claude Sonnet
    //     5. Use the top-level system field there instead."):
    //     https://platform.claude.com/docs/en/build-with-claude/mid-conversation-system-messages
    //   - Effort (`claude-sonnet-5` supported; `xhigh` and `max` both list
    //     Claude Sonnet 5; default `high`):
    //     https://platform.claude.com/docs/en/build-with-claude/effort
    //   - Structured outputs (`claude-sonnet-5` in supportedModels):
    //     https://platform.claude.com/docs/en/build-with-claude/structured-outputs
    //   - Compaction at a token threshold (`claude-sonnet-5` with
    //     compact-2026-01-12):
    //     https://platform.claude.com/docs/en/build-with-claude/compaction-threshold
    //   - Pricing (`inference_geo` applies to Claude 4.6 and later models):
    //     https://platform.claude.com/docs/en/about-claude/pricing
    //
    // Thinking is `AnthropicAdaptiveOnly` (adaptive is the default) plus
    // `disabled` at every effort: neither the migration guide above nor the
    // disabled-thinking table records an effort ceiling for Sonnet 5.
    // ASSUMED from the claude-sonnet-5-5 row:
    //   - image_tool_results (vision and tool use are documented)
    //   - supports_web_search (family-conservative default)
    // Meerkat-owned operational default: Sonnet-tier 120s call timeout.
    ModelCapabilities {
        id: "claude-sonnet-5",
        provider: Provider::Anthropic,
        display_name: "Claude Sonnet 5",
        tier: ModelTier::Supported,
        release_stage: ModelReleaseStage::Stable,
        model_family: "claude-sonnet-5",
        context_window: Some(1_000_000),
        max_input_tokens: None,
        max_output_tokens: Some(128_000),
        context_window_beta: None,
        max_output_tokens_beta: Some(BETA_OUTPUT_300K),
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        realtime_supports_provider_managed_turns: false,
        realtime_supports_explicit_commit: false,
        realtime_interrupt_supported: false,
        realtime_transcript_supported: false,
        transcription_companion_model: None,
        image_generation: false,
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::AnthropicAdaptiveOnly,
        anthropic_disabled_thinking: AnthropicDisabledThinking::AnyEffort,
        supports_reasoning: false,
        effort_levels: CLAUDE_5_OPUS_48_47_EFFORT,
        openai_responses_params: None,
        supports_web_search: true,
        supports_mid_conversation_system_messages: false,
        supports_inference_geo: true,
        supports_compaction: true,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: ADAPTIVE_COMPACTION_BETAS,
        call_timeout_secs: Some(120),
        stream_inactivity_timeout_secs: None,
        supports_forced_tool_choice: true,
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
        provider: Provider::Anthropic,
        display_name: "Claude Sonnet 4.6",
        tier: ModelTier::Recommended,
        release_stage: ModelReleaseStage::Stable,
        model_family: "claude-sonnet-4",
        context_window: Some(1_000_000),
        max_input_tokens: None,
        max_output_tokens: Some(64_000),
        context_window_beta: None,
        max_output_tokens_beta: Some(BETA_OUTPUT_300K),
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        realtime_supports_provider_managed_turns: false,
        realtime_supports_explicit_commit: false,
        realtime_interrupt_supported: false,
        realtime_transcript_supported: false,
        transcription_companion_model: None,
        image_generation: false,
        supports_temperature: true,
        supports_top_p: true,
        supports_top_k: true,
        thinking: ThinkingSupport::AnthropicAdaptiveAndEnabled,
        anthropic_disabled_thinking: AnthropicDisabledThinking::AnyEffort,
        supports_reasoning: false,
        effort_levels: SONNET_46_EFFORT,
        openai_responses_params: None,
        supports_web_search: true,
        supports_mid_conversation_system_messages: false,
        supports_inference_geo: true,
        supports_compaction: true,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: true,
        beta_headers: ADAPTIVE_COMPACTION_BETAS,
        call_timeout_secs: Some(120),
        stream_inactivity_timeout_secs: None,
        supports_forced_tool_choice: true,
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
        provider: Provider::Anthropic,
        display_name: "Claude Sonnet 4.5",
        tier: ModelTier::Supported,
        release_stage: ModelReleaseStage::Stable,
        model_family: "claude-sonnet-4",
        context_window: Some(200_000),
        max_input_tokens: None,
        max_output_tokens: Some(64_000),
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        realtime_supports_provider_managed_turns: false,
        realtime_supports_explicit_commit: false,
        realtime_interrupt_supported: false,
        realtime_transcript_supported: false,
        transcription_companion_model: None,
        image_generation: false,
        supports_temperature: true,
        supports_top_p: true,
        supports_top_k: true,
        thinking: ThinkingSupport::AnthropicEnabledOnly,
        anthropic_disabled_thinking: AnthropicDisabledThinking::AnyEffort,
        supports_reasoning: false,
        effort_levels: &[],
        openai_responses_params: None,
        supports_web_search: true,
        supports_mid_conversation_system_messages: false,
        supports_inference_geo: true,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: true,
        beta_headers: LEGACY_THINKING_BETAS,
        call_timeout_secs: Some(120),
        stream_inactivity_timeout_secs: None,
        supports_forced_tool_choice: true,
    },
    // Claude Haiku 5.5
    //
    // Sources:
    //   - Model page (API ID claude-haiku-5-5, a fixed id with no date suffix
    //     and no separate alias; released October 7, 2026; 1M context; 128k
    //     max output; 300k on the Batch API with output-300k-2026-03-24; text
    //     and image input; adaptive thinking on by default; default effort
    //     `medium`; omit temperature, top_p and top_k):
    //     https://platform.claude.com/docs/en/models/haiku-5-5/overview
    //   - What's new and migration guide (`budget_tokens` returns 400;
    //     `{type: "disabled"}` is accepted at `high` effort or below;
    //     temperature must be 1 and top_p 0.99 if sent, any top_k returns 400;
    //     assistant prefill returns 400; forced `tool_choice` `any`/`tool` is
    //     accepted, the response then starts with the tool call):
    //     https://platform.claude.com/docs/en/models/haiku-5-5/whats-new-haiku-5-5
    //     https://platform.claude.com/docs/en/models/haiku-5-5/migration-guide
    //   - Effort ("Claude Haiku 5.5 supports all five effort levels, and
    //     medium is the default"):
    //     https://platform.claude.com/docs/en/build-with-claude/effort
    //   - Compaction at a token threshold (supported models list Haiku 5.5;
    //     compact-2026-01-12):
    //     https://platform.claude.com/docs/en/build-with-claude/compaction-threshold
    //   - Structured outputs (supported models list Haiku 4.5 and 5.5; not
    //     on Amazon Bedrock, which Meerkat's Anthropic client does not use):
    //     https://platform.claude.com/docs/en/build-with-claude/structured-outputs
    //   - Mid-conversation system messages (available on Claude Haiku 5.5):
    //     https://platform.claude.com/docs/en/build-with-claude/mid-conversation-system-messages
    //   - Data residency (`inference_geo` applies to Claude 4.6 and later
    //     models, Haiku 5.5 included):
    //     https://platform.claude.com/docs/en/manage-claude/data-residency
    //
    // Thinking is `AnthropicAdaptiveOnly` (adaptive is the default) plus
    // `disabled` at `high` effort or below, per the What's new page above;
    // the default effort, `medium`, is within that ceiling.
    //
    // ASSUMED (not stated for this model on the pages above):
    //   - image_tool_results: taken from the claude-sonnet-5-5 row (vision
    //     and tool use are documented; image blocks inside tool results are
    //     not called out separately)
    //   - supports_web_search: taken from the claude-haiku-4-5 row (the web
    //     search tool page defers per-model support to the Models API
    //     `capabilities.server_tools.web_search`, which was not queried)
    // Meerkat-owned operational default: Sonnet-tier 120s call timeout,
    // since adaptive thinking is on by default (Haiku 4.5 keeps 60s).
    ModelCapabilities {
        id: "claude-haiku-5-5",
        provider: Provider::Anthropic,
        display_name: "Claude Haiku 5.5",
        tier: ModelTier::Recommended,
        release_stage: ModelReleaseStage::Stable,
        model_family: "claude-haiku-5",
        context_window: Some(1_000_000),
        max_input_tokens: None,
        max_output_tokens: Some(128_000),
        context_window_beta: None,
        max_output_tokens_beta: Some(BETA_OUTPUT_300K),
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        realtime_supports_provider_managed_turns: false,
        realtime_supports_explicit_commit: false,
        realtime_interrupt_supported: false,
        realtime_transcript_supported: false,
        transcription_companion_model: None,
        image_generation: false,
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::AnthropicAdaptiveOnly,
        anthropic_disabled_thinking: AnthropicDisabledThinking::AtOrBelowHighEffort,
        supports_reasoning: false,
        effort_levels: CLAUDE_5_OPUS_48_47_EFFORT,
        openai_responses_params: None,
        supports_web_search: true,
        supports_mid_conversation_system_messages: true,
        supports_inference_geo: true,
        supports_compaction: true,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: PRESERVED_THINKING_BETAS,
        call_timeout_secs: Some(120),
        stream_inactivity_timeout_secs: None,
        supports_forced_tool_choice: true,
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
        provider: Provider::Anthropic,
        display_name: "Claude Haiku 4.5",
        tier: ModelTier::Recommended,
        release_stage: ModelReleaseStage::Stable,
        model_family: "claude-haiku-4",
        context_window: Some(200_000),
        max_input_tokens: None,
        max_output_tokens: Some(64_000),
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        realtime_supports_provider_managed_turns: false,
        realtime_supports_explicit_commit: false,
        realtime_interrupt_supported: false,
        realtime_transcript_supported: false,
        transcription_companion_model: None,
        image_generation: false,
        supports_temperature: true,
        supports_top_p: true,
        supports_top_k: true,
        thinking: ThinkingSupport::AnthropicEnabledOnly,
        anthropic_disabled_thinking: AnthropicDisabledThinking::AnyEffort,
        supports_reasoning: false,
        effort_levels: &[],
        openai_responses_params: None,
        supports_web_search: true,
        supports_mid_conversation_system_messages: false,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: true,
        beta_headers: LEGACY_THINKING_BETAS,
        call_timeout_secs: Some(60),
        stream_inactivity_timeout_secs: None,
        supports_forced_tool_choice: true,
    },
    // Claude Haiku 4.5 alias
    //
    // Keep the official alias as a first-class catalog row so model/provider
    // resolution stays registry-owned instead of falling back to prefix folklore.
    ModelCapabilities {
        id: "claude-haiku-4-5",
        provider: Provider::Anthropic,
        display_name: "Claude Haiku 4.5",
        tier: ModelTier::Recommended,
        release_stage: ModelReleaseStage::Stable,
        model_family: "claude-haiku-4",
        context_window: Some(200_000),
        max_input_tokens: None,
        max_output_tokens: Some(64_000),
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        realtime_supports_provider_managed_turns: false,
        realtime_supports_explicit_commit: false,
        realtime_interrupt_supported: false,
        realtime_transcript_supported: false,
        transcription_companion_model: None,
        image_generation: false,
        supports_temperature: true,
        supports_top_p: true,
        supports_top_k: true,
        thinking: ThinkingSupport::AnthropicEnabledOnly,
        anthropic_disabled_thinking: AnthropicDisabledThinking::AnyEffort,
        supports_reasoning: false,
        effort_levels: &[],
        openai_responses_params: None,
        supports_web_search: true,
        supports_mid_conversation_system_messages: false,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: true,
        beta_headers: LEGACY_THINKING_BETAS,
        call_timeout_secs: Some(60),
        stream_inactivity_timeout_secs: None,
        supports_forced_tool_choice: true,
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
    //     (supported — in the Mythos/Opus 4.8/Opus 4.7/Sonnet 4.6/Opus 4.5 list;
    //      values low/medium/high/max)
    //   - Compaction:
    //     https://platform.claude.com/docs/en/build-with-claude/compaction
    //     (NOT supported — Opus 4.5 is not in the supported-models list)
    ModelCapabilities {
        id: "claude-opus-4-5",
        provider: Provider::Anthropic,
        display_name: "Claude Opus 4.5",
        tier: ModelTier::Supported,
        release_stage: ModelReleaseStage::Stable,
        model_family: "claude-opus-4",
        context_window: Some(200_000),
        max_input_tokens: None,
        max_output_tokens: Some(64_000),
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: false,
        realtime: false,
        realtime_supports_provider_managed_turns: false,
        realtime_supports_explicit_commit: false,
        realtime_interrupt_supported: false,
        realtime_transcript_supported: false,
        transcription_companion_model: None,
        image_generation: false,
        supports_temperature: true,
        supports_top_p: true,
        supports_top_k: true,
        thinking: ThinkingSupport::AnthropicEnabledOnly,
        anthropic_disabled_thinking: AnthropicDisabledThinking::AnyEffort,
        supports_reasoning: false,
        effort_levels: OPUS_45_EFFORT,
        openai_responses_params: None,
        supports_web_search: true,
        supports_mid_conversation_system_messages: false,
        supports_inference_geo: true,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: true,
        beta_headers: LEGACY_THINKING_BETAS,
        call_timeout_secs: Some(300),
        stream_inactivity_timeout_secs: None,
        supports_forced_tool_choice: true,
    },
];
