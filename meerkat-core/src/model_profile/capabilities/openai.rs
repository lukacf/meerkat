//! Per-model capability rows for OpenAI models.
//!
//! All values are cited against authoritative OpenAI primary sources:
//! `developers.openai.com/api/docs/models/*`, `developers.openai.com/api/docs/guides/reasoning`,
//! `openai.com/index/*`, and the Codex model index.

use super::{ModelCapabilities, ThinkingSupport};
use crate::model_profile::catalog::ModelTier;

/// Reasoning-effort levels accepted by GPT-5.4.
///
/// Verified against the live API: `minimal` is rejected with
/// "Supported values are: 'none', 'low', 'medium', 'high', and 'xhigh'."
/// Earlier docs implying `minimal` support appear to apply only to older
/// GPT-5.x models, not 5.4.
/// Default is `none` — opt-in reasoning on 5.4.
const GPT5_4_EFFORT: &[&str] = &["none", "low", "medium", "high", "xhigh"];

/// Reasoning-effort levels accepted by GPT-5.3 Codex.
///
/// Source: https://developers.openai.com/api/docs/models/gpt-5.3-codex
/// (Codex is a reasoning-only model — no `none`/`minimal`).
const GPT5_3_CODEX_EFFORT: &[&str] = &["low", "medium", "high", "xhigh"];

/// Realtime models accept no reasoning effort levels — they are audio-first
/// streaming models without thinking semantics.
const REALTIME_NO_EFFORT: &[&str] = &[];

/// Capability rows for OpenAI catalog models.
pub const CAPABILITIES: &[ModelCapabilities] = &[
    // GPT-5.4
    //
    // Sources:
    //   - Model page:
    //     https://developers.openai.com/api/docs/models/gpt-5.4
    //     (context 1,050,000; max output 128,000; reasoning opt-in default none)
    //   - Reasoning guide:
    //     https://developers.openai.com/api/docs/guides/reasoning
    ModelCapabilities {
        id: "gpt-5.4",
        provider: "openai",
        display_name: "GPT-5.4",
        tier: ModelTier::Recommended,
        model_family: "gpt-5",
        context_window: 1_050_000,
        max_output_tokens: 128_000,
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: false,
        inline_video: false,
        realtime: false,
        // Primary docs on the model page do not definitively state
        // temperature acceptance when reasoning is `none`. Stay with the
        // conservative pre-refactor stance (reject) until confirmed.
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::None,
        supports_reasoning: true,
        effort_levels: GPT5_4_EFFORT,
        supports_web_search: true,
        supports_inference_geo: false,
        supports_compaction: false,
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
        provider: "openai",
        display_name: "GPT-5.3 Codex",
        tier: ModelTier::Supported,
        model_family: "codex",
        context_window: 400_000,
        max_output_tokens: 128_000,
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: false,
        inline_video: false,
        realtime: false,
        supports_temperature: false,
        supports_top_p: false,
        supports_top_k: false,
        thinking: ThinkingSupport::None,
        supports_reasoning: true,
        effort_levels: GPT5_3_CODEX_EFFORT,
        supports_web_search: false,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: true,
        supports_thinking_budget_legacy: false,
        beta_headers: &[],
        call_timeout_secs: Some(600),
    },
    // gpt-realtime-1.5
    //
    // OpenAI's canonical realtime model (supersedes gpt-realtime / gpt-4o-realtime-preview).
    // Audio-first streaming model used via the realtime WebSocket API.
    // Tool-calling supported via realtime session function declarations.
    //
    // Sources:
    //   - https://platform.openai.com/docs/models/gpt-realtime
    //   - https://platform.openai.com/docs/guides/realtime
    //
    // Session capability meaning: realtime=true unlocks the realtime transport
    // substrate on a session whose current LLM is a realtime-capable model.
    // Sessions on non-realtime models (gpt-5.4, etc.) must `session/reconfigure_llm`
    // to a realtime-capable model before opening a realtime channel.
    ModelCapabilities {
        id: "gpt-realtime-1.5",
        provider: "openai",
        display_name: "GPT Realtime 1.5",
        tier: ModelTier::Recommended,
        model_family: "gpt-realtime",
        context_window: 128_000,
        max_output_tokens: 4_096,
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: false,
        image_tool_results: false,
        inline_video: false,
        realtime: true,
        supports_temperature: true,
        supports_top_p: true,
        supports_top_k: false,
        thinking: ThinkingSupport::None,
        supports_reasoning: false,
        effort_levels: REALTIME_NO_EFFORT,
        supports_web_search: false,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: &[],
        // Realtime is streaming (WebSocket) — per-call timeout here is a
        // loose ceiling on synchronous fallback paths. Realtime transport
        // owns its own reconnect/heartbeat policy.
        call_timeout_secs: Some(600),
    },
    // gpt-realtime (supported alias for gpt-realtime-1.5 predecessor)
    //
    // OpenAI's prior canonical realtime model. Retained in the catalog as a
    // Supported-tier alias so existing configs referencing `gpt-realtime`
    // keep resolving to a realtime-capable capability row during the
    // 1.5 rollout window.
    ModelCapabilities {
        id: "gpt-realtime",
        provider: "openai",
        display_name: "GPT Realtime (legacy alias)",
        tier: ModelTier::Supported,
        model_family: "gpt-realtime",
        context_window: 128_000,
        max_output_tokens: 4_096,
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: false,
        image_tool_results: false,
        inline_video: false,
        realtime: true,
        supports_temperature: true,
        supports_top_p: true,
        supports_top_k: false,
        thinking: ThinkingSupport::None,
        supports_reasoning: false,
        effort_levels: REALTIME_NO_EFFORT,
        supports_web_search: false,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: &[],
        call_timeout_secs: Some(600),
    },
];
