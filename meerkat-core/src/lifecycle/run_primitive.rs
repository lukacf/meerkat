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
    /// Project a legacy untyped per-turn value into a typed `ProviderTag`.
    ///
    /// V3 rows stored provider knobs as `serde_json::Value`. This helper
    /// maps the well-known shapes onto typed variants (e.g. Anthropic's
    /// `thinking: { budget_tokens: N }`) and falls through to
    /// `Unknown { bag }` when the shape is unrecognised — lossless round-
    /// trip instead of silent drop.
    pub fn from_legacy_value(
        namespace: impl Into<String>,
        key: impl Into<String>,
        value: &serde_json::Value,
    ) -> Self {
        let namespace = namespace.into();
        let key = key.into();

        // Anthropic `thinking: { type: "enabled", budget_tokens: N }` was
        // the one V3 row shape most at risk of silent drop (persistence-
        // migration.md §5 fixture #4). Project it directly.
        if namespace == "anthropic" && key == "thinking" {
            if let Some(budget) = value
                .get("budget_tokens")
                .and_then(serde_json::Value::as_u64)
                .and_then(|v| u32::try_from(v).ok())
            {
                return Self::Anthropic(AnthropicProviderTag {
                    thinking_budget_tokens: Some(budget),
                });
            }
        }

        // OpenAI `reasoning_effort: "low"|"medium"|"high"`.
        if namespace == "openai" && key == "reasoning_effort" {
            if let Some(effort) = value.as_str().and_then(|s| match s {
                "low" => Some(ReasoningEffort::Low),
                "medium" => Some(ReasoningEffort::Medium),
                "high" => Some(ReasoningEffort::High),
                _ => None,
            }) {
                return Self::OpenAi(OpenAiProviderTag {
                    reasoning_effort: Some(effort),
                });
            }
        }

        // Gemini `candidate_count: N`.
        if namespace == "gemini" && key == "candidate_count" {
            if let Some(count) = value.as_u64().and_then(|v| u32::try_from(v).ok()) {
                return Self::Gemini(GeminiProviderTag {
                    candidate_count: Some(count),
                });
            }
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AnthropicProviderTag {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_budget_tokens: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct OpenAiProviderTag {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct GeminiProviderTag {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_count: Option<u32>,
}

/// Typed projection of OpenAI's reasoning-effort knob.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}

impl Default for ReasoningEffort {
    fn default() -> Self {
        Self::Medium
    }
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
    /// Explicit connection reference this turn must resolve against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_ref: Option<ConnectionRef>,
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
            && self.connection_ref.is_none()
            && self.keep_alive.is_none()
            && self.render_metadata.is_none()
            && self.execution_kind.is_none()
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
        merge_scalar(
            &mut self.provider_params,
            other.provider_params,
            "provider_params",
        )?;
        merge_scalar(
            &mut self.connection_ref,
            other.connection_ref,
            "connection_ref",
        )?;
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
#[allow(clippy::unwrap_used)]
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
}
