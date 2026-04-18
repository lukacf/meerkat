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
];
