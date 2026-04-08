//! §18 Run primitives — the ONLY input core receives from the runtime layer.
//!
//! Core's entire world is: conversation mutations, run boundaries, and staged inputs.
//! It knows nothing about input acceptance, policy, queueing, or topology.

use serde::{Deserialize, Serialize};

use super::identifiers::InputId;
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
