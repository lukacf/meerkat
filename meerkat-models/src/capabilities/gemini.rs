//! Per-model capability rows for Gemini models.
//!
//! All values are cited against authoritative Google primary sources:
//! `ai.google.dev/gemini-api/docs/models/*`, `ai.google.dev/gemini-api/docs/thinking`,
//! and `ai.google.dev/gemini-api/docs/gemini-3`.

use meerkat_core::Provider;
use meerkat_core::model_profile::capabilities::{ModelCapabilities, ThinkingSupport};
use meerkat_core::model_profile::catalog::{ModelReleaseStage, ModelTier};

/// Capability rows for Gemini catalog models.
pub const CAPABILITIES: &[ModelCapabilities] = &[
    // Gemini 3.8 Flash
    //
    // Sources:
    //   - Latest model guide:
    //     https://ai.google.dev/gemini-api/docs/latest-model
    //     (GA; input 1M; output 64k; vision + video inputs; structured output;
    //      thinking_level low/medium/high with default "medium"; sampling
    //      parameters and legacy thinking_budget are not supported)
    //   - Model page:
    //     https://ai.google.dev/gemini-api/docs/models/gemini-3.8-flash
    ModelCapabilities {
        id: "gemini-3.8-flash",
        provider: Provider::Gemini,
        display_name: "Gemini 3.8 Flash",
        tier: ModelTier::Recommended,
        release_stage: ModelReleaseStage::Stable,
        model_family: "gemini-3",
        context_window: Some(1_048_576),
        max_output_tokens: Some(65_536),
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: true,
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
        thinking: ThinkingSupport::GeminiThinkingLevel,
        supports_reasoning: false,
        effort_levels: &[],
        openai_responses_params: None,
        supports_web_search: true,
        supports_mid_conversation_system_messages: false,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: &[],
        call_timeout_secs: Some(120),
    },
    // Gemini 3.7 Flash
    //
    // Sources:
    //   - Model page:
    //     https://ai.google.dev/gemini-api/docs/models/gemini-3.7-flash
    //   - Thinking:
    //     https://ai.google.dev/gemini-api/docs/thinking
    //     (thinking_level low/medium/high with default "medium")
    //   - Gemini 3.8 migration guidance:
    //     https://ai.google.dev/gemini-api/docs/latest-model
    //     (3.7 remains fully supported)
    ModelCapabilities {
        id: "gemini-3.7-flash",
        provider: Provider::Gemini,
        display_name: "Gemini 3.7 Flash",
        tier: ModelTier::Supported,
        release_stage: ModelReleaseStage::Stable,
        model_family: "gemini-3",
        context_window: Some(1_048_576),
        max_output_tokens: Some(65_536),
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: true,
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
        thinking: ThinkingSupport::GeminiThinkingLevel,
        supports_reasoning: false,
        effort_levels: &[],
        openai_responses_params: None,
        supports_web_search: true,
        supports_mid_conversation_system_messages: false,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: &[],
        call_timeout_secs: Some(120),
    },
    // Gemini 3.5 Flash
    //
    // Sources:
    //   - Model page:
    //     https://ai.google.dev/gemini-api/docs/models/gemini-3.5-flash
    //     (input 1,048,576; output 65,536; vision + video inputs; structured output)
    //   - Thinking:
    //     https://ai.google.dev/gemini-api/docs/thinking
    //     (thinking_level minimal/low/medium/high; default "high")
    //   - Gemini 3 guide:
    //     https://ai.google.dev/gemini-api/docs/gemini-3
    //     (thinking_budget accepted for backward compatibility)
    ModelCapabilities {
        id: "gemini-3.5-flash",
        provider: Provider::Gemini,
        display_name: "Gemini 3.5 Flash",
        tier: ModelTier::Supported,
        release_stage: ModelReleaseStage::Stable,
        model_family: "gemini-3",
        context_window: Some(1_048_576),
        max_output_tokens: Some(65_536),
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: true,
        realtime: false,
        realtime_supports_provider_managed_turns: false,
        realtime_supports_explicit_commit: false,
        realtime_interrupt_supported: false,
        realtime_transcript_supported: false,
        transcription_companion_model: None,
        image_generation: false,
        supports_temperature: true,
        // top_p/top_k support is not explicitly stated on the Gemini 3.x
        // model pages. Match the legacy schema which advertised both.
        supports_top_p: true,
        supports_top_k: true,
        thinking: ThinkingSupport::GeminiThinkingLevel,
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
        provider: Provider::Gemini,
        display_name: "Gemini 3.1 Pro Preview",
        tier: ModelTier::Supported,
        release_stage: ModelReleaseStage::Stable,
        model_family: "gemini-3",
        context_window: Some(1_048_576),
        max_output_tokens: Some(65_536),
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: true,
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
        thinking: ThinkingSupport::GeminiThinkingLevel,
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
        provider: Provider::Gemini,
        display_name: "Gemini 3.1 Flash Lite Preview",
        tier: ModelTier::Supported,
        release_stage: ModelReleaseStage::Stable,
        model_family: "gemini-3",
        context_window: Some(1_048_576),
        max_output_tokens: Some(65_536),
        context_window_beta: None,
        max_output_tokens_beta: None,
        vision: true,
        image_tool_results: true,
        inline_video: true,
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
        thinking: ThinkingSupport::GeminiThinkingLevel,
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
        beta_headers: &[],
        call_timeout_secs: Some(120),
    },
];
