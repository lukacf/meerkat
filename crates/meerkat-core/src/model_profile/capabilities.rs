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
use crate::model_profile::catalog::{ModelReleaseStage, ModelTier};
use serde::{Deserialize, Serialize};

/// GPT-5.6 Responses execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiReasoningMode {
    Standard,
    Pro,
}

impl OpenAiReasoningMode {
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Pro => "pro",
        }
    }
}

/// Which available reasoning items GPT-5.6 may render into the next sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiReasoningContext {
    Auto,
    CurrentTurn,
    AllTurns,
}

impl OpenAiReasoningContext {
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::CurrentTurn => "current_turn",
            Self::AllTurns => "all_turns",
        }
    }
}

/// Default level of detail for OpenAI text output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiTextVerbosity {
    Low,
    Medium,
    High,
}

impl OpenAiTextVerbosity {
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// Request-wide GPT-5.6 prompt-cache breakpoint policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiPromptCacheMode {
    Implicit,
    Explicit,
}

impl OpenAiPromptCacheMode {
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            Self::Implicit => "implicit",
            Self::Explicit => "explicit",
        }
    }
}

/// Minimum lifetime for GPT-5.6 prompt-cache entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum OpenAiPromptCacheTtl {
    // OpenAI's default lifetime; a cache write costs 1.25x the uncached
    // input rate.
    #[serde(rename = "30m")]
    ThirtyMinutes,
    // At least 12 hours (OpenAI may keep the entry up to 24 hours); a cache
    // write costs 2.5x the uncached input rate, reads cost the same. Only an
    // explicit choice: no catalog default selects it.
    #[serde(rename = "12h")]
    TwelveHours,
}

impl OpenAiPromptCacheTtl {
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            Self::ThirtyMinutes => "30m",
            Self::TwelveHours => "12h",
        }
    }
}

/// Accepted GPT-5.6-specific Responses parameter values for one model row.
#[derive(Debug, Clone, Copy)]
pub struct OpenAiResponsesParamCapabilities {
    pub reasoning_modes: &'static [OpenAiReasoningMode],
    pub reasoning_contexts: &'static [OpenAiReasoningContext],
    pub text_verbosity_levels: &'static [OpenAiTextVerbosity],
    pub prompt_cache_modes: &'static [OpenAiPromptCacheMode],
    /// The prompt-cache mode Meerkat requests by default on the OpenAI API
    /// when the caller sets none. Must be one of `prompt_cache_modes`.
    pub default_prompt_cache_mode: Option<OpenAiPromptCacheMode>,
    pub prompt_cache_ttls: &'static [OpenAiPromptCacheTtl],
    /// Whether deprecated `prompt_cache_retention: "in_memory"` is accepted.
    /// GPT-5.6 and later rows reject it: their cache lifetime is
    /// `prompt_cache_options.ttl`.
    pub supports_in_memory_prompt_cache_retention: bool,
}

impl OpenAiResponsesParamCapabilities {
    /// The row's default prompt-cache mode, if it names one it accepts.
    pub fn default_prompt_cache_mode(&self) -> Option<OpenAiPromptCacheMode> {
        self.default_prompt_cache_mode
            .filter(|mode| self.prompt_cache_modes.contains(mode))
    }

    /// Why `model` (this row) rejects the deprecated `prompt_cache_retention`
    /// value, or `None` when it accepts it.
    pub fn prompt_cache_retention_rejection(
        &self,
        model: &str,
        retention: crate::lifecycle::run_primitive::OpenAiPromptCacheRetention,
    ) -> Option<String> {
        use crate::lifecycle::run_primitive::OpenAiPromptCacheRetention;
        match retention {
            OpenAiPromptCacheRetention::InMemory
                if !self.supports_in_memory_prompt_cache_retention =>
            {
                let ttls = self
                    .prompt_cache_ttls
                    .iter()
                    .map(|ttl| format!("'{}'", ttl.as_wire_str()))
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(if ttls.is_empty() {
                    format!(
                        "OpenAI model '{model}' does not accept prompt_cache_retention 'in_memory'"
                    )
                } else {
                    format!(
                        "OpenAI model '{model}' does not accept prompt_cache_retention \
                         'in_memory'; set its prompt cache lifetime with \
                         prompt_cache_options.ttl ({ttls})"
                    )
                })
            }
            OpenAiPromptCacheRetention::InMemory | OpenAiPromptCacheRetention::TwentyFourHours => {
                None
            }
        }
    }
}

/// Full per-model capability record.
///
/// Fields group into:
/// - identity (`id`, `provider`, `display_name`, `tier`, `model_family`)
/// - context/input/output (`context_window`, `max_input_tokens`, `max_output_tokens`,
///   plus `_beta` variants)
/// - modalities (`vision`, `image_tool_results`, `inline_video`, `realtime`,
///   `image_generation`)
/// - realtime transport capability facts (`realtime_supports_provider_managed_turns`,
///   `realtime_supports_explicit_commit`, `realtime_interrupt_supported`,
///   `realtime_transcript_supported`, `transcription_companion_model`) — what
///   the model's realtime bidirectional transport can actually do; only
///   meaningful when `realtime` is true
/// - sampling (`supports_temperature`, `supports_top_p`, `supports_top_k`)
/// - reasoning (`thinking`, `supports_reasoning`, `effort_levels`,
///   `openai_responses_params`)
/// - features (`supports_web_search`, `supports_inference_geo`,
///   `supports_compaction`, `supports_structured_output`,
///   `supports_legacy_penalties`, `supports_thinking_budget_legacy`)
/// - metadata (`beta_headers`, `call_timeout_secs`,
///   `stream_inactivity_timeout_secs`)
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
    /// Release maturity and admission class.
    pub release_stage: ModelReleaseStage,
    /// Model family identifier (a stable grouping key for related model ids).
    pub model_family: &'static str,

    // ── Context / output ──────────────────────────────────────────────
    /// Shared input-plus-output context window in tokens (default, no beta).
    pub context_window: Option<u32>,
    /// Separate maximum input tokens, if the model declares an input ceiling.
    pub max_input_tokens: Option<u32>,
    /// Maximum output tokens per response (default, no beta).
    pub max_output_tokens: Option<u32>,
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
    /// Whether, and at which effort, the Anthropic API accepts
    /// `thinking: {"type": "disabled"}` for this model: a per-row fact kept
    /// apart from [`Self::thinking`]. `Refused` on every non-Anthropic row
    /// and on any row whose provider documentation does not show it
    /// accepted.
    pub anthropic_disabled_thinking: AnthropicDisabledThinking,
    /// Whether the model supports explicit reasoning effort control
    /// (OpenAI's `reasoning.effort`, distinct from Anthropic `output_config.effort`).
    pub supports_reasoning: bool,
    /// Accepted values for effort control. Empty slice = unsupported.
    /// Applies to both Anthropic `output_config.effort` and OpenAI
    /// `reasoning.effort`; the two schemas live in different request shapes but
    /// share the typed [`EffortLevel`] vocabulary. On Gemini thinking-level
    /// rows it lists the `thinkingConfig.thinkingLevel` values the model
    /// accepts; an empty slice there records none, and the params schema then
    /// advertises every Gemini level.
    pub effort_levels: &'static [EffortLevel],
    /// GPT-5.6-specific Responses parameter vocabularies. `None` means the row
    /// does not advertise these newer controls.
    pub openai_responses_params: Option<OpenAiResponsesParamCapabilities>,

    // ── Features ──────────────────────────────────────────────────────
    /// Provider-native web search tool support.
    pub supports_web_search: bool,
    /// Whether the model accepts ordered System messages inside conversation
    /// history after the leading system-prompt prefix.
    pub supports_mid_conversation_system_messages: bool,
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

    /// Whether the model accepts a tool choice that forces a call (`any` /
    /// `required` / a named tool). `false` only for models proven to reject
    /// one (Claude Opus 5.5 answers 400 "not supported for this model"); the
    /// lowering then refuses locally with a typed error. Unproven models are
    /// `true`: the call is attempted and a provider rejection maps to the
    /// same typed error.
    pub supports_forced_tool_choice: bool,

    // ── Runtime ───────────────────────────────────────────────────────
    /// Authoritative default call timeout in seconds for this model.
    /// `None` means the model has no profiled default (unknown family).
    pub call_timeout_secs: Option<u64>,
    /// Meerkat-owned stream-inactivity watchdog window in seconds for this
    /// model: how long its provider stream may stay silent before the agent
    /// loop aborts the call as stalled. `None` keeps the global window
    /// (`[retry] stream_inactivity_timeout`, 300 s by default). Rows set it
    /// where the provider documents responses that can stay silent for
    /// minutes. A set window must be at least
    /// [`crate::MIN_MODEL_STREAM_INACTIVITY_TIMEOUT`]; config load does not
    /// check catalog rows, so the catalog's owner does.
    pub stream_inactivity_timeout_secs: Option<u64>,
}

impl ModelCapabilities {
    /// Why this Anthropic row refuses the request-shaping knobs in `tag`, or
    /// `None` when the provider would accept them. Checked locally, before
    /// the provider call, so an unsupported explicit setting is a typed
    /// refusal and never a provider 400.
    ///
    /// Covers what the Anthropic lowering would actually send: the
    /// `thinking` type, the legacy flat `thinking_budget_tokens` (sent as
    /// `enabled` when `thinking` is unset), `top_k`, and a thinking type the
    /// model accepts only at `high` effort or below (`between_tools`, and
    /// `disabled` on some rows) with an effort above `high`. Generic
    /// `temperature` is not refused here: the lowering omits it for models
    /// without temperature support.
    pub fn anthropic_provider_tag_rejection(
        &self,
        tag: &crate::lifecycle::run_primitive::AnthropicProviderTag,
    ) -> Option<String> {
        use crate::lifecycle::run_primitive::{AnthropicEffort, AnthropicThinkingConfig};
        let model = self.id;
        let accepted = || self.anthropic_thinking_wire_types().join(", ");
        match tag.thinking.as_ref() {
            Some(thinking) if !self.accepts_anthropic_thinking(thinking) => {
                return Some(format!(
                    "Anthropic model '{model}' does not accept thinking type '{}'; accepted: [{}]",
                    thinking.wire_type(),
                    accepted()
                ));
            }
            Some(thinking)
                if self.anthropic_thinking_capped_at_high_effort(thinking)
                    && matches!(
                        tag.effort,
                        Some(AnthropicEffort::XHigh | AnthropicEffort::Max)
                    ) =>
            {
                return Some(format!(
                    "Anthropic model '{model}' accepts thinking type '{}' only at \
                     'high' effort or below; lower the effort to 'high' or use adaptive \
                     thinking for effort '{}'",
                    thinking.wire_type(),
                    tag.effort
                        .map(AnthropicEffort::as_legacy_str)
                        .unwrap_or_default()
                ));
            }
            Some(_) => {}
            None => {
                if tag.thinking_budget_tokens.is_some()
                    && !self.thinking.accepts_anthropic_thinking(
                        &AnthropicThinkingConfig::Enabled { budget_tokens: 0 },
                    )
                {
                    return Some(format!(
                        "Anthropic model '{model}' does not accept a thinking budget \
                         (thinking_budget_tokens); accepted thinking types: [{}]",
                        accepted()
                    ));
                }
            }
        }
        if tag.top_k.is_some() && !self.supports_top_k {
            return Some(format!("Anthropic model '{model}' does not accept top_k"));
        }
        None
    }

    /// Whether an Anthropic request may carry `thinking` = `config` for this
    /// model at some effort: what [`Self::thinking`] admits, plus `disabled`
    /// where [`Self::anthropic_disabled_thinking`] admits it.
    pub fn accepts_anthropic_thinking(
        &self,
        config: &crate::lifecycle::run_primitive::AnthropicThinkingConfig,
    ) -> bool {
        use crate::lifecycle::run_primitive::AnthropicThinkingConfig as Thinking;
        match config {
            Thinking::Disabled => {
                self.anthropic_disabled_thinking != AnthropicDisabledThinking::Refused
            }
            Thinking::Adaptive | Thinking::Enabled { .. } | Thinking::BetweenTools => {
                self.thinking.accepts_anthropic_thinking(config)
            }
        }
    }

    /// Whether this model accepts thinking `config` only at `high` effort or
    /// below, answering 400 for `xhigh` and `max`: `between_tools`, and
    /// `disabled` on rows that record that ceiling.
    pub fn anthropic_thinking_capped_at_high_effort(
        &self,
        config: &crate::lifecycle::run_primitive::AnthropicThinkingConfig,
    ) -> bool {
        use crate::lifecycle::run_primitive::AnthropicThinkingConfig as Thinking;
        match config {
            Thinking::BetweenTools => true,
            Thinking::Disabled => {
                self.anthropic_disabled_thinking == AnthropicDisabledThinking::AtOrBelowHighEffort
            }
            Thinking::Adaptive | Thinking::Enabled { .. } => false,
        }
    }

    /// The Anthropic thinking `type` values this model accepts, for messages
    /// and parameter schemas.
    pub fn anthropic_thinking_wire_types(&self) -> Vec<&'static str> {
        let mut types = self.thinking.anthropic_thinking_wire_types().to_vec();
        if self.anthropic_disabled_thinking != AnthropicDisabledThinking::Refused {
            types.push(
                crate::lifecycle::run_primitive::AnthropicThinkingConfig::Disabled.wire_type(),
            );
        }
        types
    }

    /// Why this Gemini row refuses the thinking level in `tag`, or `None`
    /// when the provider would accept it. Checked locally, before the
    /// provider call, so an unsupported explicit level is a typed refusal and
    /// never a provider 400 (Gemini answers "Thinking level MINIMAL is not
    /// supported for this model" for `minimal` on a model that refuses it).
    ///
    /// Covers the level the Gemini lowering sends: the nested
    /// `thinking.thinking_level`, else the flat `thinking_level`. A
    /// thinking-level row refuses a level outside its `effort_levels`; a row
    /// that records no levels accepts every level.
    pub fn gemini_provider_tag_rejection(
        &self,
        tag: &crate::lifecycle::run_primitive::GeminiProviderTag,
    ) -> Option<String> {
        let level = tag
            .thinking
            .as_ref()
            .and_then(|thinking| thinking.thinking_level)
            .or(tag.thinking_level)?;
        if self.thinking != ThinkingSupport::GeminiThinkingLevel
            || self.effort_levels.is_empty()
            || self.effort_levels.contains(&level.as_effort_level())
        {
            return None;
        }
        let accepted = self
            .effort_levels
            .iter()
            .map(|level| level.as_wire_str())
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!(
            "Gemini model '{}' does not accept thinking level '{}'; accepted: [{accepted}]",
            self.id,
            level.as_str()
        ))
    }
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
    /// Anthropic: `{type: "adaptive"}` (the default) or `{type: "between_tools"}`,
    /// which turns off up-front thinking at `high` effort or below.
    /// `disabled` and `enabled` (budget) return 400.
    AnthropicAdaptiveOrBetweenTools,
    /// Gemini 3.x: `generationConfig.thinkingConfig.thinking_level`.
    /// Legacy `thinking_budget` is also accepted when
    /// `supports_thinking_budget_legacy = true`.
    GeminiThinkingLevel,
}

impl ThinkingSupport {
    /// Whether an Anthropic request may carry `thinking` = `config` for a
    /// model with this support mode. `disabled` is a per-row fact
    /// ([`ModelCapabilities::anthropic_disabled_thinking`]) that this
    /// mode-level check never admits; use
    /// [`ModelCapabilities::accepts_anthropic_thinking`] for a model.
    pub fn accepts_anthropic_thinking(
        self,
        config: &crate::lifecycle::run_primitive::AnthropicThinkingConfig,
    ) -> bool {
        use crate::lifecycle::run_primitive::AnthropicThinkingConfig as Thinking;
        match (self, config) {
            (Self::AnthropicEnabledOnly, Thinking::Enabled { .. })
            | (Self::AnthropicAdaptiveOnly, Thinking::Adaptive)
            | (Self::AnthropicAdaptiveAndEnabled, Thinking::Adaptive | Thinking::Enabled { .. })
            | (
                Self::AnthropicAdaptiveOrBetweenTools,
                Thinking::Adaptive | Thinking::BetweenTools,
            ) => true,
            (
                Self::None
                | Self::GeminiThinkingLevel
                | Self::AnthropicEnabledOnly
                | Self::AnthropicAdaptiveOnly
                | Self::AnthropicAdaptiveAndEnabled
                | Self::AnthropicAdaptiveOrBetweenTools,
                _,
            ) => false,
        }
    }

    /// The Anthropic thinking `type` values this mode accepts, for messages.
    /// Leaves out `disabled`, a per-row fact; see
    /// [`ModelCapabilities::anthropic_thinking_wire_types`].
    pub const fn anthropic_thinking_wire_types(self) -> &'static [&'static str] {
        match self {
            Self::AnthropicEnabledOnly => &["enabled"],
            Self::AnthropicAdaptiveOnly => &["adaptive"],
            Self::AnthropicAdaptiveAndEnabled => &["adaptive", "enabled"],
            Self::AnthropicAdaptiveOrBetweenTools => &["adaptive", "between_tools"],
            Self::None | Self::GeminiThinkingLevel => &[],
        }
    }
}

/// Whether an Anthropic model accepts `thinking: {"type": "disabled"}`, and
/// at which effort. Recorded per catalog row from the provider's per-model
/// thinking table; a row the documentation does not cover stays `Refused`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnthropicDisabledThinking {
    /// `disabled` returns 400: thinking cannot be turned off, or the row is
    /// not an Anthropic model.
    Refused,
    /// `disabled` is accepted at `high` effort or below; with `xhigh` or
    /// `max` it returns 400. A request that sets no effort runs at the
    /// model's default, so a row records this only when that default is
    /// `high` or below.
    AtOrBelowHighEffort,
    /// `disabled` is accepted at every effort the model supports.
    AnyEffort,
}

/// Reasoning/effort control level, the shared typed vocabulary behind both
/// Anthropic `output_config.effort` and OpenAI `reasoning.effort`.
///
/// The two providers expose effort in different request shapes but draw from
/// the same level vocabulary; modeling it as a typed enum keeps the catalog
/// value-domain compiler-checked instead of relying on raw string literals.
/// Each catalog row declares its accepted subset via `effort_levels`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum EffortLevel {
    /// OpenAI `reasoning.effort: "none"` - reasoning disabled.
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
    /// Maximum reasoning effort (Anthropic and GPT-5.6 top tier).
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

    /// The level a wire string names, the inverse of [`Self::as_wire_str`].
    pub fn from_wire_str(value: &str) -> Option<Self> {
        Some(match value {
            "none" => EffortLevel::None,
            "minimal" => EffortLevel::Minimal,
            "low" => EffortLevel::Low,
            "medium" => EffortLevel::Medium,
            "high" => EffortLevel::High,
            "xhigh" => EffortLevel::Xhigh,
            "max" => EffortLevel::Max,
            _ => return None,
        })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
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

    #[test]
    fn openai_responses_parameter_wire_values_are_pinned() {
        assert_eq!(OpenAiReasoningMode::Standard.as_wire_str(), "standard");
        assert_eq!(OpenAiReasoningMode::Pro.as_wire_str(), "pro");
        assert_eq!(OpenAiReasoningContext::Auto.as_wire_str(), "auto");
        assert_eq!(
            OpenAiReasoningContext::CurrentTurn.as_wire_str(),
            "current_turn"
        );
        assert_eq!(OpenAiReasoningContext::AllTurns.as_wire_str(), "all_turns");
        assert_eq!(OpenAiTextVerbosity::High.as_wire_str(), "high");
        assert_eq!(OpenAiPromptCacheMode::Explicit.as_wire_str(), "explicit");
        assert_eq!(OpenAiPromptCacheTtl::ThirtyMinutes.as_wire_str(), "30m");
        assert_eq!(OpenAiPromptCacheTtl::TwelveHours.as_wire_str(), "12h");
    }

    /// The serde form of every prompt-cache TTL is the wire string the
    /// OpenAI lowering and the params schema use, in both directions.
    #[test]
    fn openai_prompt_cache_ttl_serde_form_is_its_wire_string() {
        for ttl in [
            OpenAiPromptCacheTtl::ThirtyMinutes,
            OpenAiPromptCacheTtl::TwelveHours,
        ] {
            let json = serde_json::to_value(ttl).expect("serialize ttl");
            assert_eq!(json, serde_json::json!(ttl.as_wire_str()));
            let parsed: OpenAiPromptCacheTtl =
                serde_json::from_value(json).expect("deserialize ttl");
            assert_eq!(parsed, ttl);
        }
        assert!(
            serde_json::from_value::<OpenAiPromptCacheTtl>(serde_json::json!("24h")).is_err(),
            "24h is prompt_cache_retention, not a prompt_cache_options.ttl value"
        );
    }

    #[test]
    fn gemini_rows_refuse_only_the_thinking_levels_they_do_not_record() {
        use crate::lifecycle::run_primitive::{
            GeminiProviderTag, GeminiThinkingConfig, GeminiThinkingLevel,
        };
        use crate::model_profile::test_catalog::{TEST_CATALOG, VIDEO_MODEL};
        let unrecorded = *TEST_CATALOG
            .capabilities_for(crate::Provider::Gemini, VIDEO_MODEL)
            .expect("test Gemini row");
        let low_to_high = ModelCapabilities {
            effort_levels: &[EffortLevel::Low, EffortLevel::Medium, EffortLevel::High],
            ..unrecorded
        };
        let flat = |level| GeminiProviderTag {
            thinking_level: Some(level),
            ..Default::default()
        };
        let nested = |level| GeminiProviderTag {
            thinking: Some(GeminiThinkingConfig {
                include_thoughts: None,
                thinking_level: Some(level),
                thinking_budget: None,
            }),
            ..Default::default()
        };

        let refusal = low_to_high
            .gemini_provider_tag_rejection(&flat(GeminiThinkingLevel::Minimal))
            .expect("minimal is outside the row");
        assert!(refusal.contains("'minimal'"), "{refusal}");
        assert!(refusal.contains("[low, medium, high]"), "{refusal}");
        assert!(
            low_to_high
                .gemini_provider_tag_rejection(&nested(GeminiThinkingLevel::Minimal))
                .is_some()
        );
        // The nested level is the one sent, so it is the one judged.
        let mut nested_wins = nested(GeminiThinkingLevel::Low);
        nested_wins.thinking_level = Some(GeminiThinkingLevel::Minimal);
        assert!(
            low_to_high
                .gemini_provider_tag_rejection(&nested_wins)
                .is_none()
        );
        assert!(
            low_to_high
                .gemini_provider_tag_rejection(&flat(GeminiThinkingLevel::High))
                .is_none()
        );
        assert!(
            low_to_high
                .gemini_provider_tag_rejection(&GeminiProviderTag::default())
                .is_none()
        );
        // A row that records no levels accepts every level.
        assert!(
            unrecorded
                .gemini_provider_tag_rejection(&flat(GeminiThinkingLevel::Minimal))
                .is_none()
        );
    }

    #[test]
    fn anthropic_disabled_thinking_follows_the_row_and_its_effort_ceiling() {
        use crate::lifecycle::run_primitive::{
            AnthropicEffort, AnthropicProviderTag, AnthropicThinkingConfig,
        };
        use crate::model_profile::test_catalog::{ANTHROPIC_MODEL, TEST_CATALOG};
        let row = *TEST_CATALOG
            .capabilities_for(crate::Provider::Anthropic, ANTHROPIC_MODEL)
            .expect("test Anthropic row");
        let with = |disabled| ModelCapabilities {
            anthropic_disabled_thinking: disabled,
            ..row
        };
        let disabled_at = |effort| AnthropicProviderTag {
            thinking: Some(AnthropicThinkingConfig::Disabled),
            effort,
            ..Default::default()
        };
        let every_effort = [
            None,
            Some(AnthropicEffort::Low),
            Some(AnthropicEffort::Medium),
            Some(AnthropicEffort::High),
            Some(AnthropicEffort::XHigh),
            Some(AnthropicEffort::Max),
        ];

        let refused = with(AnthropicDisabledThinking::Refused);
        for effort in every_effort {
            let refusal = refused
                .anthropic_provider_tag_rejection(&disabled_at(effort))
                .expect("a refusing row refuses disabled at every effort");
            assert!(
                refusal.contains("does not accept thinking type 'disabled'; accepted: [adaptive]"),
                "{refusal}"
            );
        }
        assert_eq!(refused.anthropic_thinking_wire_types(), ["adaptive"]);

        let capped = with(AnthropicDisabledThinking::AtOrBelowHighEffort);
        for effort in &every_effort[..4] {
            assert_eq!(
                capped.anthropic_provider_tag_rejection(&disabled_at(*effort)),
                None,
                "{effort:?}"
            );
        }
        for effort in [AnthropicEffort::XHigh, AnthropicEffort::Max] {
            let refusal = capped
                .anthropic_provider_tag_rejection(&disabled_at(Some(effort)))
                .expect("disabled above high effort is refused");
            assert!(
                refusal.contains("accepts thinking type 'disabled' only at 'high' effort or below"),
                "{refusal}"
            );
            assert!(
                refusal.contains(&format!(
                    "lower the effort to 'high' or use adaptive thinking for effort '{}'",
                    effort.as_legacy_str()
                )),
                "{refusal}"
            );
        }
        assert_eq!(
            capped.anthropic_thinking_wire_types(),
            ["adaptive", "disabled"]
        );

        let any = with(AnthropicDisabledThinking::AnyEffort);
        for effort in every_effort {
            assert_eq!(
                any.anthropic_provider_tag_rejection(&disabled_at(effort)),
                None,
                "{effort:?}"
            );
        }
        assert!(!any.anthropic_thinking_capped_at_high_effort(&AnthropicThinkingConfig::Disabled));
        // The mode-level check never admits disabled; the row does.
        assert!(
            !any.thinking
                .accepts_anthropic_thinking(&AnthropicThinkingConfig::Disabled)
        );
        assert!(any.accepts_anthropic_thinking(&AnthropicThinkingConfig::Disabled));
        // Disabled thinking changes nothing for the other thinking types.
        for disabled in [
            AnthropicDisabledThinking::Refused,
            AnthropicDisabledThinking::AtOrBelowHighEffort,
            AnthropicDisabledThinking::AnyEffort,
        ] {
            let caps = with(disabled);
            assert!(caps.accepts_anthropic_thinking(&AnthropicThinkingConfig::Adaptive));
            assert!(!caps.accepts_anthropic_thinking(&AnthropicThinkingConfig::BetweenTools));
            assert!(
                !caps.accepts_anthropic_thinking(&AnthropicThinkingConfig::Enabled {
                    budget_tokens: 1024
                })
            );
        }
    }
}
