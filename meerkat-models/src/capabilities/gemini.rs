//! Per-model capability rows for Gemini models.
//!
//! All values are cited against authoritative Google primary sources:
//! `ai.google.dev/gemini-api/docs/models/*`, `ai.google.dev/gemini-api/docs/thinking`,
//! and `ai.google.dev/gemini-api/docs/gemini-3`.

use super::{ModelCapabilities, ThinkingSupport};
use crate::catalog::ModelTier;

/// Capability rows for Gemini catalog models.
pub const CAPABILITIES: &[ModelCapabilities] = &[
    // Gemini 3 Flash Preview
    //
    // Sources:
    //   - Model page:
    //     https://ai.google.dev/gemini-api/docs/models/gemini-3-flash-preview
    //     (input 1,048,576; output 65,536; vision + video inputs; structured output)
    //   - Thinking:
    //     https://ai.google.dev/gemini-api/docs/thinking
    //     (thinking_level minimal/low/medium/high; default "high")
    //   - Gemini 3 guide:
    //     https://ai.google.dev/gemini-api/docs/gemini-3
    //     (thinking_budget accepted for backward compatibility)
    ModelCapabilities {
        id: "gemini-3-flash-preview",
        provider: "gemini",
        display_name: "Gemini 3 Flash Preview",
        tier: ModelTier::Recommended,
        model_family: "gemini-3",
        context_window: 1_048_576,
        max_output_tokens: 65_536,
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: true,
        supports_temperature: true,
        // top_p/top_k support is not explicitly stated on the Gemini 3.x
        // model pages. Match the legacy schema which advertised both.
        supports_top_p: true,
        supports_top_k: true,
        thinking: ThinkingSupport::GeminiThinkingLevel,
        supports_reasoning: false,
        effort_levels: &[],
        supports_web_search: true,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: true,
        beta_headers: &[],
        call_timeout_secs: Some(120),
    },
    // Gemini 3.1 Pro Preview
    //
    // Sources:
    //   - Model page:
    //     https://ai.google.dev/gemini-api/docs/models/gemini-3.1-pro-preview
    //     (input 1,048,576; output 65,536)
    //   - Thinking:
    //     https://ai.google.dev/gemini-api/docs/thinking
    //     (thinking_level default "high")
    //   - Prior gemini-3-pro-preview deprecation: shutdown 2026-03-09;
    //     migrate to 3.1 Pro
    ModelCapabilities {
        id: "gemini-3.1-pro-preview",
        provider: "gemini",
        display_name: "Gemini 3.1 Pro Preview",
        tier: ModelTier::Supported,
        model_family: "gemini-3",
        context_window: 1_048_576,
        max_output_tokens: 65_536,
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: true,
        supports_temperature: true,
        supports_top_p: true,
        supports_top_k: true,
        thinking: ThinkingSupport::GeminiThinkingLevel,
        supports_reasoning: false,
        effort_levels: &[],
        supports_web_search: true,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: true,
        beta_headers: &[],
        call_timeout_secs: Some(600),
    },
    // Gemini 3.1 Flash Lite Preview
    //
    // Sources:
    //   - Model page:
    //     https://ai.google.dev/gemini-api/docs/models/gemini-3.1-flash-lite-preview
    //     (input 1,048,576; output 65,536)
    //   - Thinking:
    //     https://ai.google.dev/gemini-api/docs/thinking
    //     (thinking_level default "minimal")
    ModelCapabilities {
        id: "gemini-3.1-flash-lite-preview",
        provider: "gemini",
        display_name: "Gemini 3.1 Flash Lite Preview",
        tier: ModelTier::Supported,
        model_family: "gemini-3",
        context_window: 1_048_576,
        max_output_tokens: 65_536,
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: true,
        supports_temperature: true,
        supports_top_p: true,
        supports_top_k: true,
        thinking: ThinkingSupport::GeminiThinkingLevel,
        supports_reasoning: false,
        effort_levels: &[],
        supports_web_search: true,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: true,
        beta_headers: &[],
        call_timeout_secs: Some(120),
    },
];
