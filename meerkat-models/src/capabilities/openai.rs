//! Per-model capability rows for OpenAI models.
//!
//! All values are cited against authoritative OpenAI primary sources:
//! `developers.openai.com/api/docs/models/*`, `developers.openai.com/api/docs/guides/reasoning`,
//! `openai.com/index/*`, and the Codex model index.

use meerkat_core::Provider;
use meerkat_core::model_profile::capabilities::{
    EffortLevel, ModelCapabilities, OpenAiPromptCacheMode, OpenAiPromptCacheTtl,
    OpenAiReasoningContext, OpenAiReasoningMode, OpenAiResponsesParamCapabilities,
    OpenAiTextVerbosity, ThinkingSupport,
};
use meerkat_core::model_profile::catalog::ModelTier;

/// Reasoning-effort levels accepted by recent GPT-5.x models (5.4, 5.5, 5.5-pro).
///
/// Verified against the live API: `minimal` is rejected with
/// "Supported values are: 'none', 'low', 'medium', 'high', and 'xhigh'."
/// Earlier docs implying `minimal` support appear to apply only to older
/// GPT-5.x models. Default is `none` — opt-in reasoning.
const GPT5_RECENT_EFFORT: &[EffortLevel] = &[
    EffortLevel::None,
    EffortLevel::Low,
    EffortLevel::Medium,
    EffortLevel::High,
    EffortLevel::Xhigh,
];

/// Reasoning-effort levels accepted by the GPT-5.6 family.
///
/// Source: https://developers.openai.com/api/docs/guides/latest-model
/// (`max` is the new API effort value; Codex `ultra` is a separate
/// multi-agent product mode, not a `reasoning.effort` wire value).
const GPT56_EFFORT: &[EffortLevel] = &[
    EffortLevel::None,
    EffortLevel::Low,
    EffortLevel::Medium,
    EffortLevel::High,
    EffortLevel::Xhigh,
    EffortLevel::Max,
];

const GPT56_RESPONSES_PARAMS: OpenAiResponsesParamCapabilities = OpenAiResponsesParamCapabilities {
    reasoning_modes: &[OpenAiReasoningMode::Standard, OpenAiReasoningMode::Pro],
    reasoning_contexts: &[
        OpenAiReasoningContext::Auto,
        OpenAiReasoningContext::CurrentTurn,
        OpenAiReasoningContext::AllTurns,
    ],
    text_verbosity_levels: &[
        OpenAiTextVerbosity::Low,
        OpenAiTextVerbosity::Medium,
        OpenAiTextVerbosity::High,
    ],
    prompt_cache_modes: &[
        OpenAiPromptCacheMode::Implicit,
        OpenAiPromptCacheMode::Explicit,
    ],
    prompt_cache_ttls: &[OpenAiPromptCacheTtl::ThirtyMinutes],
    supports_in_memory_prompt_cache_retention: false,
};

/// Reasoning-effort levels accepted by GPT-5.3 Codex.
///
/// Source: https://developers.openai.com/api/docs/models/gpt-5.3-codex
/// (Codex is a reasoning-only model — no `none`/`minimal`).
const GPT5_3_CODEX_EFFORT: &[EffortLevel] = &[
    EffortLevel::Low,
    EffortLevel::Medium,
    EffortLevel::High,
    EffortLevel::Xhigh,
];

/// Reasoning-effort levels accepted by `gpt-realtime-2`.
///
/// Sources:
///   - https://developers.openai.com/api/docs/models/gpt-realtime-2
///   - https://openai.com/index/advancing-voice-intelligence-with-new-models-in-the-api/
///
/// "Developers can now select from minimal, low, medium, high, and xhigh
/// reasoning levels, with low as the default" — gpt-realtime-2 introduced
/// configurable reasoning over the audio-first streaming substrate.
const REALTIME_2_EFFORT: &[EffortLevel] = &[
    EffortLevel::Minimal,
    EffortLevel::Low,
    EffortLevel::Medium,
    EffortLevel::High,
    EffortLevel::Xhigh,
];

/// Capability rows for OpenAI catalog models.
pub const CAPABILITIES: &[ModelCapabilities] = &[
    // GPT-5.6 Sol / Terra / Luna (plus the official Sol alias)
    //
    // Sources:
    //   - https://developers.openai.com/api/docs/models/gpt-5.6-sol
    //   - https://developers.openai.com/api/docs/models/gpt-5.6-terra
    //   - https://developers.openai.com/api/docs/models/gpt-5.6-luna
    //   - https://developers.openai.com/api/docs/guides/latest-model
    //
    // Availability remains a limited preview for selected trusted partners
    // and organizations. All three expose a 1,050,000-token context window, 128,000-token max
    // output, text+image input, text output, Responses/Chat Completions/Batch,
    // structured outputs, function calling, image generation, and web search.
    // GPT-5.6 defaults to medium reasoning and adds the `max` effort tier.
    // Sampling and legacy penalty/seed fields stay conservative: the current
    // Responses Create contract does not establish model-specific support for
    // them. The 600s timeout is the existing Meerkat operational default for
    // standard frontier OpenAI requests.
    ModelCapabilities {
        id: "gpt-5.6-sol",
        provider: Provider::OpenAI,
        display_name: "GPT-5.6 Sol",
        tier: ModelTier::Supported,
        model_family: "gpt-5",
        context_window: 1_050_000,
        max_output_tokens: 128_000,
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
        image_generation: true,
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::None,
        supports_reasoning: true,
        effort_levels: GPT56_EFFORT,
        openai_responses_params: Some(GPT56_RESPONSES_PARAMS),
        supports_web_search: true,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: &[],
        call_timeout_secs: Some(600),
    },
    ModelCapabilities {
        id: "gpt-5.6-terra",
        provider: Provider::OpenAI,
        display_name: "GPT-5.6 Terra",
        tier: ModelTier::Supported,
        model_family: "gpt-5",
        context_window: 1_050_000,
        max_output_tokens: 128_000,
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
        image_generation: true,
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::None,
        supports_reasoning: true,
        effort_levels: GPT56_EFFORT,
        openai_responses_params: Some(GPT56_RESPONSES_PARAMS),
        supports_web_search: true,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: &[],
        call_timeout_secs: Some(600),
    },
    ModelCapabilities {
        id: "gpt-5.6-luna",
        provider: Provider::OpenAI,
        display_name: "GPT-5.6 Luna",
        tier: ModelTier::Supported,
        model_family: "gpt-5",
        context_window: 1_050_000,
        max_output_tokens: 128_000,
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
        image_generation: true,
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::None,
        supports_reasoning: true,
        effort_levels: GPT56_EFFORT,
        openai_responses_params: Some(GPT56_RESPONSES_PARAMS),
        supports_web_search: true,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: &[],
        call_timeout_secs: Some(600),
    },
    // Official alias: gpt-5.6 routes to gpt-5.6-sol.
    // Keep it catalog-owned so selection does not fall back to prefix inference.
    ModelCapabilities {
        id: "gpt-5.6",
        provider: Provider::OpenAI,
        display_name: "GPT-5.6 Sol",
        tier: ModelTier::Supported,
        model_family: "gpt-5",
        context_window: 1_050_000,
        max_output_tokens: 128_000,
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
        image_generation: true,
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::None,
        supports_reasoning: true,
        effort_levels: GPT56_EFFORT,
        openai_responses_params: Some(GPT56_RESPONSES_PARAMS),
        supports_web_search: true,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: &[],
        call_timeout_secs: Some(600),
    },
    // GPT-5.5
    //
    // Successor to GPT-5.4. Capability shape is a catalog-owned row so
    // request policy does not infer facts from the model name.
    ModelCapabilities {
        id: "gpt-5.5",
        provider: Provider::OpenAI,
        display_name: "GPT-5.5",
        tier: ModelTier::Recommended,
        model_family: "gpt-5",
        context_window: 1_050_000,
        max_output_tokens: 128_000,
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
        image_generation: true,
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::None,
        supports_reasoning: true,
        effort_levels: GPT5_RECENT_EFFORT,
        openai_responses_params: None,
        supports_web_search: true,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: true,
        supports_thinking_budget_legacy: false,
        beta_headers: &[],
        call_timeout_secs: Some(600),
    },
    // GPT-5.5 Pro
    //
    // Pro tier of GPT-5.5. The long-running timeout is explicit catalog data,
    // not derived from the `-pro` suffix.
    ModelCapabilities {
        id: "gpt-5.5-pro",
        provider: Provider::OpenAI,
        display_name: "GPT-5.5 Pro",
        tier: ModelTier::Recommended,
        model_family: "gpt-5",
        context_window: 1_050_000,
        max_output_tokens: 128_000,
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
        image_generation: true,
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::None,
        supports_reasoning: true,
        effort_levels: GPT5_RECENT_EFFORT,
        openai_responses_params: None,
        supports_web_search: true,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: true,
        supports_thinking_budget_legacy: false,
        beta_headers: &[],
        call_timeout_secs: Some(7200),
    },
    // GPT-5.4
    //
    // Sources:
    //   - Model page:
    //     https://developers.openai.com/api/docs/models/gpt-5.4
    //     (context 1,050,000; max output 128,000; reasoning opt-in default none)
    //   - Reasoning guide:
    //     https://developers.openai.com/api/docs/guides/reasoning
    //
    // Demoted to Supported with the GPT-5.5 release; remains a fully
    // supported choice while the current catalog default is GPT-5.6 Sol.
    ModelCapabilities {
        id: "gpt-5.4",
        provider: Provider::OpenAI,
        display_name: "GPT-5.4",
        tier: ModelTier::Supported,
        model_family: "gpt-5",
        context_window: 1_050_000,
        max_output_tokens: 128_000,
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
        image_generation: true,
        // Primary docs on the model page do not definitively state
        // temperature acceptance when reasoning is `none`. Stay with the
        // conservative pre-refactor stance (reject) until confirmed.
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::None,
        supports_reasoning: true,
        effort_levels: GPT5_RECENT_EFFORT,
        openai_responses_params: None,
        supports_web_search: true,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: true,
        supports_thinking_budget_legacy: false,
        beta_headers: &[],
        call_timeout_secs: Some(600),
    },
    // GPT-5.4 mini
    //
    // Sources:
    //   - Model page:
    //     https://developers.openai.com/api/docs/models/gpt-5.4-mini
    //     (model type reasoning; current snapshot gpt-5.4-mini-2026-03-17)
    //   - Changelog:
    //     https://developers.openai.com/api/docs/changelog
    //     (Chat Completions + Responses API; supports tool search,
    //      built-in computer use, and compaction)
    ModelCapabilities {
        id: "gpt-5.4-mini",
        provider: Provider::OpenAI,
        display_name: "GPT-5.4 Mini",
        tier: ModelTier::Supported,
        model_family: "gpt-5",
        context_window: 128_000,
        max_output_tokens: 16_384,
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
        image_generation: true,
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::None,
        supports_reasoning: true,
        effort_levels: GPT5_RECENT_EFFORT,
        openai_responses_params: None,
        supports_web_search: true,
        supports_inference_geo: false,
        supports_compaction: true,
        supports_structured_output: true,
        supports_legacy_penalties: true,
        supports_thinking_budget_legacy: false,
        beta_headers: &[],
        call_timeout_secs: Some(600),
    },
    // GPT-5.3 Codex
    //
    // Sources:
    //   - Model page:
    //     https://developers.openai.com/api/docs/models/gpt-5.3-codex
    //     (context 400,000; max output 128,000; reasoning always-on)
    //   - Announcement:
    //     https://openai.com/index/introducing-gpt-5-3-codex/
    //   - Web search: not listed among Codex-supported tools on the model page
    ModelCapabilities {
        id: "gpt-5.3-codex",
        provider: Provider::OpenAI,
        display_name: "GPT-5.3 Codex",
        tier: ModelTier::Supported,
        model_family: "codex",
        context_window: 400_000,
        max_output_tokens: 128_000,
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
        image_generation: true,
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::None,
        supports_reasoning: true,
        effort_levels: GPT5_3_CODEX_EFFORT,
        openai_responses_params: None,
        supports_web_search: false,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: true,
        supports_thinking_budget_legacy: false,
        beta_headers: &[],
        call_timeout_secs: Some(600),
    },
    // gpt-realtime-2
    //
    // OpenAI's canonical realtime model (supersedes gpt-realtime / gpt-4o-realtime-preview).
    // gpt-realtime-2: OpenAI's current canonical realtime voice model.
    //
    // Audio-first streaming model used via the realtime WebSocket API.
    // Tool-calling supported via realtime session function declarations.
    // Accepts text + audio + image input; emits text + audio output.
    // Configurable reasoning effort (minimal/low/medium/high/xhigh, default
    // low) — this is the first realtime generation with reasoning.
    // Structured outputs are NOT supported (per OpenAI model docs).
    //
    // Verified specs (May 2026):
    //   - context window: 128,000 tokens
    //   - max output tokens: 32,000
    //   - input: text, audio, image — vision: true, inline_video: false
    //   - reasoning: minimal/low/medium/high/xhigh (default low)
    //   - structured outputs: not supported
    //
    // Sources:
    //   - https://developers.openai.com/api/docs/models/gpt-realtime-2
    //   - https://openai.com/index/advancing-voice-intelligence-with-new-models-in-the-api/
    //
    // Session capability meaning: realtime=true unlocks the realtime transport
    // substrate on a session whose current LLM is a realtime-capable model.
    // Sessions on non-realtime models (gpt-5.4, etc.) must
    // `session/reconfigure_llm` to a realtime-capable model before opening a
    // realtime channel.
    ModelCapabilities {
        id: "gpt-realtime-2",
        provider: Provider::OpenAI,
        display_name: "GPT Realtime 2",
        tier: ModelTier::Recommended,
        model_family: "gpt-realtime",
        context_window: 128_000,
        max_output_tokens: 32_000,
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: false,
        inline_video: false,
        realtime: true,
        // Realtime transport capability facts for gpt-realtime-2: both turn
        // modes (server VAD + explicit commit), barge-in, and spoken
        // transcripts are supported; input ASR pairs with the mini transcribe
        // companion. The provider seam maps these into the wire
        // `RealtimeCapabilities` (turning_modes / interrupt / transcript) and
        // resolves the transcription model from `transcription_companion_model`.
        realtime_supports_provider_managed_turns: true,
        realtime_supports_explicit_commit: true,
        realtime_interrupt_supported: true,
        realtime_transcript_supported: true,
        transcription_companion_model: Some("gpt-4o-mini-transcribe"),
        image_generation: true,
        supports_temperature: true,
        supports_top_p: true,
        supports_top_k: false,
        thinking: ThinkingSupport::None,
        supports_reasoning: true,
        effort_levels: REALTIME_2_EFFORT,
        openai_responses_params: None,
        supports_web_search: false,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: false,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: &[],
        // Realtime is streaming (WebSocket) — per-call timeout here is a
        // loose ceiling on synchronous fallback paths. Realtime transport
        // owns its own reconnect/heartbeat policy.
        call_timeout_secs: Some(600),
    },
];
