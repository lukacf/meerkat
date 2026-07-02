//! §18 Run primitives — the ONLY input core receives from the runtime layer.
//!
//! Core's entire world is: conversation mutations, run boundaries, and staged inputs.
//! It knows nothing about input acceptance, policy, queueing, or topology.

use serde::de::{self, DeserializeOwned};
use serde::{Deserialize, Serialize};

use super::identifiers::InputId;
use crate::connection::AuthBindingRef;
use crate::provider::Provider;
use crate::service::TurnToolOverlay;
use crate::skills::SkillKey;
use crate::types::{
    HandlingMode, RenderMetadata, SystemNoticeBlock, SystemNoticeKind, TranscriptMessageIdentity,
};

/// When to apply a conversation mutation relative to the run lifecycle.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunApplyBoundary {
    /// Apply immediately (no run boundary required).
    Immediate,
    /// Apply at the start of the next run.
    RunStart,
    /// Apply at the next checkpoint within a run.
    RunCheckpoint,
}

/// Renderable content that can be appended to a conversation.
#[non_exhaustive]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CoreRenderable {
    /// Plain text content.
    Text { text: String },
    /// Multimodal content blocks (text + images).
    Blocks {
        blocks: Vec<crate::types::ContentBlock>,
    },
    /// JSON-structured content. Uses `Value` because the runtime layer constructs
    /// these from various typed sources (peer messages, external events) and core
    /// needs to render them into conversation messages — not a pass-through boundary.
    Json { value: serde_json::Value },
    /// Typed runtime-authored system notice content.
    SystemNotice {
        kind: SystemNoticeKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        blocks: Vec<SystemNoticeBlock>,
    },
    /// Reference to an external artifact.
    Reference { uri: String, label: Option<String> },
}

impl CoreRenderable {
    /// Construct a plain-text renderable.
    ///
    /// Convenience constructor for callers that only carry a `String` body
    /// (the common runtime/system-context append case). Richer producers build
    /// the `Blocks` / `SystemNotice` / `Json` / `Reference` variants directly.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Render this content to its canonical plain-text projection.
    ///
    /// This is the single owner of the renderable -> text lowering used by
    /// system-context surfaces; callers must not re-implement per-variant
    /// flattening. Non-text variants project to their model-facing text form
    /// (multimodal blocks collapse to their text, JSON pretty-prints, a
    /// reference renders a `[Reference] ...` line, a system notice renders its
    /// model-projection text).
    #[must_use]
    pub fn render_text(&self) -> String {
        match self {
            Self::Text { text } => text.clone(),
            Self::Blocks { blocks } => crate::types::text_content(blocks),
            Self::Json { value } => {
                serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
            }
            Self::Reference { uri, label } => match label {
                Some(label) if !label.trim().is_empty() => format!("[Reference] {label} ({uri})"),
                _ => format!("[Reference] {uri}"),
            },
            Self::SystemNotice { kind, body, blocks } => {
                crate::types::SystemNoticeMessage::with_blocks(*kind, body.clone(), blocks.clone())
                    .model_projection_text()
            }
        }
    }
}

/// Which role to append to in the conversation.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationAppendRole {
    /// User message.
    User,
    /// Assistant message.
    Assistant,
    /// System notice (injected context).
    SystemNotice,
    /// Tool result.
    Tool,
    /// Host-attached injected context on the user channel.
    ///
    /// Lowers into a `Message::User` carrying
    /// [`crate::types::TranscriptUserRole::InjectedContext`], so the typed
    /// slot the content arrived in — not free-form role strings — mints the
    /// transcript role.
    InjectedContext,
}

/// A single conversation append operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationAppend {
    /// The role for this message.
    pub role: ConversationAppendRole,
    /// The content to append.
    pub content: CoreRenderable,
}

/// A context-only append (system context, not user-facing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationContextAppend {
    /// Key for deduplication/replacement.
    pub key: String,
    /// The context content.
    pub content: CoreRenderable,
}

/// Typed execution intent classified by the runtime layer.
///
/// The runtime stamps this on `RuntimeTurnMetadata` so the session layer can
/// dispatch `run_turn` vs `run_pending` from typed intent rather than inferring
/// from prompt emptiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeExecutionKind {
    /// Ordinary content turn: prompts, peer messages/requests/terminal-responses,
    /// external events, flow steps.
    ContentTurn,
    /// Explicit continuation that resumes pending work at a boundary.
    ResumePending,
}

/// Machine-owned apply intent for terminal peer responses.
///
/// Terminal peer responses are context facts and requester wake/reaction work.
/// This closed intent prevents context-only executor shortcuts from inferring a
/// different meaning from the primitive's append shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerResponseTerminalApplyIntent {
    /// Append the durable system-context fact, then run the requester reaction
    /// turn using the appended context.
    AppendContextAndRun,
}

/// Opaque model identifier carried by a per-turn override.
///
/// A bare string here is a failure of the typed-metadata invariant: validation
/// against the catalog happens at the runtime boundary before `ModelId` is
/// constructed. Construct via [`ModelId::new`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelId(String);

impl ModelId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ModelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Keep-alive policy for a materialized session during a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeepAlivePolicy {
    #[serde(with = "duration_seconds")]
    pub ttl: std::time::Duration,
    pub policy: KeepAliveMode,
}

/// Keep-alive mode: pinned (caller-owned) or policy-driven (runtime sweeps).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeepAliveMode {
    Pinned,
    PolicyDriven,
}

/// Per-turn keep-alive directive carried on [`RuntimeTurnMetadata`].
///
/// Together with the carrier's `Option`, this is the typed tri-state the
/// generated `RuntimeKeepAliveRequest` admission input models:
/// `Some(Enable(policy))` -> `Enable`, `Some(Disable)` -> `Disable` (explicit
/// operator intent to turn keep-alive off), and `None` -> `Preserve` (the
/// session's existing keep-alive stands unchanged). The former
/// `Option<KeepAlivePolicy>` carrier had no `Disable` representation, so a
/// caller's `keep_alive: false` was silently collapsed into `Preserve`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "directive", rename_all = "snake_case")]
pub enum KeepAliveDirective {
    /// Enable keep-alive for the session with the given policy.
    Enable(KeepAlivePolicy),
    /// Explicitly disable keep-alive for the session.
    Disable,
}

/// Single additional instruction attached to a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnInstruction {
    pub kind: TurnInstructionKind,
    pub body: String,
}

/// Typed category of [`TurnInstruction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnInstructionKind {
    User,
    System,
    Host,
}

/// Typed non-semantic opaque bag for per-turn provider knobs that cannot be
/// fully typed without blocking a wave boundary. Explicitly marked
/// non-semantic and RMAT-exempt.
///
/// Use of this type is a deliberate boundary marker: content is passed
/// through without interpretation. Any consumer that needs to interpret the
/// content must promote the relevant structure into a proper typed variant
/// in its own wave.
///
/// Relocated from `meerkat_contracts::wire::runtime` into core so
/// `ProviderTag::Unknown { bag }` can name the bag without a cross-crate
/// cycle (adversarial review flaw 5). `meerkat-contracts` re-exports this
/// type so the wire path is preserved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StructuredProviderExtension {
    /// Free-form provider namespace discriminator (e.g. `"anthropic"`).
    pub namespace: String,
    /// Opaque key identifying the extension within the namespace.
    pub key: String,
    /// Opaque body. Non-semantic — never pattern matched across the wire.
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    #[serde(default)]
    pub body: String,
}

/// Provider-specific typed override payload carried on a single turn.
///
/// Each provider family gets its own typed variant. Anything that does not
/// fit a typed field belongs on the per-binding auth/backend profile, not
/// on the per-turn override — the per-turn seam carries only scalars the
/// runtime can route authoritatively.
///
/// `Unknown { bag }` is the typed escape hatch for V3 legacy-row
/// deserialize (see C-TM-V3): the untyped `serde_json::Value` thinking
/// carrier from pre-wave rows projects into `StructuredProviderExtension`
/// rather than being silently dropped (persistence-migration.md §3.1,
/// adversarial review flaw 5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum ProviderTag {
    Anthropic(AnthropicProviderTag),
    OpenAi(OpenAiProviderTag),
    Gemini(GeminiProviderTag),
    /// Opaque pass-through for legacy-row knobs that don't (yet) map to a
    /// typed variant. Carries the namespaced bag so a later wave can
    /// promote the structure to a typed variant without losing data.
    Unknown {
        bag: StructuredProviderExtension,
    },
}

/// Opaque provider-native JSON body carried verbatim from caller to
/// provider. Used for pass-through sub-shapes (web search config,
/// provider-native custom compaction edits, OpenAI-compatible
/// `chat_template_kwargs`/`thinking`/`reasoning` forwards) where the
/// exact wire shape varies across downstream providers (Anthropic /
/// DeepSeek / OpenRouter / custom proxies) and the runtime deliberately
/// does not parse the body — it simply forwards it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct OpaqueProviderBody {
    /// Serialized JSON body. Callers that have a `serde_json::Value`
    /// should use [`OpaqueProviderBody::from_value`]. Providers reading
    /// the body call [`OpaqueProviderBody::as_value`] to recover a
    /// `Value` for wire emission.
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub body: String,
}

impl OpaqueProviderBody {
    pub fn from_value(v: &serde_json::Value) -> Self {
        Self {
            body: v.to_string(),
        }
    }

    pub fn as_value(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

/// Typed shape of Anthropic's extended-thinking knob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicThinkingConfig {
    /// Adaptive thinking — provider picks the budget.
    Adaptive,
    /// Explicit budget — model emits at most `budget_tokens` tokens of
    /// reasoning before the assistant text.
    Enabled { budget_tokens: u32 },
}

/// Typed shape of Anthropic's response-effort knob.
/// `XHigh` is the Opus 4.8 / 4.7 extended-high effort level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AnthropicEffort {
    Low,
    Medium,
    High,
    Max,
    XHigh,
}

impl AnthropicEffort {
    pub fn as_legacy_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Max => "max",
            Self::XHigh => "xhigh",
        }
    }
}

/// Typed shape of Anthropic's data-residency knob.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AnthropicInferenceGeo {
    Us,
    Global,
    /// Caller-provided region string — providers may accept region codes
    /// this typed variant does not yet enumerate.
    Other {
        region: String,
    },
}

/// Typed shape of Anthropic's context-window opt-in.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnthropicContextWindow {
    /// 1M-token beta context window (2025-08-07 beta header).
    OneMegabyte,
}

/// Typed shape of Anthropic's automatic-compaction knob.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AnthropicCompactionConfig {
    /// `"auto"` — provider picks trigger and instructions.
    Auto,
    /// Caller-provided edit body merged into the compact edit shape.
    /// Fields like `trigger` / `instructions` are preserved verbatim.
    Custom { edit: OpaqueProviderBody },
}

/// Typed shape of Anthropic's prompt-cache breakpoint policy.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnthropicCacheControlPolicy {
    /// Do not request Anthropic prompt-cache breakpoints.
    Disabled,
    /// Mark the stable top-level system prompt as an ephemeral cache prefix.
    SystemPrefix,
}

/// Per-turn Anthropic-specific knobs carried in `ProviderTag::Anthropic`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AnthropicProviderTag {
    /// Extended-thinking configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<AnthropicThinkingConfig>,
    /// Legacy flat `thinking_budget_tokens` — preserved for V3
    /// persistence round-trip (single-key legacy projector) and for
    /// callers that cannot express the full `thinking` shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_budget_tokens: Option<u32>,
    /// Provider-native web-search tool body, injected alongside
    /// `tools` at the provider-runtime boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_search: Option<OpaqueProviderBody>,
    /// Override top-k sampling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Response-effort knob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<AnthropicEffort>,
    /// Structured-output schema (forces JSON-schema output envelope).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<crate::OutputSchema>,
    /// Data-residency override for inference routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inference_geo: Option<AnthropicInferenceGeo>,
    /// Automatic compaction configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<AnthropicCompactionConfig>,
    /// Context-window opt-in (1M beta).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<AnthropicContextWindow>,
    /// Prompt-cache breakpoint policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<AnthropicCacheControlPolicy>,
    /// Internal override: force-enable temperature for this request even
    /// when the model profile says unsupported. Used by proxied /
    /// custom deployments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_temperature_override: Option<bool>,
}

/// Typed shape of OpenAI's prompt-cache retention hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum OpenAiPromptCacheRetention {
    /// Retain only in memory.
    InMemory,
    /// Retain for 24 hours.
    #[serde(rename = "24h")]
    TwentyFourHours,
}

/// Per-turn OpenAI-specific knobs carried in `ProviderTag::OpenAi`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct OpenAiProviderTag {
    /// Reasoning-effort level for o-series and GPT-5 models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Deterministic-sampling seed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// Frequency penalty (-2.0 .. 2.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    /// Presence penalty (-2.0 .. 2.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    /// Provider-native web-search tool body, injected alongside `tools`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_search: Option<OpaqueProviderBody>,
    /// Structured-output schema (forces `text.format.json_schema`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<crate::OutputSchema>,
    /// OpenAI-compatible endpoints (DeepSeek / OpenRouter / vLLM):
    /// full `reasoning` body forwarded verbatim alongside
    /// `reasoning_effort`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<OpaqueProviderBody>,
    /// OpenAI-compatible endpoints: `chat_template_kwargs` passthrough.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_template_kwargs: Option<OpaqueProviderBody>,
    /// OpenAI-compatible endpoints: vendor-specific `thinking` body
    /// forwarded verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<OpaqueProviderBody>,
    /// Responses API persistence override. Absent means the client sends
    /// `store: false` by default; callers may explicitly opt in with
    /// `Some(true)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<bool>,
    /// OpenAI prompt-cache affinity routing key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    /// OpenAI prompt-cache retention hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_retention: Option<OpenAiPromptCacheRetention>,
    /// Internal override: force-enable temperature for this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_temperature_override: Option<bool>,
    /// Internal override: force-enable reasoning payload for this
    /// request (used by client_compatible endpoints).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_reasoning_override: Option<bool>,
}

/// Gemini 3 reasoning levels accepted by the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum GeminiThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
}

impl GeminiThinkingLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// Typed shape of Gemini's thinking knob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct GeminiThinkingConfig {
    /// Whether reasoning output is included in the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_thoughts: Option<bool>,
    /// Gemini 3 reasoning level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<GeminiThinkingLevel>,
    /// Reasoning token budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u32>,
}

/// Per-turn Gemini-specific knobs carried in `ProviderTag::Gemini`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct GeminiProviderTag {
    /// Thinking configuration (Gemini 3+ models).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<GeminiThinkingConfig>,
    /// Legacy flat `thinking_budget` — preserved for V3 persistence
    /// round-trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u32>,
    /// Gemini 3 flat thinking level override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<GeminiThinkingLevel>,
    /// Top-K sampling override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Top-P (nucleus) sampling override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Structured-output schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<crate::OutputSchema>,
    /// Provider-native google_search grounding tool body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub google_search: Option<OpaqueProviderBody>,
    /// Number of candidate completions (runtime/persistence knob, not
    /// read by the Gemini client today — preserved for V3 round-trip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_count: Option<u32>,
    /// Gemini explicit context-cache resource name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_content_name: Option<String>,
}

/// Typed projection of OpenAI's reasoning-effort knob.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    None,
    Low,
    #[default]
    Medium,
    High,
    #[serde(rename = "xhigh")]
    XHigh,
}

impl ReasoningEffort {
    pub fn as_legacy_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
        }
    }
}

/// Typed mode for generalized reasoning emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ReasoningMode {
    /// Reasoning output is emitted inline to the caller.
    Emit,
    /// Reasoning is performed but not emitted.
    Silent,
    /// Reasoning is disabled entirely for this turn.
    Off,
}

/// Typed per-turn provider parameter overrides.
///
/// Replaces the legacy untyped `serde_json::Value` bag. Every knob exposed
/// by the runtime on a per-turn seam must have a typed field here. Anything
/// provider-specific enough to not fit goes on [`ProviderTag`]; anything
/// that is fundamentally per-binding (not per-turn) lives on the auth /
/// backend profile and never traverses this seam.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ProviderParamsOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_budget_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_tag: Option<ProviderTag>,
}

impl ProviderParamsOverride {
    pub fn is_empty(&self) -> bool {
        self.temperature.is_none()
            && self.top_p.is_none()
            && self.max_output_tokens.is_none()
            && self.reasoning.is_none()
            && self.thinking_budget_tokens.is_none()
            && self.provider_tag.is_none()
    }

    /// Clear any provider-native web-search / grounding tool body from this
    /// override. Used when an extraction (deterministic, tool-free) turn must
    /// suppress web search without re-deriving provider-native key names.
    pub fn clear_web_search(&mut self) {
        match self.provider_tag.as_mut() {
            Some(ProviderTag::Anthropic(t)) => t.web_search = None,
            Some(ProviderTag::OpenAi(t)) => t.web_search = None,
            Some(ProviderTag::Gemini(t)) => t.google_search = None,
            _ => {}
        }
    }

    /// Inject the structured-output schema for an extraction turn into the
    /// provider tag slot owned by `provider`.
    ///
    /// Fails closed when the override already carries a tag for a different
    /// provider family (an identity conflict is a typed fault, never a silent
    /// overwrite) and when the provider has no typed structured-output slot.
    pub fn set_structured_output(
        &mut self,
        provider: Provider,
        schema: crate::OutputSchema,
    ) -> Result<StructuredOutputInjection, ProviderParamsMergeError> {
        match (provider, self.provider_tag.as_mut()) {
            (Provider::Anthropic, Some(ProviderTag::Anthropic(tag))) => {
                tag.structured_output = Some(schema);
            }
            (Provider::Anthropic, None) => {
                self.provider_tag = Some(ProviderTag::Anthropic(AnthropicProviderTag {
                    structured_output: Some(schema),
                    ..Default::default()
                }));
            }
            // Self-hosted endpoints speak the OpenAI-compatible surface.
            (Provider::OpenAI | Provider::SelfHosted, Some(ProviderTag::OpenAi(tag))) => {
                tag.structured_output = Some(schema);
            }
            (Provider::OpenAI | Provider::SelfHosted, None) => {
                self.provider_tag = Some(ProviderTag::OpenAi(OpenAiProviderTag {
                    structured_output: Some(schema),
                    ..Default::default()
                }));
            }
            (Provider::Gemini, Some(ProviderTag::Gemini(tag))) => {
                tag.structured_output = Some(schema);
            }
            (Provider::Gemini, None) => {
                self.provider_tag = Some(ProviderTag::Gemini(GeminiProviderTag {
                    structured_output: Some(schema),
                    ..Default::default()
                }));
            }
            (Provider::Other, _) => {
                // `Other` has no provider-native structured-output slot —
                // a typed capability fact, not a fault. Extraction proceeds
                // prompt-based; the schema is still enforced at the
                // validation seam after the call.
                return Ok(StructuredOutputInjection::NoProviderSlot);
            }
            (_, Some(tag)) => {
                return Err(ProviderParamsMergeError::ProviderTagMismatch {
                    explicit: tag.provider_label(),
                    defaults: provider.as_str(),
                });
            }
        }
        Ok(StructuredOutputInjection::Injected)
    }
}

/// Typed outcome of [`ProviderParamsOverride::set_structured_output`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructuredOutputInjection {
    /// The schema was injected into the provider's typed structured-output
    /// slot — the provider enforces the output envelope natively.
    Injected,
    /// The provider has no typed structured-output slot; extraction runs
    /// prompt-based and the schema is enforced at the validation seam.
    NoProviderSlot,
}

/// Typed fault from the field-wise provider-params merge.
///
/// A merge conflict between the explicit per-session override and the
/// build-derived defaults (or an extraction injection) is a configuration
/// identity fault — it propagates typed instead of fabricating a mixed bag.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderParamsMergeError {
    #[error(
        "provider params carry a `{explicit}` provider tag but the merge target is `{defaults}`"
    )]
    ProviderTagMismatch {
        explicit: &'static str,
        defaults: &'static str,
    },
}

impl ProviderTag {
    /// Stable label for the provider family this tag belongs to.
    pub fn provider_label(&self) -> &'static str {
        match self {
            Self::Anthropic(_) => "anthropic",
            Self::OpenAi(_) => "openai",
            Self::Gemini(_) => "gemini",
            Self::Unknown { .. } => "unknown",
        }
    }

    /// Field-wise merge: fill every `None` knob on `self` from `defaults`.
    /// Explicitly-set knobs always win; a provider-family mismatch is a typed
    /// fault rather than a silent union of unrelated provider bags.
    pub fn merge_missing_from(
        &mut self,
        defaults: &ProviderTag,
    ) -> Result<(), ProviderParamsMergeError> {
        fn fill<T: Clone>(target: &mut Option<T>, default: &Option<T>) {
            if target.is_none()
                && let Some(value) = default
            {
                *target = Some(value.clone());
            }
        }
        match (self, defaults) {
            (Self::Anthropic(target), Self::Anthropic(default)) => {
                fill(&mut target.thinking, &default.thinking);
                fill(
                    &mut target.thinking_budget_tokens,
                    &default.thinking_budget_tokens,
                );
                fill(&mut target.web_search, &default.web_search);
                fill(&mut target.top_k, &default.top_k);
                fill(&mut target.effort, &default.effort);
                fill(&mut target.structured_output, &default.structured_output);
                fill(&mut target.inference_geo, &default.inference_geo);
                fill(&mut target.compaction, &default.compaction);
                fill(&mut target.context, &default.context);
                fill(&mut target.cache_control, &default.cache_control);
                fill(
                    &mut target.supports_temperature_override,
                    &default.supports_temperature_override,
                );
                Ok(())
            }
            (Self::OpenAi(target), Self::OpenAi(default)) => {
                fill(&mut target.reasoning_effort, &default.reasoning_effort);
                fill(&mut target.seed, &default.seed);
                fill(&mut target.frequency_penalty, &default.frequency_penalty);
                fill(&mut target.presence_penalty, &default.presence_penalty);
                fill(&mut target.web_search, &default.web_search);
                fill(&mut target.structured_output, &default.structured_output);
                fill(&mut target.reasoning, &default.reasoning);
                fill(
                    &mut target.chat_template_kwargs,
                    &default.chat_template_kwargs,
                );
                fill(&mut target.thinking, &default.thinking);
                fill(&mut target.store, &default.store);
                fill(&mut target.prompt_cache_key, &default.prompt_cache_key);
                fill(
                    &mut target.prompt_cache_retention,
                    &default.prompt_cache_retention,
                );
                fill(
                    &mut target.supports_temperature_override,
                    &default.supports_temperature_override,
                );
                fill(
                    &mut target.supports_reasoning_override,
                    &default.supports_reasoning_override,
                );
                Ok(())
            }
            (Self::Gemini(target), Self::Gemini(default)) => {
                fill(&mut target.thinking, &default.thinking);
                fill(&mut target.thinking_budget, &default.thinking_budget);
                fill(&mut target.thinking_level, &default.thinking_level);
                fill(&mut target.top_k, &default.top_k);
                fill(&mut target.top_p, &default.top_p);
                fill(&mut target.structured_output, &default.structured_output);
                fill(&mut target.google_search, &default.google_search);
                fill(&mut target.candidate_count, &default.candidate_count);
                fill(
                    &mut target.cached_content_name,
                    &default.cached_content_name,
                );
                Ok(())
            }
            // An opaque pass-through bag cannot be field-merged; the explicit
            // bag wins wholesale only when both sides are the same opaque tag.
            (Self::Unknown { .. }, Self::Unknown { .. }) => Ok(()),
            (explicit, defaults) => Err(ProviderParamsMergeError::ProviderTagMismatch {
                explicit: explicit.provider_label(),
                defaults: defaults.provider_label(),
            }),
        }
    }
}

/// Config-resident typed owner of provider parameter facts.
///
/// The carrier pairs the explicit, persisted [`ProviderParamsOverride`] (the
/// LLM-edge value domain) with the build-derived provider-native tool
/// defaults. It is parsed fail-closed at config ingress — malformed provider
/// params are rejected when the config is read, never deferred to the first
/// LLM call — and the per-turn effective params are produced by a typed
/// field-wise merge ([`ProviderParamsCarrier::effective_params`]), replacing
/// the retired RFC-7396 raw-JSON merge-patch.
///
/// Serde shape is transparent over `params`: the durable/wire face of the
/// carrier is exactly the typed override shape. `tool_defaults` is never
/// persisted; it is re-derived on every build from config + model profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct ProviderParamsCarrier {
    /// Explicit provider parameter overrides (persisted).
    pub params: ProviderParamsOverride,
    /// Build-derived provider-native tool defaults for the session's
    /// provider (e.g. web-search / grounding tool bodies). Never persisted.
    #[serde(skip)]
    pub tool_defaults: Option<ProviderTag>,
}

impl ProviderParamsCarrier {
    /// Carrier with explicit params and no build-derived defaults.
    pub fn from_params(params: ProviderParamsOverride) -> Self {
        Self {
            params,
            tool_defaults: None,
        }
    }

    /// Whether the carrier holds no facts at all.
    pub fn is_empty(&self) -> bool {
        self.params.is_empty() && self.tool_defaults.is_none()
    }

    /// Whether the carrier's durable face serializes nothing: serde is
    /// transparent over `params` and `tool_defaults` is `#[serde(skip)]`
    /// (build-derived, never persisted), so only the params half decides.
    pub fn serializes_empty(&self) -> bool {
        self.params.is_empty()
    }

    /// Produce the effective per-turn params: explicit overrides win, tool
    /// defaults fill the unset provider-native slots. A provider-family
    /// conflict propagates typed.
    pub fn effective_params(&self) -> Result<ProviderParamsOverride, ProviderParamsMergeError> {
        let mut effective = self.params.clone();
        if let Some(defaults) = self.tool_defaults.as_ref() {
            match effective.provider_tag.as_mut() {
                None => effective.provider_tag = Some(defaults.clone()),
                Some(tag) => tag.merge_missing_from(defaults)?,
            }
        }
        Ok(effective)
    }
}

/// Error returned when [`merge_batch_turn_metadata`] sees two distinct scalar
/// overrides for the same field in a single batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnMetadataMergeConflict {
    pub field: &'static str,
    pub reason: &'static str,
}

impl std::fmt::Display for TurnMetadataMergeConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "batch turn-metadata scalar conflict on field `{}`: {}",
            self.field, self.reason
        )
    }
}

impl std::error::Error for TurnMetadataMergeConflict {}

/// Tri-state per-turn metadata override.
///
/// `None` on the containing field means preserve the durable session value.
/// `Some(Set(value))` overrides it for this turn, and `Some(Clear)` removes it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "action", content = "value", rename_all = "snake_case")]
pub enum TurnMetadataOverride<T> {
    Set(T),
    Clear,
}

impl<T> TurnMetadataOverride<T> {
    pub fn set(value: T) -> Self {
        Self::Set(value)
    }

    pub const fn clear() -> Self {
        Self::Clear
    }

    pub fn as_set(&self) -> Option<&T> {
        match self {
            Self::Set(value) => Some(value),
            Self::Clear => None,
        }
    }

    pub fn into_set(self) -> Option<T> {
        match self {
            Self::Set(value) => Some(value),
            Self::Clear => None,
        }
    }

    pub const fn is_clear(&self) -> bool {
        matches!(self, Self::Clear)
    }

    /// Borrow the inner value, mapping `&TurnMetadataOverride<T>` to
    /// `TurnMetadataOverride<&T>` (mirrors [`Option::as_ref`]). Useful when a
    /// borrowed override seam (e.g. `SessionLlmIdentityOverride<'a>`) needs the
    /// tri-state without taking ownership.
    pub fn as_ref(&self) -> TurnMetadataOverride<&T> {
        match self {
            Self::Set(value) => TurnMetadataOverride::Set(value),
            Self::Clear => TurnMetadataOverride::Clear,
        }
    }
}

impl<'de, T> Deserialize<'de> for TurnMetadataOverride<T>
where
    T: DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = serde_json::Value::deserialize(deserializer)?;
        if let Some(object) = raw.as_object() {
            let Some(action_value) = object.get("action") else {
                return serde_json::from_value(raw)
                    .map(Self::Set)
                    .map_err(de::Error::custom);
            };
            let action = action_value.as_str().ok_or_else(|| {
                de::Error::custom("turn metadata override action must be a string")
            })?;
            return match action {
                "clear" => {
                    if object.contains_key("value") {
                        return Err(de::Error::custom("clear override cannot include value"));
                    }
                    Ok(Self::Clear)
                }
                "set" => {
                    let value = object
                        .get("value")
                        .ok_or_else(|| de::Error::custom("set override is missing value"))?;
                    serde_json::from_value(value.clone())
                        .map(Self::Set)
                        .map_err(de::Error::custom)
                }
                other => Err(de::Error::custom(format!(
                    "unknown turn metadata override action `{other}`"
                ))),
            };
        }

        serde_json::from_value(raw)
            .map(Self::Set)
            .map_err(de::Error::custom)
    }
}

/// Canonical per-turn runtime metadata carried alongside a
/// [`StagedRunInput`]. This is the typed seam consumed by the core layer —
/// `serde_json::Value` does not appear anywhere in this shape.
///
/// Construction in the runtime crate MUST go through the single canonical
/// `for_input(&Input)` constructor. Other code paths that previously built
/// a `RuntimeTurnMetadata` literal are updated to call `for_input` or be
/// deleted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RuntimeTurnMetadata {
    /// Handling mode for staged ordinary work when admitted through runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handling_mode: Option<HandlingMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_references: Option<Vec<SkillKey>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_tool_overlay: Option<TurnToolOverlay>,
    /// Additional instructions for this turn, typed by role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<TurnInstruction>>,
    /// Override model for this turn (hot-swap on materialized sessions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelId>,
    /// Override provider for this turn (hot-swap on materialized sessions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    /// Override, clear, or preserve provider-specific parameters for this turn
    /// (typed; no Value).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<TurnMetadataOverride<ProviderParamsOverride>>,
    /// Override, clear, or preserve the auth binding reference this turn must
    /// resolve against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_binding: Option<TurnMetadataOverride<AuthBindingRef>>,
    /// Keep-alive directive for materialized resources for this turn.
    ///
    /// `None` preserves the session's existing keep-alive setting; see
    /// [`KeepAliveDirective`] for the explicit enable/disable tri-state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<KeepAliveDirective>,
    /// Optional normalized rendering metadata for this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_metadata: Option<RenderMetadata>,
    /// Typed execution intent classified by the runtime layer.
    ///
    /// `None` is retained only for legacy/caller-owned metadata before runtime
    /// admission. Runtime boundaries must stamp this field before applying a
    /// turn; the session layer must not invent a runtime execution kind.
    /// `Some(ContentTurn)` forces `run_turn`.
    /// `Some(ResumePending)` forces `run_pending`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_kind: Option<RuntimeExecutionKind>,
    /// Typed terminal peer-response apply intent classified by the runtime
    /// machine at admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_response_terminal_apply_intent: Option<PeerResponseTerminalApplyIntent>,
    /// Stable transcript identity derived at runtime admission. Persisting this
    /// on transcript messages lets history readers join persisted frames with
    /// live frames without falling back to message text.
    #[serde(default, skip_serializing_if = "TranscriptMessageIdentity::is_empty")]
    pub transcript_identity: TranscriptMessageIdentity,
}

impl RuntimeTurnMetadata {
    /// True when every field is `None` — used to skip serializing empty
    /// metadata carriers on the wire.
    pub fn is_empty(&self) -> bool {
        self.handling_mode.is_none()
            && self.skill_references.is_none()
            && self.flow_tool_overlay.is_none()
            && self.additional_instructions.is_none()
            && self.model.is_none()
            && self.provider.is_none()
            && self.provider_params.is_none()
            && self.auth_binding.is_none()
            && self.keep_alive.is_none()
            && self.render_metadata.is_none()
            && self.execution_kind.is_none()
            && self.peer_response_terminal_apply_intent.is_none()
            && self.transcript_identity.is_empty()
    }

    pub fn transcript_message_identity(&self) -> Option<TranscriptMessageIdentity> {
        (!self.transcript_identity.is_empty()).then(|| self.transcript_identity.clone())
    }

    /// Merge another metadata carrier into this one. Scalar conflicts (two
    /// inputs in a batch disagreeing on `model`, `provider`, `auth_binding`,
    /// etc.) return a typed [`TurnMetadataMergeConflict`] rather than
    /// last-wins. Collection fields accumulate.
    pub fn merge(&mut self, other: Self) -> Result<(), TurnMetadataMergeConflict> {
        // Scalar: conflict-refusing merge.
        merge_scalar(
            &mut self.handling_mode,
            other.handling_mode,
            "handling_mode",
        )?;
        merge_scalar(
            &mut self.flow_tool_overlay,
            other.flow_tool_overlay,
            "flow_tool_overlay",
        )?;
        merge_scalar(&mut self.model, other.model, "model")?;
        merge_scalar(&mut self.provider, other.provider, "provider")?;
        merge_override(
            &mut self.provider_params,
            other.provider_params,
            "provider_params",
        )?;
        merge_override(&mut self.auth_binding, other.auth_binding, "auth_binding")?;
        merge_scalar(&mut self.keep_alive, other.keep_alive, "keep_alive")?;
        merge_scalar(
            &mut self.render_metadata,
            other.render_metadata,
            "render_metadata",
        )?;
        merge_scalar(
            &mut self.execution_kind,
            other.execution_kind,
            "execution_kind",
        )?;
        merge_scalar(
            &mut self.peer_response_terminal_apply_intent,
            other.peer_response_terminal_apply_intent,
            "peer_response_terminal_apply_intent",
        )?;
        merge_transcript_identity(&mut self.transcript_identity, other.transcript_identity);

        // Collections: accumulate.
        if let Some(extra) = other.skill_references {
            self.skill_references
                .get_or_insert_with(Vec::new)
                .extend(extra);
        }
        if let Some(extra) = other.additional_instructions {
            self.additional_instructions
                .get_or_insert_with(Vec::new)
                .extend(extra);
        }
        Ok(())
    }
}

fn merge_transcript_identity(lhs: &mut TranscriptMessageIdentity, rhs: TranscriptMessageIdentity) {
    if rhs.is_empty() {
        return;
    }
    if lhs.is_empty() {
        *lhs = rhs;
        return;
    }
    if *lhs != rhs {
        *lhs = TranscriptMessageIdentity::default();
    }
}

fn merge_scalar<T: PartialEq>(
    lhs: &mut Option<T>,
    rhs: Option<T>,
    field: &'static str,
) -> Result<(), TurnMetadataMergeConflict> {
    match (lhs.as_ref(), rhs) {
        (_, None) => Ok(()),
        (None, Some(v)) => {
            *lhs = Some(v);
            Ok(())
        }
        (Some(existing), Some(new)) => {
            if *existing == new {
                Ok(())
            } else {
                Err(TurnMetadataMergeConflict {
                    field,
                    reason: "two inputs in one batch set distinct scalar overrides",
                })
            }
        }
    }
}

fn merge_override<T: PartialEq>(
    lhs: &mut Option<TurnMetadataOverride<T>>,
    rhs: Option<TurnMetadataOverride<T>>,
    field: &'static str,
) -> Result<(), TurnMetadataMergeConflict> {
    match (lhs.as_ref(), rhs) {
        (_, None) => Ok(()),
        (None, Some(override_fact)) => {
            *lhs = Some(override_fact);
            Ok(())
        }
        (Some(existing), Some(new)) if *existing == new => Ok(()),
        (Some(TurnMetadataOverride::Set(_)), Some(TurnMetadataOverride::Set(_))) => {
            Err(TurnMetadataMergeConflict {
                field,
                reason: "two inputs in one batch set distinct scalar overrides",
            })
        }
        (Some(_), Some(_)) => Err(TurnMetadataMergeConflict {
            field,
            reason: "one input sets the field while another clears it",
        }),
    }
}

mod duration_seconds {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(value: &Duration, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_u64(value.as_secs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(de)?;
        Ok(Duration::from_secs(secs))
    }
}

/// An input staged for application at a run boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StagedRunInput {
    /// When to apply this input.
    pub boundary: RunApplyBoundary,
    /// Conversation mutations to apply.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub appends: Vec<ConversationAppend>,
    /// Context-only appends.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_appends: Vec<ConversationContextAppend>,
    /// Input IDs contributing to this staged input (opaque to core).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contributing_input_ids: Vec<InputId>,
    /// Optional turn semantics that must survive crash recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_metadata: Option<RuntimeTurnMetadata>,
}

/// The ONLY type core receives from the runtime layer for run execution.
///
/// This is the complete interface between the runtime control-plane and core.
/// Core does not know about Input, InputState, PolicyDecision, or any
/// runtime-layer types. It only sees this.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "primitive_type", rename_all = "snake_case")]
// StagedInput is intentionally large — it carries the full
// RuntimeTurnMetadata (model/provider/auth_binding overrides,
// rendering metadata, skill refs, etc.). Boxing would force an
// allocation on every input construction, which is in the hot path.
#[allow(clippy::large_enum_variant)]
pub enum RunPrimitive {
    /// Apply conversation mutations at a boundary.
    StagedInput(StagedRunInput),
    /// Inject content immediately (no boundary required).
    ImmediateAppend(ConversationAppend),
    /// Inject context immediately.
    ImmediateContextAppend(ConversationContextAppend),
}

impl RunPrimitive {
    /// Get all contributing input IDs (if any).
    pub fn contributing_input_ids(&self) -> &[InputId] {
        match self {
            RunPrimitive::StagedInput(staged) => &staged.contributing_input_ids,
            RunPrimitive::ImmediateAppend(_) | RunPrimitive::ImmediateContextAppend(_) => &[],
        }
    }

    pub fn turn_metadata(&self) -> Option<&RuntimeTurnMetadata> {
        match self {
            RunPrimitive::StagedInput(staged) => staged.turn_metadata.as_ref(),
            RunPrimitive::ImmediateAppend(_) | RunPrimitive::ImmediateContextAppend(_) => None,
        }
    }

    /// Extract content input from this primitive's conversation appends.
    ///
    /// Consolidates the 5 near-identical `extract_prompt` / `extract_runtime_prompt`
    /// functions that were duplicated across RPC, REST, MCP, mob, and CLI surfaces.
    pub fn extract_content_input(&self) -> crate::types::ContentInput {
        match self {
            RunPrimitive::StagedInput(staged) => {
                content_input_from_conversation_appends(&staged.appends)
            }
            RunPrimitive::ImmediateAppend(append) => {
                content_input_from_core_renderable(&append.content)
            }
            RunPrimitive::ImmediateContextAppend(ctx) => {
                content_input_from_core_renderable(&ctx.content)
            }
        }
    }

    /// Project this primitive into provider-visible input.
    ///
    /// Unlike [`RunPrimitive::extract_content_input`], this is allowed to
    /// render typed runtime-authored notices into model text. The rendered text
    /// is an internal provider projection only; transcript persistence remains
    /// typed `SystemNotice` content.
    pub fn model_projection_content_input(&self) -> crate::types::ContentInput {
        match self {
            RunPrimitive::StagedInput(staged) => {
                model_projection_content_input_from_conversation_appends(&staged.appends)
            }
            RunPrimitive::ImmediateAppend(append) => {
                model_projection_content_input_from_core_renderable(&append.content)
            }
            RunPrimitive::ImmediateContextAppend(ctx) => {
                model_projection_content_input_from_core_renderable(&ctx.content)
            }
        }
    }

    /// Runtime-authored transcript appends carried by this primitive.
    ///
    /// Provider prompt text is derived separately; these appends remain the
    /// durable authorship source for transcript persistence.
    pub fn typed_turn_appends(&self) -> Vec<ConversationAppend> {
        match self {
            RunPrimitive::StagedInput(staged) => staged.appends.clone(),
            RunPrimitive::ImmediateAppend(append) => vec![append.clone()],
            RunPrimitive::ImmediateContextAppend(_) => Vec::new(),
        }
    }

    /// Return the canonical runtime apply boundary for this primitive.
    pub fn apply_boundary(&self) -> RunApplyBoundary {
        match self {
            RunPrimitive::StagedInput(staged) => staged.boundary,
            RunPrimitive::ImmediateAppend(_) | RunPrimitive::ImmediateContextAppend(_) => {
                RunApplyBoundary::Immediate
            }
        }
    }

    pub fn peer_response_terminal_apply_intent(&self) -> Option<PeerResponseTerminalApplyIntent> {
        self.turn_metadata()
            .and_then(|metadata| metadata.peer_response_terminal_apply_intent)
    }

    pub fn is_peer_response_terminal_context_and_run(&self) -> bool {
        matches!(
            self.peer_response_terminal_apply_intent(),
            Some(PeerResponseTerminalApplyIntent::AppendContextAndRun)
        )
    }

    pub fn peer_response_terminal_apply_intent_violation(&self) -> Option<&'static str> {
        if !self.is_peer_response_terminal_context_and_run() {
            return None;
        }

        let RunPrimitive::StagedInput(staged) = self else {
            return Some("terminal peer-response apply intent requires a staged primitive");
        };
        if staged.boundary != RunApplyBoundary::RunStart {
            return Some("terminal peer-response apply intent requires RunStart boundary");
        }
        if staged.context_appends.is_empty() {
            return Some("terminal peer-response apply intent requires a staged context append");
        }
        if staged
            .turn_metadata
            .as_ref()
            .and_then(|metadata| metadata.execution_kind)
            != Some(RuntimeExecutionKind::ContentTurn)
        {
            return Some("terminal peer-response apply intent requires ContentTurn execution kind");
        }
        None
    }

    /// Whether this primitive's context appends should be applied without
    /// running a requester reaction turn.
    pub fn is_context_only_apply_without_turn(&self) -> bool {
        matches!(
            self,
            RunPrimitive::StagedInput(staged)
            if staged.appends.is_empty()
                && !staged.context_appends.is_empty()
                && !self.is_peer_response_terminal_context_and_run()
        )
    }

    /// Whether this primitive is a context-only staged input that should be
    /// routed to `apply_runtime_context_appends` rather than a full turn.
    pub fn is_context_only_immediate(&self) -> bool {
        matches!(
            self,
            RunPrimitive::StagedInput(staged)
            if staged.appends.is_empty()
                && !staged.context_appends.is_empty()
                && staged.boundary == RunApplyBoundary::Immediate
        )
    }
}

pub fn content_input_from_conversation_appends(
    appends: &[ConversationAppend],
) -> crate::types::ContentInput {
    let mut all_blocks = Vec::new();
    for append in appends {
        append_content_blocks(&append.content, &mut all_blocks);
    }
    content_input_from_blocks(all_blocks)
}

pub fn model_projection_content_input_from_conversation_appends(
    appends: &[ConversationAppend],
) -> crate::types::ContentInput {
    let mut all_blocks = Vec::new();
    for append in appends {
        append_model_projection_blocks(&append.content, &mut all_blocks);
    }
    content_input_from_blocks(all_blocks)
}

fn content_input_from_core_renderable(content: &CoreRenderable) -> crate::types::ContentInput {
    let mut all_blocks = Vec::new();
    append_content_blocks(content, &mut all_blocks);
    content_input_from_blocks(all_blocks)
}

fn model_projection_content_input_from_core_renderable(
    content: &CoreRenderable,
) -> crate::types::ContentInput {
    let mut all_blocks = Vec::new();
    append_model_projection_blocks(content, &mut all_blocks);
    content_input_from_blocks(all_blocks)
}

fn append_content_blocks(
    content: &CoreRenderable,
    all_blocks: &mut Vec<crate::types::ContentBlock>,
) {
    use crate::types::ContentBlock;
    match content {
        CoreRenderable::Text { text } => {
            all_blocks.push(ContentBlock::Text { text: text.clone() });
        }
        CoreRenderable::Blocks { blocks } => {
            all_blocks.extend(blocks.iter().cloned());
        }
        CoreRenderable::SystemNotice { .. } => {}
        _ => {}
    }
}

fn append_model_projection_blocks(
    content: &CoreRenderable,
    all_blocks: &mut Vec<crate::types::ContentBlock>,
) {
    use crate::types::{ContentBlock, SystemNoticeMessage};
    match content {
        CoreRenderable::SystemNotice { kind, body, blocks } => {
            let projection = SystemNoticeMessage::with_blocks(*kind, body.clone(), blocks.clone())
                .model_projection_text();
            if !projection.trim().is_empty() {
                all_blocks.push(ContentBlock::Text { text: projection });
            }
            append_system_notice_media_blocks(blocks, all_blocks);
        }
        _ => append_content_blocks(content, all_blocks),
    }
}

fn append_system_notice_media_blocks(
    blocks: &[SystemNoticeBlock],
    all_blocks: &mut Vec<crate::types::ContentBlock>,
) {
    for block in blocks {
        let content = match block {
            SystemNoticeBlock::Comms { content, .. }
            | SystemNoticeBlock::ExternalEvent { content, .. } => content,
            _ => continue,
        };
        all_blocks.extend(
            content
                .iter()
                .filter(|block| !matches!(block, crate::types::ContentBlock::Text { .. }))
                .cloned(),
        );
    }
}

fn content_input_from_blocks(
    all_blocks: Vec<crate::types::ContentBlock>,
) -> crate::types::ContentInput {
    use crate::types::{ContentBlock, ContentInput};
    if all_blocks.is_empty() {
        ContentInput::Text(String::new())
    } else if all_blocks.len() == 1 {
        if let ContentBlock::Text { text } = &all_blocks[0] {
            ContentInput::Text(text.clone())
        } else {
            ContentInput::Blocks(all_blocks)
        }
    } else {
        ContentInput::Blocks(all_blocks)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn run_apply_boundary_serde_roundtrip() {
        for boundary in [
            RunApplyBoundary::Immediate,
            RunApplyBoundary::RunStart,
            RunApplyBoundary::RunCheckpoint,
        ] {
            let json = serde_json::to_value(boundary).unwrap();
            let parsed: RunApplyBoundary = serde_json::from_value(json).unwrap();
            assert_eq!(boundary, parsed);
        }
    }

    #[test]
    fn core_renderable_text_serde() {
        let r = CoreRenderable::Text {
            text: "hello".into(),
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "hello");
        let parsed: CoreRenderable = serde_json::from_value(json).unwrap();
        assert_eq!(r, parsed);
    }

    #[test]
    fn core_renderable_json_serde() {
        let r = CoreRenderable::Json {
            value: serde_json::json!({"key": "val"}),
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["type"], "json");
        let parsed: CoreRenderable = serde_json::from_value(json).unwrap();
        assert_eq!(r, parsed);
    }

    // --- extract_content_input tests ---

    fn make_staged(appends: Vec<ConversationAppend>) -> RunPrimitive {
        RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::RunStart,
            appends,
            context_appends: vec![],
            contributing_input_ids: vec![],
            turn_metadata: None,
        })
    }

    #[test]
    fn extract_content_from_staged_text() {
        let p = make_staged(vec![ConversationAppend {
            role: ConversationAppendRole::User,
            content: CoreRenderable::Text {
                text: "hello".into(),
            },
        }]);
        assert_eq!(
            p.extract_content_input(),
            crate::types::ContentInput::Text("hello".into())
        );
    }

    #[test]
    fn extract_content_from_staged_blocks() {
        let p = make_staged(vec![ConversationAppend {
            role: ConversationAppendRole::User,
            content: CoreRenderable::Blocks {
                blocks: vec![
                    crate::types::ContentBlock::Text { text: "a".into() },
                    crate::types::ContentBlock::Text { text: "b".into() },
                ],
            },
        }]);
        let result = p.extract_content_input();
        assert!(
            matches!(&result, crate::types::ContentInput::Blocks(blocks) if blocks.len() == 2),
            "expected Blocks with 2 elements, got {result:?}"
        );
    }

    #[test]
    fn extract_content_from_staged_empty() {
        let p = make_staged(vec![]);
        assert_eq!(
            p.extract_content_input(),
            crate::types::ContentInput::Text(String::new())
        );
    }

    #[test]
    fn extract_content_single_text_block_collapses() {
        let p = make_staged(vec![ConversationAppend {
            role: ConversationAppendRole::User,
            content: CoreRenderable::Blocks {
                blocks: vec![crate::types::ContentBlock::Text {
                    text: "single".into(),
                }],
            },
        }]);
        assert_eq!(
            p.extract_content_input(),
            crate::types::ContentInput::Text("single".into())
        );
    }

    #[test]
    fn system_notice_append_does_not_leak_projection_into_operator_prompt() {
        let append = ConversationAppend {
            role: ConversationAppendRole::SystemNotice,
            content: CoreRenderable::SystemNotice {
                kind: SystemNoticeKind::Comms,
                body: Some("Peer request: checksum_token".to_string()),
                blocks: vec![SystemNoticeBlock::Comms {
                    kind: crate::types::CommsNoticeKind::Request,
                    direction: crate::types::SystemNoticeDirection::Incoming,
                    peer: Some(crate::types::SystemNoticePeer {
                        id: crate::comms::PeerId::new(),
                        display_name: Some("worker-1".to_string()),
                    }),
                    sender_taint: None,
                    request_id: Some(crate::time_compat::new_uuid_v7().to_string()),
                    intent: Some("checksum_token".to_string()),
                    status: None,
                    summary: Some("Peer request: checksum_token".to_string()),
                    payload: None,
                    content: vec![crate::types::ContentBlock::Text {
                        text: "What is the token?".to_string(),
                    }],
                }],
            },
        };
        let p = make_staged(vec![append.clone()]);

        assert_eq!(p.typed_turn_appends(), vec![append]);
        assert_eq!(
            p.extract_content_input(),
            crate::types::ContentInput::Text(String::new())
        );
        let projection = p.model_projection_content_input().text_content();
        assert!(projection.contains("Peer request"));
        assert!(projection.contains("checksum_token"));
        assert!(projection.contains("What is the token?"));
    }

    #[test]
    fn system_notice_media_remains_typed_notice_not_operator_prompt() {
        let image = crate::types::ContentBlock::Image {
            media_type: "image/png".to_string(),
            data: crate::types::ImageData::Inline {
                data: "aW1hZ2U=".to_string(),
            },
        };
        let p = make_staged(vec![ConversationAppend {
            role: ConversationAppendRole::SystemNotice,
            content: CoreRenderable::SystemNotice {
                kind: SystemNoticeKind::ExternalEvent,
                body: Some("External event".to_string()),
                blocks: vec![SystemNoticeBlock::ExternalEvent {
                    source: "webhook".to_string(),
                    event_type: "image".to_string(),
                    summary: Some("Webhook image".to_string()),
                    body: Some("Do not inject this prose".to_string()),
                    payload: None,
                    content: vec![
                        crate::types::ContentBlock::Text {
                            text: "Do not inject this text".to_string(),
                        },
                        image,
                    ],
                }],
            },
        }]);

        assert_eq!(
            p.extract_content_input(),
            crate::types::ContentInput::Text(String::new())
        );
        match p.model_projection_content_input() {
            crate::types::ContentInput::Blocks(blocks) => {
                assert!(blocks.iter().any(|block| matches!(
                    block,
                    crate::types::ContentBlock::Text { text }
                        if text.contains("Do not inject this prose")
                            && text.contains("Do not inject this text")
                )));
                assert!(
                    blocks
                        .iter()
                        .any(|block| matches!(block, crate::types::ContentBlock::Image { .. }))
                );
            }
            other => panic!("expected typed notice projection with media blocks, got {other:?}"),
        }
    }

    #[test]
    fn typed_turn_appends_excludes_context_only_appends() {
        let p = RunPrimitive::ImmediateContextAppend(ConversationContextAppend {
            key: "ctx".to_string(),
            content: CoreRenderable::SystemNotice {
                kind: SystemNoticeKind::Comms,
                body: Some("context".to_string()),
                blocks: Vec::new(),
            },
        });

        assert!(p.typed_turn_appends().is_empty());
        assert_eq!(
            p.extract_content_input(),
            crate::types::ContentInput::Text(String::new())
        );
    }

    // --- is_context_only_immediate tests ---

    #[test]
    fn context_only_immediate_true() {
        let p = RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::Immediate,
            appends: vec![],
            context_appends: vec![ConversationContextAppend {
                key: "k".into(),
                content: CoreRenderable::Text { text: "ctx".into() },
            }],
            contributing_input_ids: vec![],
            turn_metadata: None,
        });
        assert!(p.is_context_only_immediate());
    }

    #[test]
    fn context_only_immediate_false_with_appends() {
        let p = RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::Immediate,
            appends: vec![ConversationAppend {
                role: ConversationAppendRole::User,
                content: CoreRenderable::Text { text: "hi".into() },
            }],
            context_appends: vec![ConversationContextAppend {
                key: "k".into(),
                content: CoreRenderable::Text { text: "ctx".into() },
            }],
            contributing_input_ids: vec![],
            turn_metadata: None,
        });
        assert!(!p.is_context_only_immediate());
    }

    #[test]
    fn context_only_immediate_false_wrong_boundary() {
        let p = RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::RunCheckpoint,
            appends: vec![],
            context_appends: vec![ConversationContextAppend {
                key: "k".into(),
                content: CoreRenderable::Text { text: "ctx".into() },
            }],
            contributing_input_ids: vec![],
            turn_metadata: None,
        });
        assert!(!p.is_context_only_immediate());
    }

    #[test]
    fn context_only_apply_without_turn_true_for_plain_context() {
        let p = RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::RunCheckpoint,
            appends: vec![],
            context_appends: vec![ConversationContextAppend {
                key: "k".into(),
                content: CoreRenderable::Text { text: "ctx".into() },
            }],
            contributing_input_ids: vec![],
            turn_metadata: Some(RuntimeTurnMetadata {
                execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                ..Default::default()
            }),
        });

        assert!(p.is_context_only_apply_without_turn());
    }

    #[test]
    fn terminal_peer_response_context_and_run_bypasses_context_only_shortcut() {
        let p = RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::RunStart,
            appends: vec![],
            context_appends: vec![ConversationContextAppend {
                key: "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:req-123".into(),
                content: CoreRenderable::Text {
                    text: "Peer terminal response: done".into(),
                },
            }],
            contributing_input_ids: vec![InputId::new()],
            turn_metadata: Some(RuntimeTurnMetadata {
                execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                peer_response_terminal_apply_intent: Some(
                    PeerResponseTerminalApplyIntent::AppendContextAndRun,
                ),
                ..Default::default()
            }),
        });

        assert!(p.is_peer_response_terminal_context_and_run());
        assert_eq!(p.peer_response_terminal_apply_intent_violation(), None);
        assert!(!p.is_context_only_apply_without_turn());
    }

    #[test]
    fn terminal_peer_response_with_conversation_append_keeps_context_and_run_intent() {
        let p = RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::RunStart,
            appends: vec![ConversationAppend {
                role: ConversationAppendRole::User,
                content: CoreRenderable::Blocks {
                    blocks: vec![crate::types::ContentBlock::Text {
                        text: "Peer terminal response: done".into(),
                    }],
                },
            }],
            context_appends: vec![ConversationContextAppend {
                key: "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:req-123".into(),
                content: CoreRenderable::Text {
                    text: "Peer terminal response: done".into(),
                },
            }],
            contributing_input_ids: vec![InputId::new()],
            turn_metadata: Some(RuntimeTurnMetadata {
                execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                peer_response_terminal_apply_intent: Some(
                    PeerResponseTerminalApplyIntent::AppendContextAndRun,
                ),
                ..Default::default()
            }),
        });

        assert!(p.is_peer_response_terminal_context_and_run());
        assert_eq!(p.peer_response_terminal_apply_intent_violation(), None);
        assert!(!p.is_context_only_apply_without_turn());
    }

    #[test]
    fn non_staged_is_not_context_only() {
        let p = RunPrimitive::ImmediateAppend(ConversationAppend {
            role: ConversationAppendRole::User,
            content: CoreRenderable::Text { text: "hi".into() },
        });
        assert!(!p.is_context_only_immediate());
    }

    #[test]
    fn core_renderable_reference_serde() {
        let r = CoreRenderable::Reference {
            uri: "file:///tmp/a.txt".into(),
            label: Some("a file".into()),
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["type"], "reference");
        let parsed: CoreRenderable = serde_json::from_value(json).unwrap();
        assert_eq!(r, parsed);
    }

    #[test]
    fn execution_kind_serde_round_trip() {
        for kind in [
            RuntimeExecutionKind::ContentTurn,
            RuntimeExecutionKind::ResumePending,
        ] {
            let json = serde_json::to_value(kind).unwrap();
            let parsed: RuntimeExecutionKind = serde_json::from_value(json.clone()).unwrap();
            assert_eq!(kind, parsed);
        }
        // Verify snake_case naming
        assert_eq!(
            serde_json::to_value(RuntimeExecutionKind::ContentTurn).unwrap(),
            serde_json::Value::String("content_turn".into())
        );
        assert_eq!(
            serde_json::to_value(RuntimeExecutionKind::ResumePending).unwrap(),
            serde_json::Value::String("resume_pending".into())
        );
    }

    #[test]
    fn turn_metadata_execution_kind_defaults_to_none() {
        let meta = RuntimeTurnMetadata::default();
        assert_eq!(meta.execution_kind, None);
    }

    #[test]
    fn turn_metadata_execution_kind_round_trips() {
        let meta = RuntimeTurnMetadata {
            execution_kind: Some(RuntimeExecutionKind::ContentTurn),
            ..Default::default()
        };
        let json = serde_json::to_value(&meta).unwrap();
        assert_eq!(json["execution_kind"], "content_turn");
        let parsed: RuntimeTurnMetadata = serde_json::from_value(json).unwrap();
        assert_eq!(
            parsed.execution_kind,
            Some(RuntimeExecutionKind::ContentTurn)
        );
    }

    #[test]
    fn turn_metadata_without_execution_kind_deserializes() {
        // Backward compat: old payloads without execution_kind deserialize to None
        let json = serde_json::json!({});
        let parsed: RuntimeTurnMetadata = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.execution_kind, None);
    }

    #[test]
    fn conversation_append_role_serde() {
        for role in [
            ConversationAppendRole::User,
            ConversationAppendRole::Assistant,
            ConversationAppendRole::SystemNotice,
            ConversationAppendRole::Tool,
            ConversationAppendRole::InjectedContext,
        ] {
            let json = serde_json::to_value(role).unwrap();
            let parsed: ConversationAppendRole = serde_json::from_value(json).unwrap();
            assert_eq!(role, parsed);
        }
    }

    #[test]
    fn conversation_append_role_injected_context_is_snake_case() {
        assert_eq!(
            serde_json::to_value(ConversationAppendRole::InjectedContext).unwrap(),
            serde_json::json!("injected_context"),
        );
    }

    #[test]
    fn conversation_append_serde() {
        let append = ConversationAppend {
            role: ConversationAppendRole::User,
            content: CoreRenderable::Text {
                text: "hello".into(),
            },
        };
        let json = serde_json::to_value(&append).unwrap();
        let parsed: ConversationAppend = serde_json::from_value(json).unwrap();
        assert_eq!(append, parsed);
    }

    #[test]
    fn staged_run_input_serde() {
        let staged = StagedRunInput {
            boundary: RunApplyBoundary::RunStart,
            appends: vec![ConversationAppend {
                role: ConversationAppendRole::User,
                content: CoreRenderable::Text {
                    text: "prompt".into(),
                },
            }],
            context_appends: vec![],
            contributing_input_ids: vec![InputId::new()],
            turn_metadata: Some(RuntimeTurnMetadata {
                keep_alive: Some(KeepAliveDirective::Enable(KeepAlivePolicy {
                    ttl: std::time::Duration::from_secs(30),
                    policy: KeepAliveMode::Pinned,
                })),
                ..Default::default()
            }),
        };
        let json = serde_json::to_value(&staged).unwrap();
        let parsed: StagedRunInput = serde_json::from_value(json).unwrap();
        assert_eq!(staged, parsed);
    }

    /// Dogma K13: the per-turn keep-alive carrier must represent the full
    /// tri-state. `Disable` must survive serde round-trips distinctly from
    /// both `Enable` and absence (`Preserve`).
    #[test]
    fn keep_alive_directive_tri_state_round_trip() {
        let disable = RuntimeTurnMetadata {
            keep_alive: Some(KeepAliveDirective::Disable),
            ..Default::default()
        };
        let json = serde_json::to_value(&disable).unwrap();
        let parsed: RuntimeTurnMetadata = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.keep_alive, Some(KeepAliveDirective::Disable));
        assert!(!parsed.is_empty(), "explicit Disable is not empty metadata");

        let enable = RuntimeTurnMetadata {
            keep_alive: Some(KeepAliveDirective::Enable(KeepAlivePolicy {
                ttl: std::time::Duration::from_secs(30),
                policy: KeepAliveMode::Pinned,
            })),
            ..Default::default()
        };
        let json = serde_json::to_value(&enable).unwrap();
        let parsed: RuntimeTurnMetadata = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.keep_alive, enable.keep_alive);
        assert_ne!(parsed.keep_alive, Some(KeepAliveDirective::Disable));
    }

    #[test]
    fn run_primitive_staged_input_serde() {
        let primitive = RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::RunStart,
            appends: vec![],
            context_appends: vec![],
            contributing_input_ids: vec![InputId::new(), InputId::new()],
            turn_metadata: None,
        });
        let json = serde_json::to_value(&primitive).unwrap();
        assert_eq!(json["primitive_type"], "staged_input");
        let parsed: RunPrimitive = serde_json::from_value(json).unwrap();
        assert_eq!(primitive, parsed);
    }

    #[test]
    fn run_primitive_immediate_append_serde() {
        let primitive = RunPrimitive::ImmediateAppend(ConversationAppend {
            role: ConversationAppendRole::SystemNotice,
            content: CoreRenderable::Text {
                text: "notice".into(),
            },
        });
        let json = serde_json::to_value(&primitive).unwrap();
        assert_eq!(json["primitive_type"], "immediate_append");
        let parsed: RunPrimitive = serde_json::from_value(json).unwrap();
        assert_eq!(primitive, parsed);
    }

    #[test]
    fn run_primitive_contributing_input_ids() {
        let ids = vec![InputId::new(), InputId::new()];
        let primitive = RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::RunStart,
            appends: vec![],
            context_appends: vec![],
            contributing_input_ids: ids.clone(),
            turn_metadata: None,
        });
        assert_eq!(primitive.contributing_input_ids(), &ids);

        let immediate = RunPrimitive::ImmediateAppend(ConversationAppend {
            role: ConversationAppendRole::User,
            content: CoreRenderable::Text { text: "hi".into() },
        });
        assert!(immediate.contributing_input_ids().is_empty());
    }

    #[test]
    fn conversation_context_append_serde() {
        let ctx = ConversationContextAppend {
            key: "peers".into(),
            content: CoreRenderable::Json {
                value: serde_json::json!(["peer1", "peer2"]),
            },
        };
        let json = serde_json::to_value(&ctx).unwrap();
        let parsed: ConversationContextAppend = serde_json::from_value(json).unwrap();
        assert_eq!(ctx, parsed);
    }

    /// K2 invariant: the per-turn effective params come from a typed
    /// field-wise merge on the carrier — explicit overrides win, build-derived
    /// tool defaults fill unset slots, and nothing round-trips through JSON.
    #[test]
    fn carrier_effective_params_typed_fieldwise_merge() {
        let carrier = ProviderParamsCarrier {
            params: ProviderParamsOverride {
                temperature: Some(0.3),
                provider_tag: Some(ProviderTag::Anthropic(AnthropicProviderTag {
                    effort: Some(AnthropicEffort::High),
                    cache_control: Some(AnthropicCacheControlPolicy::Disabled),
                    ..Default::default()
                })),
                ..Default::default()
            },
            tool_defaults: Some(ProviderTag::Anthropic(AnthropicProviderTag {
                effort: Some(AnthropicEffort::Low),
                cache_control: Some(AnthropicCacheControlPolicy::SystemPrefix),
                web_search: Some(OpaqueProviderBody::from_value(
                    &serde_json::json!({"type": "web_search_20250305"}),
                )),
                ..Default::default()
            })),
        };

        let effective = carrier.effective_params().expect("merge succeeds");
        assert_eq!(effective.temperature, Some(0.3));
        let Some(ProviderTag::Anthropic(tag)) = effective.provider_tag else {
            panic!("anthropic tag expected");
        };
        // Explicit knob wins over the default.
        assert_eq!(tag.effort, Some(AnthropicEffort::High));
        assert_eq!(
            tag.cache_control,
            Some(AnthropicCacheControlPolicy::Disabled)
        );
        // Unset slot is filled from the build-derived default.
        assert_eq!(
            tag.web_search,
            Some(OpaqueProviderBody::from_value(
                &serde_json::json!({"type": "web_search_20250305"})
            ))
        );
    }

    /// K2 invariant: a provider-family conflict between explicit params and
    /// tool defaults is a typed fault, never a silently-fabricated mixed bag.
    #[test]
    fn carrier_effective_params_provider_mismatch_fails_typed() {
        let carrier = ProviderParamsCarrier {
            params: ProviderParamsOverride {
                provider_tag: Some(ProviderTag::Gemini(GeminiProviderTag::default())),
                ..Default::default()
            },
            tool_defaults: Some(ProviderTag::OpenAi(OpenAiProviderTag::default())),
        };
        let err = carrier.effective_params().expect_err("mismatch is a fault");
        assert_eq!(
            err,
            ProviderParamsMergeError::ProviderTagMismatch {
                explicit: "gemini",
                defaults: "openai",
            }
        );
    }

    #[test]
    fn carrier_effective_params_preserves_explicit_openai_store_false() {
        let carrier = ProviderParamsCarrier {
            params: ProviderParamsOverride {
                provider_tag: Some(ProviderTag::OpenAi(OpenAiProviderTag {
                    store: Some(false),
                    ..Default::default()
                })),
                ..Default::default()
            },
            tool_defaults: Some(ProviderTag::OpenAi(OpenAiProviderTag {
                store: Some(true),
                prompt_cache_key: Some("default-key".to_string()),
                ..Default::default()
            })),
        };

        let effective = carrier.effective_params().expect("merge succeeds");
        let Some(ProviderTag::OpenAi(tag)) = effective.provider_tag else {
            panic!("openai tag expected");
        };
        assert_eq!(tag.store, Some(false));
        assert_eq!(tag.prompt_cache_key.as_deref(), Some("default-key"));
    }

    /// K2 invariant: the carrier's durable face is exactly the typed override
    /// shape (transparent serde), and unknown keys fail closed at ingress.
    #[test]
    fn carrier_serde_is_transparent_and_fail_closed() {
        let parsed: ProviderParamsCarrier =
            serde_json::from_value(serde_json::json!({ "temperature": 0.7 }))
                .expect("typed shape parses");
        assert_eq!(parsed.params.temperature, Some(0.7));
        assert!(parsed.tool_defaults.is_none());

        let unknown =
            serde_json::from_value::<ProviderParamsCarrier>(serde_json::json!({ "thinking": {} }));
        assert!(
            unknown.is_err(),
            "legacy/unknown provider-params keys must be rejected at ingress"
        );

        // The durable face is exactly the typed override shape: one
        // `temperature` key (f32-backed, so the JSON number is the f32
        // value), and the re-parsed carrier is identical.
        let round = serde_json::to_value(&parsed).expect("serialize");
        let keys: Vec<&str> = round
            .as_object()
            .expect("carrier serializes as an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["temperature"]);
        let reparsed: ProviderParamsCarrier =
            serde_json::from_value(round).expect("round-trip parses");
        assert_eq!(reparsed, parsed);
    }

    /// K2 invariant: extraction structured-output injection goes through the
    /// typed ProviderTag owner and fails typed on identity conflicts.
    #[test]
    fn set_structured_output_typed_injection() {
        let schema = crate::OutputSchema::from_json_value(
            serde_json::json!({"type": "object", "properties": {}}),
        )
        .expect("schema");

        let mut params = ProviderParamsOverride::default();
        params
            .set_structured_output(Provider::Anthropic, schema.clone())
            .expect("inject into empty override");
        assert!(matches!(
            params.provider_tag,
            Some(ProviderTag::Anthropic(AnthropicProviderTag {
                structured_output: Some(_),
                ..
            }))
        ));

        let mut mismatched = ProviderParamsOverride {
            provider_tag: Some(ProviderTag::Gemini(GeminiProviderTag::default())),
            ..Default::default()
        };
        let err = mismatched
            .set_structured_output(Provider::Anthropic, schema.clone())
            .expect_err("identity conflict is typed");
        assert!(matches!(
            err,
            ProviderParamsMergeError::ProviderTagMismatch { .. }
        ));

        // `Other` has no provider-native slot: a typed capability outcome,
        // never a fault and never a silent injection.
        let mut other = ProviderParamsOverride::default();
        let outcome = other
            .set_structured_output(Provider::Other, schema)
            .expect("no-slot providers proceed prompt-based");
        assert_eq!(outcome, StructuredOutputInjection::NoProviderSlot);
        assert!(other.provider_tag.is_none());
    }

    #[test]
    fn opaque_provider_body_round_trip() {
        let v = serde_json::json!({"max_uses": 5, "allowed_domains": ["example.com"]});
        let body = OpaqueProviderBody::from_value(&v);
        assert_eq!(body.as_value(), v);
    }
}
