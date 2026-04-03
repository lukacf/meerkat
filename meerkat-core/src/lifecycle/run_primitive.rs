//! §18 Run primitives — the ONLY input core receives from the runtime layer.
//!
//! Core's entire world is: conversation mutations, run boundaries, and staged inputs.
//! It knows nothing about input acceptance, policy, queueing, or topology.

use serde::{Deserialize, Serialize};

use super::identifiers::InputId;
use crate::service::TurnToolOverlay;
use crate::session::PendingSystemContextAppend;
use crate::skills::SkillKey;
use crate::time_compat::SystemTime;
use crate::types::{ContentBlock, ContentInput, HandlingMode, RenderMetadata, text_content};

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

impl CoreRenderable {
    /// Canonical text projection when a richer renderable crosses a text-only seam.
    pub fn text_projection(&self) -> String {
        match self {
            Self::Text { text } => text.clone(),
            Self::Blocks { blocks } => text_content(blocks),
            Self::Json { value } => {
                serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
            }
            Self::Reference { uri, label } => match label {
                Some(label) if !label.trim().is_empty() => {
                    format!("[Reference] {label} ({uri})")
                }
                _ => format!("[Reference] {uri}"),
            },
        }
    }

    /// Canonical runtime-backed projection into content blocks.
    pub fn append_content_blocks(&self, blocks: &mut Vec<ContentBlock>) {
        match self {
            Self::Text { text } => blocks.push(ContentBlock::Text { text: text.clone() }),
            Self::Blocks {
                blocks: render_blocks,
            } => blocks.extend(render_blocks.iter().cloned()),
            Self::Json { .. } | Self::Reference { .. } => blocks.push(ContentBlock::Text {
                text: self.text_projection(),
            }),
        }
    }

    pub fn to_content_input(&self) -> ContentInput {
        let mut blocks = Vec::new();
        self.append_content_blocks(&mut blocks);
        content_blocks_to_input(blocks)
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
    /// Canonical text content for runtime system context.
    pub text: String,
}

impl ConversationContextAppend {
    pub fn into_pending_system_context_append(self) -> PendingSystemContextAppend {
        PendingSystemContextAppend {
            text: self.text,
            source: Some(self.key),
            idempotency_key: None,
            accepted_at: SystemTime::now(),
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeApplyAction {
    StartTurn {
        prompt: ContentInput,
    },
    ApplySystemContextOnly {
        appends: Vec<PendingSystemContextAppend>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeApplyPlan {
    pub boundary: RunApplyBoundary,
    pub contributing_input_ids: Vec<InputId>,
    pub action: RuntimeApplyAction,
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

    /// Canonical runtime/session apply seam shared by all transport surfaces.
    pub fn runtime_apply_plan(&self) -> RuntimeApplyPlan {
        match self {
            RunPrimitive::StagedInput(staged) => {
                let boundary = staged.boundary;
                let contributing_input_ids = staged.contributing_input_ids.clone();
                let context_appends = staged
                    .context_appends
                    .clone()
                    .into_iter()
                    .map(ConversationContextAppend::into_pending_system_context_append)
                    .collect::<Vec<_>>();

                if staged.appends.is_empty()
                    && !context_appends.is_empty()
                    && boundary == RunApplyBoundary::Immediate
                {
                    return RuntimeApplyPlan {
                        boundary,
                        contributing_input_ids,
                        action: RuntimeApplyAction::ApplySystemContextOnly {
                            appends: context_appends,
                        },
                    };
                }

                RuntimeApplyPlan {
                    boundary,
                    contributing_input_ids,
                    action: RuntimeApplyAction::StartTurn {
                        prompt: conversation_appends_to_content_input(&staged.appends),
                    },
                }
            }
            RunPrimitive::ImmediateAppend(append) => RuntimeApplyPlan {
                boundary: RunApplyBoundary::Immediate,
                contributing_input_ids: Vec::new(),
                action: RuntimeApplyAction::StartTurn {
                    prompt: append.content.to_content_input(),
                },
            },
            RunPrimitive::ImmediateContextAppend(append) => RuntimeApplyPlan {
                boundary: RunApplyBoundary::Immediate,
                contributing_input_ids: Vec::new(),
                action: RuntimeApplyAction::ApplySystemContextOnly {
                    appends: vec![append.clone().into_pending_system_context_append()],
                },
            },
        }
    }
}

fn conversation_appends_to_content_input(appends: &[ConversationAppend]) -> ContentInput {
    let mut blocks = Vec::new();
    for append in appends {
        append.content.append_content_blocks(&mut blocks);
    }
    content_blocks_to_input(blocks)
}

fn content_blocks_to_input(blocks: Vec<ContentBlock>) -> ContentInput {
    if blocks.len() == 1
        && let ContentBlock::Text { text } = &blocks[0]
    {
        return ContentInput::Text(text.clone());
    }
    if blocks.is_empty() {
        ContentInput::Text(String::new())
    } else {
        ContentInput::Blocks(blocks)
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
    fn core_renderable_to_content_input_projects_json_to_text() {
        let renderable = CoreRenderable::Json {
            value: serde_json::json!({"hello": "world"}),
        };
        match renderable.to_content_input() {
            ContentInput::Text(text) => assert!(text.contains("\"hello\"")),
            other => panic!("expected text projection, got {other:?}"),
        }
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
            text: "peer1\npeer2".into(),
        };
        let json = serde_json::to_value(&ctx).unwrap();
        assert_eq!(json["text"], "peer1\npeer2");
        let parsed: ConversationContextAppend = serde_json::from_value(json).unwrap();
        assert_eq!(ctx, parsed);
    }

    #[test]
    fn runtime_apply_plan_uses_context_only_action_for_immediate_context_primitive() {
        let primitive = RunPrimitive::ImmediateContextAppend(ConversationContextAppend {
            key: "env".into(),
            text: "linux".into(),
        });
        let plan = primitive.runtime_apply_plan();
        match plan.action {
            RuntimeApplyAction::ApplySystemContextOnly { appends } => {
                assert_eq!(appends.len(), 1);
                assert_eq!(appends[0].text, "linux");
                assert_eq!(appends[0].source.as_deref(), Some("env"));
            }
            other => panic!("expected context-only action, got {other:?}"),
        }
    }

    #[test]
    fn runtime_apply_plan_projects_json_and_reference_prompt_content() {
        let primitive = RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::RunStart,
            appends: vec![
                ConversationAppend {
                    role: ConversationAppendRole::User,
                    content: CoreRenderable::Json {
                        value: serde_json::json!({"hello": "world"}),
                    },
                },
                ConversationAppend {
                    role: ConversationAppendRole::User,
                    content: CoreRenderable::Reference {
                        uri: "file:///tmp/a.txt".into(),
                        label: Some("artifact".into()),
                    },
                },
            ],
            context_appends: vec![],
            contributing_input_ids: vec![InputId::new()],
            turn_metadata: None,
        });

        let plan = primitive.runtime_apply_plan();
        match plan.action {
            RuntimeApplyAction::StartTurn {
                prompt: ContentInput::Blocks(blocks),
            } => {
                assert_eq!(blocks.len(), 2);
                match &blocks[0] {
                    ContentBlock::Text { text } => assert!(text.contains("\"hello\"")),
                    other => panic!("expected text block, got {other:?}"),
                }
                match &blocks[1] {
                    ContentBlock::Text { text } => {
                        assert!(text.contains("artifact"));
                        assert!(text.contains("file:///tmp/a.txt"));
                    }
                    other => panic!("expected text block, got {other:?}"),
                }
            }
            other => panic!("expected start-turn action, got {other:?}"),
        }
    }
}
