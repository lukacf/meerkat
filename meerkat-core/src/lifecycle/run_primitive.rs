//! §18 Run primitives — the ONLY input core receives from the runtime layer.
//!
//! Core's entire world is: conversation mutations, run boundaries, and staged inputs.
//! It knows nothing about input acceptance, policy, queueing, or topology.

use serde::{Deserialize, Serialize};

use super::identifiers::InputId;
use crate::connection::ConnectionRef;
use crate::provider::Provider;
use crate::service::TurnToolOverlay;
use crate::skills::SkillKey;
use crate::types::{HandlingMode, RenderMetadata};

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
    /// Reference to an external artifact.
    Reference { uri: String, label: Option<String> },
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

impl ProviderTag {
    /// Project a single-key legacy untyped per-turn value into a typed
    /// `ProviderTag`. Used by V3-row persistence projectors (C-TM-V3)
    /// where each key/value was stored separately. Multi-key JSON blobs
    /// at the provider-runtime boundary project through the per-provider
    /// `from_legacy_value` associated functions instead.
    pub fn from_legacy_value(
        namespace: impl Into<String>,
        key: impl Into<String>,
        value: &serde_json::Value,
    ) -> Self {
        let namespace = namespace.into();
        let key = key.into();

        if namespace == "anthropic"
            && key == "thinking"
            && let Some(budget) = value
                .get("budget_tokens")
                .and_then(serde_json::Value::as_u64)
                .and_then(|v| u32::try_from(v).ok())
        {
            return Self::Anthropic(AnthropicProviderTag {
                thinking: Some(AnthropicThinkingConfig::Enabled {
                    budget_tokens: budget,
                }),
                ..Default::default()
            });
        }

        if namespace == "openai"
            && key == "reasoning_effort"
            && let Some(effort) = value.as_str().and_then(|s| match s {
                "low" => Some(ReasoningEffort::Low),
                "medium" => Some(ReasoningEffort::Medium),
                "high" => Some(ReasoningEffort::High),
                _ => None,
            })
        {
            return Self::OpenAi(OpenAiProviderTag {
                reasoning_effort: Some(effort),
                ..Default::default()
            });
        }

        if namespace == "gemini"
            && key == "candidate_count"
            && let Some(count) = value.as_u64().and_then(|v| u32::try_from(v).ok())
        {
            return Self::Gemini(GeminiProviderTag {
                candidate_count: Some(count),
                ..Default::default()
            });
        }

        Self::Unknown {
            bag: StructuredProviderExtension {
                namespace,
                key,
                body: value.to_string(),
            },
        }
    }
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
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicThinkingConfig {
    /// Opus 4.6 adaptive thinking — provider picks the budget.
    Adaptive,
    /// Explicit budget — model emits at most `budget_tokens` tokens of
    /// reasoning before the assistant text.
    Enabled { budget_tokens: u32 },
}

/// Typed shape of Anthropic's response-effort knob (Opus 4.6+).
/// `XHigh` is the Opus 4.7 extended-high effort level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnthropicEffort {
    Low,
    Medium,
    High,
    Max,
    XHigh,
}

/// Typed shape of Anthropic's data-residency knob.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnthropicContextWindow {
    /// 1M-token beta context window (2025-08-07 beta header).
    OneMegabyte,
}

/// Typed shape of Anthropic's automatic-compaction knob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AnthropicCompactionConfig {
    /// `"auto"` — provider picks trigger and instructions.
    Auto,
    /// Caller-provided edit body merged into the compact edit shape.
    /// Fields like `trigger` / `instructions` are preserved verbatim.
    Custom { edit: OpaqueProviderBody },
}

/// Per-turn Anthropic-specific knobs carried in `ProviderTag::Anthropic`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
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
    /// Response-effort knob (Opus 4.6+).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<AnthropicEffort>,
    /// Structured-output schema (forces JSON-schema output envelope).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<crate::OutputSchema>,
    /// Data-residency override for inference routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inference_geo: Option<AnthropicInferenceGeo>,
    /// Automatic compaction configuration (Opus 4.6, beta).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<AnthropicCompactionConfig>,
    /// Context-window opt-in (1M beta).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<AnthropicContextWindow>,
    /// Internal override: force-enable temperature for this request even
    /// when the model profile says unsupported. Used by proxied /
    /// custom deployments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_temperature_override: Option<bool>,
}

/// Per-turn OpenAI-specific knobs carried in `ProviderTag::OpenAi`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
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
}

impl AnthropicProviderTag {
    /// Project a raw per-turn JSON object into a typed Anthropic tag.
    /// Unknown keys are preserved: the first unknown key surfaces as a
    /// `Err` so the caller can decide to return a typed error rather
    /// than silently drop. The runtime caller wraps any `Err` into
    /// `ProviderTag::Unknown { bag }` so wire round-trip stays lossless.
    pub fn from_legacy_value(value: &serde_json::Value) -> Result<Self, LegacyProviderParamsError> {
        let Some(obj) = value.as_object() else {
            if value.is_null() {
                return Ok(Self::default());
            }
            return Err(LegacyProviderParamsError::NotAnObject);
        };
        let mut tag = Self::default();
        for (k, v) in obj {
            match k.as_str() {
                "thinking" => {
                    if v.get("type").and_then(|t| t.as_str()) == Some("adaptive") {
                        tag.thinking = Some(AnthropicThinkingConfig::Adaptive);
                    } else if let Some(budget) = v
                        .get("budget_tokens")
                        .and_then(serde_json::Value::as_u64)
                        .and_then(|n| u32::try_from(n).ok())
                    {
                        tag.thinking = Some(AnthropicThinkingConfig::Enabled {
                            budget_tokens: budget,
                        });
                    } else {
                        return Err(LegacyProviderParamsError::unknown_shape("thinking"));
                    }
                }
                "thinking_budget" => {
                    let budget =
                        v.as_u64()
                            .and_then(|n| u32::try_from(n).ok())
                            .ok_or_else(|| {
                                LegacyProviderParamsError::unknown_shape("thinking_budget")
                            })?;
                    tag.thinking_budget_tokens = Some(budget);
                }
                "top_k" => {
                    let top_k = match v {
                        serde_json::Value::Number(n) => n
                            .as_u64()
                            .and_then(|n| u32::try_from(n).ok())
                            .ok_or_else(|| LegacyProviderParamsError::unknown_shape("top_k"))?,
                        serde_json::Value::String(s) => s
                            .parse::<u32>()
                            .map_err(|_| LegacyProviderParamsError::unknown_shape("top_k"))?,
                        _ => return Err(LegacyProviderParamsError::unknown_shape("top_k")),
                    };
                    tag.top_k = Some(top_k);
                }
                "effort" => {
                    let effort = match v.as_str() {
                        Some("low") => AnthropicEffort::Low,
                        Some("medium") => AnthropicEffort::Medium,
                        Some("high") => AnthropicEffort::High,
                        Some("max") => AnthropicEffort::Max,
                        Some("xhigh") => AnthropicEffort::XHigh,
                        _ => return Err(LegacyProviderParamsError::unknown_shape("effort")),
                    };
                    tag.effort = Some(effort);
                }
                "structured_output" => {
                    let schema =
                        serde_json::from_value::<crate::OutputSchema>(v.clone()).map_err(|e| {
                            LegacyProviderParamsError::InvalidStructuredOutput {
                                reason: e.to_string(),
                            }
                        })?;
                    tag.structured_output = Some(schema);
                }
                "inference_geo" => {
                    let geo = match v.as_str() {
                        Some("us") => AnthropicInferenceGeo::Us,
                        Some("global") => AnthropicInferenceGeo::Global,
                        Some(other) => AnthropicInferenceGeo::Other {
                            region: other.to_string(),
                        },
                        None => {
                            return Err(LegacyProviderParamsError::unknown_shape("inference_geo"));
                        }
                    };
                    tag.inference_geo = Some(geo);
                }
                "compaction" => {
                    if v.as_str() == Some("auto") {
                        tag.compaction = Some(AnthropicCompactionConfig::Auto);
                    } else if v.is_object() {
                        tag.compaction = Some(AnthropicCompactionConfig::Custom {
                            edit: OpaqueProviderBody::from_value(v),
                        });
                    } else {
                        return Err(LegacyProviderParamsError::unknown_shape("compaction"));
                    }
                }
                "context" => match v.as_str() {
                    Some("1m") => tag.context = Some(AnthropicContextWindow::OneMegabyte),
                    _ => return Err(LegacyProviderParamsError::unknown_shape("context")),
                },
                "web_search" => {
                    if v.is_object() {
                        tag.web_search = Some(OpaqueProviderBody::from_value(v));
                    } else if !v.is_boolean() && !v.is_null() {
                        return Err(LegacyProviderParamsError::unknown_shape("web_search"));
                    }
                }
                "__meerkat_supports_temperature" => {
                    tag.supports_temperature_override = v.as_bool();
                }
                other => {
                    return Err(LegacyProviderParamsError::UnknownKey {
                        key: other.to_string(),
                    });
                }
            }
        }
        Ok(tag)
    }
}

impl OpenAiProviderTag {
    pub fn from_legacy_value(value: &serde_json::Value) -> Result<Self, LegacyProviderParamsError> {
        let Some(obj) = value.as_object() else {
            if value.is_null() {
                return Ok(Self::default());
            }
            return Err(LegacyProviderParamsError::NotAnObject);
        };
        let mut tag = Self::default();
        for (k, v) in obj {
            match k.as_str() {
                "reasoning_effort" => {
                    let effort = match v.as_str() {
                        Some("low") => ReasoningEffort::Low,
                        Some("medium") => ReasoningEffort::Medium,
                        Some("high") => ReasoningEffort::High,
                        _ => {
                            return Err(LegacyProviderParamsError::unknown_shape(
                                "reasoning_effort",
                            ));
                        }
                    };
                    tag.reasoning_effort = Some(effort);
                }
                "seed" => {
                    tag.seed = v.as_i64();
                }
                "frequency_penalty" => {
                    let f = v.as_f64().ok_or_else(|| {
                        LegacyProviderParamsError::unknown_shape("frequency_penalty")
                    })?;
                    tag.frequency_penalty = Some(f as f32);
                }
                "presence_penalty" => {
                    let f = v.as_f64().ok_or_else(|| {
                        LegacyProviderParamsError::unknown_shape("presence_penalty")
                    })?;
                    tag.presence_penalty = Some(f as f32);
                }
                "web_search" => {
                    if v.is_object() {
                        tag.web_search = Some(OpaqueProviderBody::from_value(v));
                    } else if !v.is_boolean() && !v.is_null() {
                        return Err(LegacyProviderParamsError::unknown_shape("web_search"));
                    }
                }
                "structured_output" => {
                    let schema =
                        serde_json::from_value::<crate::OutputSchema>(v.clone()).map_err(|e| {
                            LegacyProviderParamsError::InvalidStructuredOutput {
                                reason: e.to_string(),
                            }
                        })?;
                    tag.structured_output = Some(schema);
                }
                "reasoning" => {
                    if v.is_object() {
                        tag.reasoning = Some(OpaqueProviderBody::from_value(v));
                    }
                }
                "chat_template_kwargs" => {
                    if v.is_object() {
                        tag.chat_template_kwargs = Some(OpaqueProviderBody::from_value(v));
                    }
                }
                "thinking" => {
                    if v.is_object() {
                        tag.thinking = Some(OpaqueProviderBody::from_value(v));
                    }
                }
                "__meerkat_supports_temperature" => {
                    tag.supports_temperature_override = v.as_bool();
                }
                "__meerkat_supports_reasoning" => {
                    tag.supports_reasoning_override = v.as_bool();
                }
                other => {
                    return Err(LegacyProviderParamsError::UnknownKey {
                        key: other.to_string(),
                    });
                }
            }
        }
        Ok(tag)
    }
}

impl GeminiProviderTag {
    pub fn from_legacy_value(value: &serde_json::Value) -> Result<Self, LegacyProviderParamsError> {
        let Some(obj) = value.as_object() else {
            if value.is_null() {
                return Ok(Self::default());
            }
            return Err(LegacyProviderParamsError::NotAnObject);
        };
        let mut tag = Self::default();
        for (k, v) in obj {
            match k.as_str() {
                "thinking" => {
                    let cfg = serde_json::from_value::<GeminiThinkingConfig>(v.clone())
                        .map_err(|_| LegacyProviderParamsError::unknown_shape("thinking"))?;
                    tag.thinking = Some(cfg);
                }
                "thinking_budget" => {
                    let b = v
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or_else(|| {
                            LegacyProviderParamsError::unknown_shape("thinking_budget")
                        })?;
                    tag.thinking_budget = Some(b);
                }
                "thinking_level" => {
                    let level = serde_json::from_value::<GeminiThinkingLevel>(v.clone())
                        .map_err(|_| LegacyProviderParamsError::unknown_shape("thinking_level"))?;
                    tag.thinking_level = Some(level);
                }
                "top_k" => {
                    let n = v
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or_else(|| LegacyProviderParamsError::unknown_shape("top_k"))?;
                    tag.top_k = Some(n);
                }
                "top_p" => {
                    let f = v
                        .as_f64()
                        .ok_or_else(|| LegacyProviderParamsError::unknown_shape("top_p"))?;
                    tag.top_p = Some(f as f32);
                }
                "structured_output" => {
                    let schema =
                        serde_json::from_value::<crate::OutputSchema>(v.clone()).map_err(|e| {
                            LegacyProviderParamsError::InvalidStructuredOutput {
                                reason: e.to_string(),
                            }
                        })?;
                    tag.structured_output = Some(schema);
                }
                "google_search" => {
                    if v.is_object() {
                        tag.google_search = Some(OpaqueProviderBody::from_value(v));
                    } else if !v.is_boolean() && !v.is_null() {
                        return Err(LegacyProviderParamsError::unknown_shape("google_search"));
                    }
                }
                "candidate_count" => {
                    let n = v
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or_else(|| {
                            LegacyProviderParamsError::unknown_shape("candidate_count")
                        })?;
                    tag.candidate_count = Some(n);
                }
                other => {
                    return Err(LegacyProviderParamsError::UnknownKey {
                        key: other.to_string(),
                    });
                }
            }
        }
        Ok(tag)
    }
}

/// Error returned by the per-provider `from_legacy_value` projectors
/// when an untyped legacy JSON shape cannot be mapped onto a typed
/// variant. Callers that must preserve the legacy body losslessly wrap
/// it into `ProviderTag::Unknown { bag: StructuredProviderExtension }`
/// instead of silently dropping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyProviderParamsError {
    NotAnObject,
    UnknownKey { key: String },
    UnknownShape { field: &'static str },
    InvalidStructuredOutput { reason: String },
}

impl LegacyProviderParamsError {
    fn unknown_shape(field: &'static str) -> Self {
        Self::UnknownShape { field }
    }
}

impl std::fmt::Display for LegacyProviderParamsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAnObject => f.write_str("legacy provider-params value is not an object"),
            Self::UnknownKey { key } => write!(f, "unknown legacy provider-params key: {key}"),
            Self::UnknownShape { field } => {
                write!(f, "legacy provider-params shape invalid for field {field}")
            }
            Self::InvalidStructuredOutput { reason } => {
                write!(f, "structured_output deserialize failed: {reason}")
            }
        }
    }
}

impl std::error::Error for LegacyProviderParamsError {}

/// Typed projection of OpenAI's reasoning-effort knob.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    Low,
    #[default]
    Medium,
    High,
}

/// Typed mode for generalized reasoning emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

    pub fn from_legacy_provider_value(provider: &str, value: &serde_json::Value) -> Self {
        let Some(obj) = value.as_object() else {
            if value.is_null() {
                return Self::default();
            }
            return Self {
                provider_tag: Some(unknown_provider_tag(provider, value)),
                ..Default::default()
            };
        };

        let mut remaining = serde_json::Map::new();
        let mut override_params = Self::default();

        for (key, item) in obj {
            match key.as_str() {
                "temperature" => {
                    if let Some(value) = item.as_f64() {
                        override_params.temperature = Some(value as f32);
                    } else {
                        remaining.insert(key.clone(), item.clone());
                    }
                }
                "top_p" => {
                    if let Some(value) = item.as_f64() {
                        override_params.top_p = Some(value as f32);
                    } else {
                        remaining.insert(key.clone(), item.clone());
                    }
                }
                "max_output_tokens" => {
                    if let Some(value) = item.as_u64().and_then(|value| u32::try_from(value).ok()) {
                        override_params.max_output_tokens = Some(value);
                    } else {
                        remaining.insert(key.clone(), item.clone());
                    }
                }
                "reasoning" => {
                    if let Some(value) = item.as_str().and_then(parse_reasoning_mode) {
                        override_params.reasoning = Some(value);
                    } else {
                        remaining.insert(key.clone(), item.clone());
                    }
                }
                "thinking_budget_tokens" => {
                    if let Some(value) = item.as_u64().and_then(|value| u32::try_from(value).ok()) {
                        override_params.thinking_budget_tokens = Some(value);
                    } else {
                        remaining.insert(key.clone(), item.clone());
                    }
                }
                _ => {
                    remaining.insert(key.clone(), item.clone());
                }
            }
        }

        if !remaining.is_empty() {
            let provider_value = serde_json::Value::Object(remaining);
            override_params.provider_tag =
                Some(project_legacy_provider_tag(provider, &provider_value));
        }

        override_params
    }
}

fn parse_reasoning_mode(value: &str) -> Option<ReasoningMode> {
    match value {
        "emit" => Some(ReasoningMode::Emit),
        "silent" => Some(ReasoningMode::Silent),
        "off" => Some(ReasoningMode::Off),
        _ => None,
    }
}

fn project_legacy_provider_tag(provider: &str, value: &serde_json::Value) -> ProviderTag {
    match provider {
        "anthropic" => AnthropicProviderTag::from_legacy_value(value)
            .map(ProviderTag::Anthropic)
            .unwrap_or_else(|_| unknown_provider_tag("anthropic", value)),
        "openai" => OpenAiProviderTag::from_legacy_value(value)
            .map(ProviderTag::OpenAi)
            .unwrap_or_else(|_| unknown_provider_tag("openai", value)),
        "gemini" | "google" => GeminiProviderTag::from_legacy_value(value)
            .map(ProviderTag::Gemini)
            .unwrap_or_else(|_| unknown_provider_tag("gemini", value)),
        other => unknown_provider_tag(other, value),
    }
}

fn unknown_provider_tag(provider: &str, value: &serde_json::Value) -> ProviderTag {
    ProviderTag::Unknown {
        bag: StructuredProviderExtension {
            namespace: provider.to_string(),
            key: "provider_params".to_string(),
            body: value.to_string(),
        },
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

/// Canonical per-turn runtime metadata carried alongside a
/// [`StagedRunInput`]. This is the typed seam consumed by the core layer —
/// `serde_json::Value` does not appear anywhere in this shape.
///
/// Construction in the runtime crate MUST go through the single canonical
/// `for_input(&Input)` constructor. Other code paths that previously built
/// a `RuntimeTurnMetadata` literal are updated to call `for_input` or be
/// deleted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
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
    /// Override provider-specific parameters for this turn (typed; no Value).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<ProviderParamsOverride>,
    /// Explicitly clear durable provider params. Omitted `provider_params`
    /// means inherit the current session value.
    #[serde(default, skip_serializing_if = "is_false")]
    pub clear_provider_params: bool,
    /// Explicit connection reference this turn must resolve against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_ref: Option<ConnectionRef>,
    /// Explicitly clear durable connection_ref. Omitted `connection_ref`
    /// means inherit the current session value.
    #[serde(default, skip_serializing_if = "is_false")]
    pub clear_connection_ref: bool,
    /// Keep-alive policy for materialized resources for this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<KeepAlivePolicy>,
    /// Optional normalized rendering metadata for this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_metadata: Option<RenderMetadata>,
    /// Typed execution intent classified by the runtime layer.
    ///
    /// `None` means the session layer should use its existing heuristic
    /// (backward compat for non-runtime substrate-direct paths).
    /// `Some(ContentTurn)` forces `run_turn`.
    /// `Some(ResumePending)` forces `run_pending`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_kind: Option<RuntimeExecutionKind>,
    /// Typed terminal peer-response apply intent classified by the runtime
    /// machine at admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_response_terminal_apply_intent: Option<PeerResponseTerminalApplyIntent>,
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
            && !self.clear_provider_params
            && self.connection_ref.is_none()
            && !self.clear_connection_ref
            && self.keep_alive.is_none()
            && self.render_metadata.is_none()
            && self.execution_kind.is_none()
            && self.peer_response_terminal_apply_intent.is_none()
    }

    /// Merge another metadata carrier into this one. Scalar conflicts (two
    /// inputs in a batch disagreeing on `model`, `provider`, `connection_ref`,
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
        reject_set_clear_conflict(
            self.provider_params.is_some(),
            self.clear_provider_params,
            other.provider_params.is_some(),
            other.clear_provider_params,
            "provider_params",
        )?;
        merge_scalar(
            &mut self.provider_params,
            other.provider_params,
            "provider_params",
        )?;
        self.clear_provider_params |= other.clear_provider_params;
        reject_set_clear_conflict(
            self.connection_ref.is_some(),
            self.clear_connection_ref,
            other.connection_ref.is_some(),
            other.clear_connection_ref,
            "connection_ref",
        )?;
        merge_scalar(
            &mut self.connection_ref,
            other.connection_ref,
            "connection_ref",
        )?;
        self.clear_connection_ref |= other.clear_connection_ref;
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

fn is_false(value: &bool) -> bool {
    !*value
}

fn reject_set_clear_conflict(
    lhs_set: bool,
    lhs_clear: bool,
    rhs_set: bool,
    rhs_clear: bool,
    field: &'static str,
) -> Result<(), TurnMetadataMergeConflict> {
    if (lhs_set && rhs_clear) || (lhs_clear && rhs_set) {
        return Err(TurnMetadataMergeConflict {
            field,
            reason: "one input sets the field while another clears it",
        });
    }
    Ok(())
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
// RuntimeTurnMetadata (model/provider/connection_ref overrides,
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
        use crate::types::{ContentBlock, ContentInput};
        match self {
            RunPrimitive::StagedInput(staged) => {
                let mut all_blocks = Vec::new();
                for append in &staged.appends {
                    match &append.content {
                        CoreRenderable::Text { text } => {
                            all_blocks.push(ContentBlock::Text { text: text.clone() });
                        }
                        CoreRenderable::Blocks { blocks } => {
                            all_blocks.extend(blocks.iter().cloned());
                        }
                        _ => {}
                    }
                }
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
            RunPrimitive::ImmediateAppend(append) => match &append.content {
                CoreRenderable::Text { text } => ContentInput::Text(text.clone()),
                CoreRenderable::Blocks { blocks } => ContentInput::Blocks(blocks.clone()),
                _ => ContentInput::Text(String::new()),
            },
            RunPrimitive::ImmediateContextAppend(ctx) => match &ctx.content {
                CoreRenderable::Text { text } => ContentInput::Text(text.clone()),
                CoreRenderable::Blocks { blocks } => ContentInput::Blocks(blocks.clone()),
                _ => ContentInput::Text(String::new()),
            },
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
        if !staged.appends.is_empty() || staged.context_appends.is_empty() {
            return Some(
                "terminal peer-response apply intent requires context-only staged appends",
            );
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
                key: "peer_response_terminal:analyst-rt:req-123".into(),
                content: CoreRenderable::Text {
                    text: "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL] done".into(),
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
        ] {
            let json = serde_json::to_value(role).unwrap();
            let parsed: ConversationAppendRole = serde_json::from_value(json).unwrap();
            assert_eq!(role, parsed);
        }
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
                keep_alive: Some(KeepAlivePolicy {
                    ttl: std::time::Duration::from_secs(30),
                    policy: KeepAliveMode::Pinned,
                }),
                ..Default::default()
            }),
        };
        let json = serde_json::to_value(&staged).unwrap();
        let parsed: StagedRunInput = serde_json::from_value(json).unwrap();
        assert_eq!(staged, parsed);
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

    #[test]
    fn anthropic_from_legacy_value_projects_multi_key_blob() {
        let v = serde_json::json!({
            "thinking": {"type": "adaptive"},
            "top_k": 40,
            "effort": "max",
            "inference_geo": "global",
            "context": "1m",
            "compaction": "auto",
            "__meerkat_supports_temperature": true,
        });
        let tag = AnthropicProviderTag::from_legacy_value(&v).expect("projects");
        assert_eq!(tag.thinking, Some(AnthropicThinkingConfig::Adaptive));
        assert_eq!(tag.top_k, Some(40));
        assert_eq!(tag.effort, Some(AnthropicEffort::Max));
        assert_eq!(tag.inference_geo, Some(AnthropicInferenceGeo::Global));
        assert_eq!(tag.context, Some(AnthropicContextWindow::OneMegabyte));
        assert_eq!(tag.compaction, Some(AnthropicCompactionConfig::Auto));
        assert_eq!(tag.supports_temperature_override, Some(true));
    }

    #[test]
    fn anthropic_from_legacy_value_unknown_key_errs() {
        let v = serde_json::json!({"unknown_key": 1});
        let err = AnthropicProviderTag::from_legacy_value(&v).unwrap_err();
        assert!(matches!(err, LegacyProviderParamsError::UnknownKey { .. }));
    }

    #[test]
    fn openai_from_legacy_value_projects_reasoning_and_penalties() {
        let v = serde_json::json!({
            "reasoning_effort": "high",
            "seed": 42,
            "frequency_penalty": 0.5,
            "presence_penalty": 0.3,
        });
        let tag = OpenAiProviderTag::from_legacy_value(&v).expect("projects");
        assert_eq!(tag.reasoning_effort, Some(ReasoningEffort::High));
        assert_eq!(tag.seed, Some(42));
        assert!(matches!(tag.frequency_penalty, Some(v) if (v - 0.5).abs() < 1e-6));
        assert!(matches!(tag.presence_penalty, Some(v) if (v - 0.3).abs() < 1e-6));
    }

    #[test]
    fn gemini_from_legacy_value_projects_thinking_and_top_knobs() {
        let v = serde_json::json!({
            "thinking": {"include_thoughts": true, "thinking_budget": 8000},
            "top_k": 40,
            "top_p": 0.95,
        });
        let tag = GeminiProviderTag::from_legacy_value(&v).expect("projects");
        let thinking = tag.thinking.expect("thinking present");
        assert_eq!(thinking.include_thoughts, Some(true));
        assert_eq!(thinking.thinking_budget, Some(8000));
        assert_eq!(tag.top_k, Some(40));
        assert!(matches!(tag.top_p, Some(v) if (v - 0.95).abs() < 1e-6));
    }

    #[test]
    fn opaque_provider_body_round_trip() {
        let v = serde_json::json!({"max_uses": 5, "allowed_domains": ["example.com"]});
        let body = OpaqueProviderBody::from_value(&v);
        assert_eq!(body.as_value(), v);
    }
}
