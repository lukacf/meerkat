//! Per-model capability rows for Gemini models.
//!
//! All values are cited against authoritative Google primary sources:
//! `ai.google.dev/gemini-api/docs/models/*`, `ai.google.dev/gemini-api/docs/thinking`,
//! and `ai.google.dev/gemini-api/docs/gemini-3`.

use meerkat_core::Provider;
use meerkat_core::model_profile::capabilities::{
    AnthropicDisabledThinking, EffortLevel, ModelCapabilities, ThinkingSupport,
};
use meerkat_core::model_profile::catalog::{ModelReleaseStage, ModelTier};

/// Thinking levels of models that refuse `minimal`: the API answers 400
/// "Thinking level MINIMAL is not supported for this model" (live probe
/// 2026-10-14 for gemini-4-argon, gemini-3.8-flash, gemini-3.7-flash and
/// gemini-3.1-pro-preview; the thinking guide lists low/medium/high for them
/// and for gemini-4-carbon).
const GEMINI_LOW_TO_HIGH: &[EffortLevel] =
    &[EffortLevel::Low, EffortLevel::Medium, EffortLevel::High];

/// Thinking levels of models that also accept `minimal` (thinking guide;
/// live probe 2026-10-14 answered 200).
const GEMINI_MINIMAL_TO_HIGH: &[EffortLevel] = &[
    EffortLevel::Minimal,
    EffortLevel::Low,
    EffortLevel::Medium,
    EffortLevel::High,
];

/// Capability rows for Gemini catalog models.
pub const CAPABILITIES: &[ModelCapabilities] = &[
    // Gemini 4 Carbon
    //
    // Sources:
    //   - Model page:
    //     https://ai.google.dev/gemini-api/docs/models/gemini-4-carbon
    //     (stable; text, image, video, audio and PDF input, text output;
    //      1,048,576-token context window shared by input and output; max
    //      output 65,536 synchronous, up to 1,048,576 only with long decode;
    //      thinking levels low, medium and high; implicit caching only,
    //      explicit caching is not supported; function calling, structured
    //      outputs and Search grounding supported; Live API and image
    //      generation not supported)
    //   - Release notes, 2026-11-10:
    //     https://ai.google.dev/gemini-api/docs/changelog
    //     (generally available)
    //   - Latest model guide:
    //     https://ai.google.dev/gemini-api/docs/latest-model
    //     (Google's strongest reasoning model, slower than Gemini 4 Argon;
    //      thinking levels low, medium (default) and high)
    //   - Thinking:
    //     https://ai.google.dev/gemini-api/docs/thinking
    //     (levels low/medium/high, on at medium by default)
    //   - Pricing:
    //     https://ai.google.dev/gemini-api/docs/pricing
    //     (no free tier: paid Gemini API tiers only)
    //   - Long decode:
    //     https://ai.google.dev/gemini-api/docs/long-decode
    //     (set HTTP read timeouts to 15 to 20 minutes per stream)
    //
    // Google documents the limits, inputs, thinking levels, caching and tier
    // of Gemini 4 Argon for this model, so the row is the Argon row under
    // its own identity: sampling parameters are not sent, max_output_tokens
    // is the synchronous cap, and call_timeout_secs and
    // stream_inactivity_timeout_secs are the same 900 s. Not a default
    // model: the Gemini default stays gemini-4-argon.
    ModelCapabilities {
        id: "gemini-4-carbon",
        provider: Provider::Gemini,
        display_name: "Gemini 4 Carbon",
        tier: ModelTier::Recommended,
        release_stage: ModelReleaseStage::Stable,
        model_family: "gemini-4",
        context_window: Some(1_048_576),
        max_input_tokens: None,
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
        anthropic_disabled_thinking: AnthropicDisabledThinking::Refused,
        supports_reasoning: false,
        effort_levels: GEMINI_LOW_TO_HIGH,
        openai_responses_params: None,
        supports_web_search: true,
        supports_mid_conversation_system_messages: false,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: &[],
        call_timeout_secs: Some(900),
        // The stream can stay silent while the model thinks, as on Gemini 4
        // Argon: the same window bounds the silence like the call.
        stream_inactivity_timeout_secs: Some(900),
        supports_forced_tool_choice: true,
    },
    // Gemini 4 Argon
    //
    // Sources:
    //   - Model page:
    //     https://ai.google.dev/gemini-api/docs/models/gemini-4-argon
    //     (stable; text, image, video, audio and PDF input, text output;
    //      1,048,576-token context window shared by input, thinking and
    //      output; max output 65,536 synchronous, up to 1,048,576 only with
    //      long decode; implicit caching only, explicit caching is not
    //      supported; function calling, structured outputs and Search
    //      grounding supported; Live API and image generation not supported)
    //   - Latest model guide:
    //     https://ai.google.dev/gemini-api/docs/latest-model
    //     (GA 2026-10-13, paid tier only; thinking_level low/medium/high with
    //      default "medium" and no way to disable thinking; temperature,
    //      top_p and top_k are ignored if sent; raise max_output_tokens to at
    //      least 32,768; set HTTP client timeouts to at least 15 minutes)
    //   - Thinking:
    //     https://ai.google.dev/gemini-api/docs/thinking
    //     (levels low/medium/high; synchronous max_output_tokens cap 65,536)
    //
    // Long decode beyond 65,536 output tokens needs background execution on
    // the Interactions API, which Meerkat does not use, so max_output_tokens
    // is the synchronous cap. Thinking budgets are not documented for this
    // model (thinking_level only). call_timeout_secs and
    // stream_inactivity_timeout_secs follow Google's 15-minute client-timeout
    // guidance. Default Gemini model since 0.9.1
    // (paid Gemini API tier only; the Google OAuth login route keeps its own
    // default).
    ModelCapabilities {
        id: "gemini-4-argon",
        provider: Provider::Gemini,
        display_name: "Gemini 4 Argon",
        tier: ModelTier::Recommended,
        release_stage: ModelReleaseStage::Stable,
        model_family: "gemini-4",
        context_window: Some(1_048_576),
        max_input_tokens: None,
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
        anthropic_disabled_thinking: AnthropicDisabledThinking::Refused,
        supports_reasoning: false,
        effort_levels: GEMINI_LOW_TO_HIGH,
        openai_responses_params: None,
        supports_web_search: true,
        supports_mid_conversation_system_messages: false,
        supports_inference_geo: false,
        supports_compaction: false,
        supports_structured_output: true,
        supports_legacy_penalties: false,
        supports_thinking_budget_legacy: false,
        beta_headers: &[],
        call_timeout_secs: Some(900),
        // The stream can stay silent while the model thinks: Google's
        // 15-minute client-timeout guidance bounds the silence like the call.
        stream_inactivity_timeout_secs: Some(900),
        supports_forced_tool_choice: true,
    },
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
        max_input_tokens: None,
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
        anthropic_disabled_thinking: AnthropicDisabledThinking::Refused,
        supports_reasoning: false,
        effort_levels: GEMINI_LOW_TO_HIGH,
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
        stream_inactivity_timeout_secs: None,
        supports_forced_tool_choice: true,
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
    //   - Release notes, 2026-10-08:
    //     https://ai.google.dev/gemini-api/docs/changelog
    //     (gemini-3.7-flash is deprecated; requests are routed to
    //      gemini-3.8-flash. The row stays so sessions that recorded this id
    //      keep resolving; the catalog has no deprecation field.)
    ModelCapabilities {
        id: "gemini-3.7-flash",
        provider: Provider::Gemini,
        display_name: "Gemini 3.7 Flash",
        tier: ModelTier::Supported,
        release_stage: ModelReleaseStage::Stable,
        model_family: "gemini-3",
        context_window: Some(1_048_576),
        max_input_tokens: None,
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
        anthropic_disabled_thinking: AnthropicDisabledThinking::Refused,
        supports_reasoning: false,
        effort_levels: GEMINI_LOW_TO_HIGH,
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
        stream_inactivity_timeout_secs: None,
        supports_forced_tool_choice: true,
    },
    // Gemini 3.6 Flash
    //
    // Sources:
    //   - Model page:
    //     https://ai.google.dev/gemini-api/docs/models/gemini-3.6-flash
    //     (stable; text, image, video, audio and PDF input, text output;
    //      input 1,048,576; output 65,536; structured outputs, function
    //      calling and Search grounding supported)
    //   - What's new in Gemini 3.6 Flash and 3.5 Flash-Lite:
    //     https://ai.google.dev/gemini-api/docs/whats-new-gemini-3.6
    //     (GA 2026-07-21; previous-generation Flash model that remains
    //      available for production use; default thinking level "medium";
    //      temperature, top_p and top_k are deprecated and ignored; replace
    //      thinking_budget with thinking_level; prefilled model turns 400)
    //   - Thinking:
    //     https://ai.google.dev/gemini-api/docs/thinking
    //     (levels minimal/low/medium/high)
    //
    // Live check 2026-10-14: models.get lists generateContent with
    // 1,048,576 input and 65,536 output tokens, and a streamGenerateContent
    // call answers with modelVersion gemini-3.6-flash. Google routes
    // gemini-3.5-flash requests here since 2026-10-08.
    ModelCapabilities {
        id: "gemini-3.6-flash",
        provider: Provider::Gemini,
        display_name: "Gemini 3.6 Flash",
        tier: ModelTier::Supported,
        release_stage: ModelReleaseStage::Stable,
        model_family: "gemini-3",
        context_window: Some(1_048_576),
        max_input_tokens: None,
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
        anthropic_disabled_thinking: AnthropicDisabledThinking::Refused,
        supports_reasoning: false,
        effort_levels: GEMINI_MINIMAL_TO_HIGH,
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
        stream_inactivity_timeout_secs: None,
        supports_forced_tool_choice: true,
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
    //   - Release notes, 2026-10-08:
    //     https://ai.google.dev/gemini-api/docs/changelog
    //     (gemini-3.5-flash is deprecated; requests are routed to
    //      gemini-3.6-flash. The row stays so sessions that recorded this id
    //      keep resolving; the catalog has no deprecation field.)
    ModelCapabilities {
        id: "gemini-3.5-flash",
        provider: Provider::Gemini,
        display_name: "Gemini 3.5 Flash",
        tier: ModelTier::Supported,
        release_stage: ModelReleaseStage::Stable,
        model_family: "gemini-3",
        context_window: Some(1_048_576),
        max_input_tokens: None,
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
        anthropic_disabled_thinking: AnthropicDisabledThinking::Refused,
        supports_reasoning: false,
        effort_levels: GEMINI_MINIMAL_TO_HIGH,
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
        stream_inactivity_timeout_secs: None,
        supports_forced_tool_choice: true,
    },
    // Gemini 3.5 Flash-Lite
    //
    // Sources:
    //   - Model page:
    //     https://ai.google.dev/gemini-api/docs/models/gemini-3.5-flash-lite
    //     (stable; text, image, video, audio and PDF input, text output;
    //      input 1,048,576; output 65,536; structured outputs, function
    //      calling and Search grounding supported)
    //   - What's new in Gemini 3.6 Flash and 3.5 Flash-Lite:
    //     https://ai.google.dev/gemini-api/docs/whats-new-gemini-3.6
    //     (GA 2026-07-21; fastest, lowest-cost 3.5 model; default thinking
    //      level "minimal"; temperature, top_p and top_k are deprecated and
    //      ignored; thinking_level replaces thinking_budget)
    //   - Thinking:
    //     https://ai.google.dev/gemini-api/docs/thinking
    //     (levels minimal/low/medium/high)
    //
    // Live check 2026-10-14: models.get lists generateContent with
    // 1,048,576 input and 65,536 output tokens, and a streamGenerateContent
    // call answers with modelVersion gemini-3.5-flash-lite.
    ModelCapabilities {
        id: "gemini-3.5-flash-lite",
        provider: Provider::Gemini,
        display_name: "Gemini 3.5 Flash Lite",
        tier: ModelTier::Supported,
        release_stage: ModelReleaseStage::Stable,
        model_family: "gemini-3",
        context_window: Some(1_048_576),
        max_input_tokens: None,
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
        anthropic_disabled_thinking: AnthropicDisabledThinking::Refused,
        supports_reasoning: false,
        effort_levels: GEMINI_MINIMAL_TO_HIGH,
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
        stream_inactivity_timeout_secs: None,
        supports_forced_tool_choice: true,
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
        max_input_tokens: None,
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
        anthropic_disabled_thinking: AnthropicDisabledThinking::Refused,
        supports_reasoning: false,
        effort_levels: GEMINI_LOW_TO_HIGH,
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
        stream_inactivity_timeout_secs: None,
        supports_forced_tool_choice: true,
    },
    // Gemini 3.1 Flash-Lite
    //
    // Sources:
    //   - Model page:
    //     https://ai.google.dev/gemini-api/docs/models/gemini-3.1-flash-lite
    //     (stable, GA 2026-05-07; text, image, video, audio and PDF input,
    //      text output; input 1,048,576; output 65,536; structured outputs,
    //      function calling and Search grounding supported)
    //   - Gemini 3 guide:
    //     https://ai.google.dev/gemini-api/docs/gemini-3
    //     (thinking_level minimal (default)/low/medium/high; thinking_budget
    //      still accepted for backward compatibility; keep temperature at the
    //      1.0 default)
    //
    // Released before the 2026-07-21 sampling-parameter deprecation, which
    // applies from Gemini 3.6 Flash and 3.5 Flash-Lite on, so sampling
    // support mirrors the gemini-3.1-flash-lite-preview row (top_p/top_k
    // ASSUMED from that row; the pages do not state them). Live check
    // 2026-10-14: models.get lists generateContent with 1,048,576 input and
    // 65,536 output tokens, and a streamGenerateContent call answers with
    // modelVersion gemini-3.1-flash-lite. Google routes
    // gemini-3.1-flash-lite-preview requests here.
    ModelCapabilities {
        id: "gemini-3.1-flash-lite",
        provider: Provider::Gemini,
        display_name: "Gemini 3.1 Flash Lite",
        tier: ModelTier::Supported,
        release_stage: ModelReleaseStage::Stable,
        model_family: "gemini-3",
        context_window: Some(1_048_576),
        max_input_tokens: None,
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
        anthropic_disabled_thinking: AnthropicDisabledThinking::Refused,
        supports_reasoning: false,
        effort_levels: GEMINI_MINIMAL_TO_HIGH,
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
        stream_inactivity_timeout_secs: None,
        supports_forced_tool_choice: true,
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
        max_input_tokens: None,
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
        anthropic_disabled_thinking: AnthropicDisabledThinking::Refused,
        supports_reasoning: false,
        effort_levels: GEMINI_MINIMAL_TO_HIGH,
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
        stream_inactivity_timeout_secs: None,
        supports_forced_tool_choice: true,
    },
];
