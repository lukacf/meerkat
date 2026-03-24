//! §18 Run primitives — the ONLY input core receives from the runtime layer.
//!
//! Core's entire world is: conversation mutations, run boundaries, and staged inputs.
//! It knows nothing about input acceptance, policy, queueing, or topology.

use serde::{Deserialize, Serialize};

use super::identifiers::InputId;
use crate::service::TurnToolOverlay;
use crate::skills::SkillKey;
use crate::types::HandlingMode;

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

/// An input staged for application at a run boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RuntimeTurnMetadata {
    /// Handling mode for staged ordinary work when admitted through runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handling_mode: Option<HandlingMode>,
    /// `None` = use session default; `Some(true)` = force keep-alive; `Some(false)` = force non-keep-alive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_references: Option<Vec<SkillKey>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_tool_overlay: Option<TurnToolOverlay>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
    /// Override model for this turn (hot-swap on materialized sessions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Override provider for this turn (hot-swap on materialized sessions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Override provider-specific parameters for this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<serde_json::Value>,
}

/// An input staged for application at a run boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "primitive_type", rename_all = "snake_case")]
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
                keep_alive: Some(true),
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
