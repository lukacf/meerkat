//! §8 Input types — the 6 input variants accepted by the runtime layer.
//!
//! Core never sees these. The runtime's policy table resolves each Input
//! to a PolicyDecision, then the runtime translates accepted Inputs into
//! RunPrimitive for core consumption.

use chrono::{DateTime, Utc};
use meerkat_core::lifecycle::InputId;
use meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata;
use meerkat_core::ops::{OpEvent, OperationId};
use meerkat_core::types::HandlingMode;
use meerkat_core::{
    BlobStore, BlobStoreError, MissingBlobBehavior, externalize_content_blocks,
    hydrate_content_blocks,
};
use serde::{Deserialize, Serialize};

use crate::identifiers::{
    CorrelationId, IdempotencyKey, KindId, LogicalRuntimeId, SupersessionKey,
};
use meerkat_core::types::RenderMetadata;

/// Common header for all input variants.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputHeader {
    /// Unique ID for this input.
    pub id: InputId,
    /// When the input was created.
    pub timestamp: DateTime<Utc>,
    /// Source of the input.
    pub source: InputOrigin,
    /// Durability requirement.
    pub durability: InputDurability,
    /// Visibility controls.
    pub visibility: InputVisibility,
    /// Optional idempotency key for dedup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<IdempotencyKey>,
    /// Optional supersession key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersession_key: Option<SupersessionKey>,
    /// Optional correlation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<CorrelationId>,
}

/// Where the input originated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum InputOrigin {
    /// Human operator / external API caller.
    Operator,
    /// Peer agent (comms).
    Peer {
        peer_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        runtime_id: Option<LogicalRuntimeId>,
    },
    /// Flow engine (mob orchestration).
    Flow { flow_id: String, step_index: usize },
    /// System-generated (compaction, projection, etc.).
    System,
    /// External event source.
    External { source_name: String },
}

/// Durability requirement for an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InputDurability {
    /// Must be persisted before acknowledgment.
    Durable,
    /// In-memory only, may be lost on crash.
    Ephemeral,
    /// Derived from other inputs (can be reconstructed).
    Derived,
}

/// Visibility controls for an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputVisibility {
    /// Whether this input appears in the conversation transcript.
    pub transcript_eligible: bool,
    /// Whether this input is visible to operator surfaces.
    pub operator_eligible: bool,
}

impl Default for InputVisibility {
    fn default() -> Self {
        Self {
            transcript_eligible: true,
            operator_eligible: true,
        }
    }
}

/// The 6 input variants accepted by the runtime layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "input_type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Input {
    /// User/operator prompt.
    Prompt(PromptInput),
    /// Peer-originated input (comms).
    Peer(PeerInput),
    /// Flow step input (mob orchestration).
    FlowStep(FlowStepInput),
    /// External event input.
    ExternalEvent(ExternalEventInput),
    /// Explicit runtime continuation work.
    #[serde(alias = "system_generated")]
    Continuation(ContinuationInput),
    /// Explicit non-content operation/lifecycle input.
    #[serde(alias = "projected")]
    Operation(OperationInput),
}

impl Input {
    /// Get the input header.
    pub fn header(&self) -> &InputHeader {
        match self {
            Input::Prompt(i) => &i.header,
            Input::Peer(i) => &i.header,
            Input::FlowStep(i) => &i.header,
            Input::ExternalEvent(i) => &i.header,
            Input::Continuation(i) => &i.header,
            Input::Operation(i) => &i.header,
        }
    }

    /// Get the input ID.
    pub fn id(&self) -> &InputId {
        &self.header().id
    }

    /// Get the kind ID for policy resolution.
    pub fn kind_id(&self) -> KindId {
        match self {
            Input::Prompt(_) => KindId::new("prompt"),
            Input::Peer(p) => match &p.convention {
                Some(PeerConvention::Message) => KindId::new("peer_message"),
                Some(PeerConvention::Request { .. }) => KindId::new("peer_request"),
                Some(PeerConvention::ResponseProgress { .. }) => {
                    KindId::new("peer_response_progress")
                }
                Some(PeerConvention::ResponseTerminal { .. }) => {
                    KindId::new("peer_response_terminal")
                }
                None => KindId::new("peer_message"),
            },
            Input::FlowStep(_) => KindId::new("flow_step"),
            Input::ExternalEvent(_) => KindId::new("external_event"),
            Input::Continuation(_) => KindId::new("continuation"),
            Input::Operation(_) => KindId::new("operation"),
        }
    }

    /// Handling-mode hint for ordinary work admitted through the runtime.
    pub fn handling_mode(&self) -> Option<HandlingMode> {
        match self {
            Input::Prompt(prompt) => prompt.turn_metadata.as_ref()?.handling_mode,
            Input::FlowStep(flow_step) => flow_step.turn_metadata.as_ref()?.handling_mode,
            Input::ExternalEvent(event) => Some(event.handling_mode),
            Input::Continuation(continuation) => Some(continuation.handling_mode),
            _ => None,
        }
    }
}

async fn externalize_payload_blocks(
    blob_store: &dyn BlobStore,
    payload: &mut serde_json::Value,
) -> Result<(), BlobStoreError> {
    let Some(obj) = payload.as_object_mut() else {
        return Ok(());
    };
    let Some(blocks_value) = obj.get_mut("blocks") else {
        return Ok(());
    };
    let mut blocks =
        serde_json::from_value::<Vec<meerkat_core::types::ContentBlock>>(blocks_value.clone())
            .map_err(|err| {
                BlobStoreError::Internal(format!("failed to decode payload blocks: {err}"))
            })?;
    externalize_content_blocks(blob_store, &mut blocks).await?;
    *blocks_value = serde_json::to_value(blocks).map_err(|err| {
        BlobStoreError::Internal(format!("failed to encode payload blocks: {err}"))
    })?;
    Ok(())
}

async fn hydrate_payload_blocks(
    blob_store: &dyn BlobStore,
    payload: &mut serde_json::Value,
    missing_behavior: MissingBlobBehavior,
) -> Result<(), BlobStoreError> {
    let Some(obj) = payload.as_object_mut() else {
        return Ok(());
    };
    let Some(blocks_value) = obj.get_mut("blocks") else {
        return Ok(());
    };
    let mut blocks =
        serde_json::from_value::<Vec<meerkat_core::types::ContentBlock>>(blocks_value.clone())
            .map_err(|err| {
                BlobStoreError::Internal(format!("failed to decode payload blocks: {err}"))
            })?;
    hydrate_content_blocks(blob_store, &mut blocks, missing_behavior).await?;
    *blocks_value = serde_json::to_value(blocks).map_err(|err| {
        BlobStoreError::Internal(format!("failed to encode payload blocks: {err}"))
    })?;
    Ok(())
}

pub async fn externalize_input_images(
    blob_store: &dyn BlobStore,
    input: &mut Input,
) -> Result<(), BlobStoreError> {
    match input {
        Input::Prompt(prompt) => {
            if let Some(blocks) = prompt.blocks.as_mut() {
                externalize_content_blocks(blob_store, blocks).await?;
            }
        }
        Input::Peer(peer) => {
            if let Some(blocks) = peer.blocks.as_mut() {
                externalize_content_blocks(blob_store, blocks).await?;
            }
        }
        Input::FlowStep(flow_step) => {
            if let Some(blocks) = flow_step.blocks.as_mut() {
                externalize_content_blocks(blob_store, blocks).await?;
            }
        }
        Input::ExternalEvent(event) => {
            if let Some(blocks) = event.blocks.as_mut() {
                externalize_content_blocks(blob_store, blocks).await?;
            }
            externalize_payload_blocks(blob_store, &mut event.payload).await?;
        }
        Input::Continuation(_) | Input::Operation(_) => {}
    }
    Ok(())
}

pub async fn hydrate_input_images(
    blob_store: &dyn BlobStore,
    input: &mut Input,
    missing_behavior: MissingBlobBehavior,
) -> Result<(), BlobStoreError> {
    match input {
        Input::Prompt(prompt) => {
            if let Some(blocks) = prompt.blocks.as_mut() {
                hydrate_content_blocks(blob_store, blocks, missing_behavior).await?;
            }
        }
        Input::Peer(peer) => {
            if let Some(blocks) = peer.blocks.as_mut() {
                hydrate_content_blocks(blob_store, blocks, missing_behavior).await?;
            }
        }
        Input::FlowStep(flow_step) => {
            if let Some(blocks) = flow_step.blocks.as_mut() {
                hydrate_content_blocks(blob_store, blocks, missing_behavior).await?;
            }
        }
        Input::ExternalEvent(event) => {
            if let Some(blocks) = event.blocks.as_mut() {
                hydrate_content_blocks(blob_store, blocks, missing_behavior).await?;
            }
            hydrate_payload_blocks(blob_store, &mut event.payload, missing_behavior).await?;
        }
        Input::Continuation(_) | Input::Operation(_) => {}
    }
    Ok(())
}

/// User/operator prompt input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptInput {
    pub header: InputHeader,
    /// The prompt text.
    pub text: String,
    /// Optional multimodal content blocks. When present, `text` serves as the
    /// text projection (backwards compat), and `blocks` carries the full content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<meerkat_core::types::ContentBlock>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_metadata: Option<RuntimeTurnMetadata>,
}

impl PromptInput {
    /// Create a new operator prompt with default header.
    pub fn new(text: impl Into<String>, turn_metadata: Option<RuntimeTurnMetadata>) -> Self {
        Self {
            header: InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: chrono::Utc::now(),
                source: InputOrigin::Operator,
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            text: text.into(),
            blocks: None,
            turn_metadata,
        }
    }

    /// Create a multimodal prompt from `ContentInput`.
    pub fn from_content_input(
        input: meerkat_core::types::ContentInput,
        turn_metadata: Option<RuntimeTurnMetadata>,
    ) -> Self {
        let text = input.text_content();
        let blocks = if input.has_images() {
            Some(input.into_blocks())
        } else {
            None
        };
        Self {
            header: InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: chrono::Utc::now(),
                source: InputOrigin::Operator,
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            text,
            blocks,
            turn_metadata,
        }
    }
}

/// Peer-originated input from comms.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInput {
    pub header: InputHeader,
    /// The peer convention (message, request, response).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub convention: Option<PeerConvention>,
    /// LLM-facing rendered text projection for this peer input.
    pub body: String,
    /// Optional multimodal content blocks. When present, `body` serves as the
    /// text projection (backwards compat), and `blocks` carries the full content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<meerkat_core::types::ContentBlock>>,
}

/// Peer communication conventions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "convention_type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PeerConvention {
    /// Simple peer-to-peer message.
    Message,
    /// Request expecting a response.
    Request { request_id: String, intent: String },
    /// Progress update for an ongoing response.
    ResponseProgress {
        request_id: String,
        phase: ResponseProgressPhase,
    },
    /// Terminal response (completed or failed).
    ResponseTerminal {
        request_id: String,
        status: ResponseTerminalStatus,
    },
}

/// Phase of a response progress update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ResponseProgressPhase {
    /// Request was accepted.
    Accepted,
    /// Work is in progress.
    InProgress,
    /// Partial result available.
    PartialResult,
}

/// Terminal status of a response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ResponseTerminalStatus {
    /// Request completed successfully.
    Completed,
    /// Request failed.
    Failed,
    /// Request was cancelled.
    Cancelled,
}

/// Flow step input from mob orchestration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowStepInput {
    pub header: InputHeader,
    /// Flow step identifier.
    pub step_id: String,
    /// Step instructions/prompt.
    pub instructions: String,
    /// Optional multimodal content blocks. When present, `instructions` serves
    /// as the text projection (backwards compat), and `blocks` carries the
    /// full content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<meerkat_core::types::ContentBlock>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_metadata: Option<RuntimeTurnMetadata>,
}

/// External event input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalEventInput {
    pub header: InputHeader,
    /// Event type/name.
    pub event_type: String,
    /// Event payload. Uses `Value` because the runtime layer may inspect/merge
    /// payloads during coalescing and projection — not a pure pass-through.
    pub payload: serde_json::Value,
    /// Optional multimodal blocks carried by the external event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<meerkat_core::types::ContentBlock>>,
    /// Runtime-owned handling hint for this external event.
    #[serde(default)]
    pub handling_mode: HandlingMode,
    /// Optional normalized render metadata carried with the event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_metadata: Option<RenderMetadata>,
}

/// Explicit continuation request that asks the runtime to keep draining
/// ordinary work after a boundary-local event (for example, terminal peer
/// responses injected into session state).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContinuationInput {
    pub header: InputHeader,
    /// Stable reason for the continuation request.
    pub reason: String,
    /// Ordinary-work handling mode for the continuation.
    #[serde(default)]
    pub handling_mode: HandlingMode,
    /// Optional request/correlation handle tied to the continuation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl ContinuationInput {
    /// Build the common runtime-owned continuation used after terminal peer
    /// response injection.
    pub fn terminal_peer_response(reason: impl Into<String>) -> Self {
        Self::terminal_peer_response_for_request(reason, None)
    }

    /// Build the common runtime-owned continuation used after terminal peer
    /// response injection, preserving the correlated request when known.
    pub fn terminal_peer_response_for_request(
        reason: impl Into<String>,
        request_id: Option<String>,
    ) -> Self {
        Self {
            header: InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: chrono::Utc::now(),
                source: InputOrigin::System,
                durability: InputDurability::Ephemeral,
                visibility: InputVisibility {
                    transcript_eligible: false,
                    operator_eligible: false,
                },
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            reason: reason.into(),
            handling_mode: HandlingMode::Steer,
            request_id,
        }
    }
}

/// Explicit operation/lifecycle input admitted through runtime instead of
/// being smuggled through transcript projections or peer-only paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationInput {
    pub header: InputHeader,
    /// Stable operation identifier.
    pub operation_id: OperationId,
    /// Typed lifecycle event for the operation.
    pub event: OpEvent,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_header() -> InputHeader {
        InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Operator,
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        }
    }

    #[test]
    fn prompt_input_serde() {
        let input = Input::Prompt(PromptInput {
            header: make_header(),
            text: "hello".into(),
            blocks: None,
            turn_metadata: None,
        });
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["input_type"], "prompt");
        let parsed: Input = serde_json::from_value(json).unwrap();
        assert!(matches!(parsed, Input::Prompt(_)));
    }

    #[test]
    fn peer_input_message_serde() {
        let input = Input::Peer(PeerInput {
            header: make_header(),
            convention: Some(PeerConvention::Message),
            body: "hi there".into(),
            blocks: None,
        });
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["input_type"], "peer");
        let parsed: Input = serde_json::from_value(json).unwrap();
        assert!(matches!(parsed, Input::Peer(_)));
    }

    #[test]
    fn peer_input_request_serde() {
        let input = Input::Peer(PeerInput {
            header: make_header(),
            convention: Some(PeerConvention::Request {
                request_id: "req-1".into(),
                intent: "mob.peer_added".into(),
            }),
            body: "Agent joined".into(),
            blocks: None,
        });
        let json = serde_json::to_value(&input).unwrap();
        let parsed: Input = serde_json::from_value(json).unwrap();
        if let Input::Peer(p) = parsed {
            assert!(matches!(p.convention, Some(PeerConvention::Request { .. })));
        } else {
            panic!("Expected PeerInput");
        }
    }

    #[test]
    fn peer_input_response_terminal_serde() {
        let input = Input::Peer(PeerInput {
            header: make_header(),
            convention: Some(PeerConvention::ResponseTerminal {
                request_id: "req-1".into(),
                status: ResponseTerminalStatus::Completed,
            }),
            body: "Done".into(),
            blocks: None,
        });
        let json = serde_json::to_value(&input).unwrap();
        let parsed: Input = serde_json::from_value(json).unwrap();
        assert!(matches!(parsed, Input::Peer(_)));
    }

    #[test]
    fn peer_input_response_progress_serde() {
        let input = Input::Peer(PeerInput {
            header: make_header(),
            convention: Some(PeerConvention::ResponseProgress {
                request_id: "req-1".into(),
                phase: ResponseProgressPhase::InProgress,
            }),
            body: "Working...".into(),
            blocks: None,
        });
        let json = serde_json::to_value(&input).unwrap();
        let parsed: Input = serde_json::from_value(json).unwrap();
        assert!(matches!(parsed, Input::Peer(_)));
    }

    #[test]
    fn flow_step_input_serde() {
        let input = Input::FlowStep(FlowStepInput {
            header: make_header(),
            step_id: "step-1".into(),
            instructions: "analyze the data".into(),
            blocks: Some(vec![
                meerkat_core::types::ContentBlock::Text {
                    text: "analyze the data".into(),
                },
                meerkat_core::types::ContentBlock::Image {
                    media_type: "image/png".into(),
                    data: meerkat_core::types::ImageData::Inline {
                        data: "abc123".into(),
                    },
                },
            ]),
            turn_metadata: None,
        });
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["input_type"], "flow_step");
        let parsed: Input = serde_json::from_value(json).unwrap();
        assert!(matches!(parsed, Input::FlowStep(_)));
    }

    #[test]
    fn external_event_input_serde() {
        let input = Input::ExternalEvent(ExternalEventInput {
            header: make_header(),
            event_type: "webhook.received".into(),
            payload: serde_json::json!({"url": "https://example.com"}),
            blocks: Some(vec![
                meerkat_core::types::ContentBlock::Text {
                    text: "look".into(),
                },
                meerkat_core::types::ContentBlock::Image {
                    media_type: "image/png".into(),
                    data: meerkat_core::types::ImageData::Inline {
                        data: "abc123".into(),
                    },
                },
            ]),
            handling_mode: HandlingMode::Queue,
            render_metadata: None,
        });
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["input_type"], "external_event");
        let parsed: Input = serde_json::from_value(json).unwrap();
        assert!(matches!(parsed, Input::ExternalEvent(_)));
    }

    #[test]
    fn continuation_input_serde() {
        let input = Input::Continuation(ContinuationInput::terminal_peer_response_for_request(
            "terminal peer response",
            Some("req-1".into()),
        ));
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["input_type"], "continuation");
        assert_eq!(json["request_id"], "req-1");
        let parsed: Input = serde_json::from_value(json).unwrap();
        match parsed {
            Input::Continuation(continuation) => {
                assert_eq!(continuation.request_id.as_deref(), Some("req-1"));
                assert_eq!(continuation.handling_mode, HandlingMode::Steer);
            }
            other => panic!("Expected Continuation, got {other:?}"),
        }
    }

    #[test]
    fn continuation_input_accepts_legacy_system_generated_tag() {
        let input = Input::Continuation(ContinuationInput::terminal_peer_response_for_request(
            "legacy system generated",
            Some("req-legacy".into()),
        ));
        let mut json = serde_json::to_value(&input).unwrap();
        json["input_type"] = serde_json::Value::String("system_generated".into());
        let parsed: Input = serde_json::from_value(json).unwrap();
        match parsed {
            Input::Continuation(continuation) => {
                assert_eq!(continuation.request_id.as_deref(), Some("req-legacy"));
            }
            other => panic!("Expected Continuation, got {other:?}"),
        }
    }

    #[test]
    fn operation_input_serde() {
        let input = Input::Operation(OperationInput {
            header: InputHeader {
                durability: InputDurability::Derived,
                ..make_header()
            },
            operation_id: OperationId::new(),
            event: OpEvent::Cancelled {
                id: OperationId::new(),
            },
        });
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["input_type"], "operation");
        let parsed: Input = serde_json::from_value(json).unwrap();
        assert!(matches!(parsed, Input::Operation(_)));
    }

    #[test]
    fn operation_input_accepts_legacy_projected_tag() {
        let input = Input::Operation(OperationInput {
            header: InputHeader {
                durability: InputDurability::Derived,
                ..make_header()
            },
            operation_id: OperationId::new(),
            event: OpEvent::Cancelled {
                id: OperationId::new(),
            },
        });
        let mut json = serde_json::to_value(&input).unwrap();
        json["input_type"] = serde_json::Value::String("projected".into());
        let parsed: Input = serde_json::from_value(json).unwrap();
        assert!(matches!(parsed, Input::Operation(_)));
    }

    #[test]
    fn input_kind_id() {
        let prompt = Input::Prompt(PromptInput {
            header: make_header(),
            text: "hi".into(),
            blocks: None,
            turn_metadata: None,
        });
        assert_eq!(prompt.kind_id().0, "prompt");

        let peer_msg = Input::Peer(PeerInput {
            header: make_header(),
            convention: Some(PeerConvention::Message),
            body: "hi".into(),
            blocks: None,
        });
        assert_eq!(peer_msg.kind_id().0, "peer_message");

        let peer_req = Input::Peer(PeerInput {
            header: make_header(),
            convention: Some(PeerConvention::Request {
                request_id: "r".into(),
                intent: "i".into(),
            }),
            body: "hi".into(),
            blocks: None,
        });
        assert_eq!(peer_req.kind_id().0, "peer_request");

        let continuation = Input::Continuation(ContinuationInput {
            header: make_header(),
            reason: "continue".into(),
            handling_mode: HandlingMode::Steer,
            request_id: None,
        });
        assert_eq!(continuation.kind_id().0, "continuation");

        let operation = Input::Operation(OperationInput {
            header: make_header(),
            operation_id: OperationId::new(),
            event: OpEvent::Cancelled {
                id: OperationId::new(),
            },
        });
        assert_eq!(operation.kind_id().0, "operation");
    }

    #[test]
    fn input_source_variants() {
        let sources = vec![
            InputOrigin::Operator,
            InputOrigin::Peer {
                peer_id: "p1".into(),
                runtime_id: None,
            },
            InputOrigin::Flow {
                flow_id: "f1".into(),
                step_index: 0,
            },
            InputOrigin::System,
            InputOrigin::External {
                source_name: "webhook".into(),
            },
        ];
        for source in sources {
            let json = serde_json::to_value(&source).unwrap();
            let parsed: InputOrigin = serde_json::from_value(json).unwrap();
            assert_eq!(source, parsed);
        }
    }

    #[test]
    fn input_durability_serde() {
        for d in [
            InputDurability::Durable,
            InputDurability::Ephemeral,
            InputDurability::Derived,
        ] {
            let json = serde_json::to_value(d).unwrap();
            let parsed: InputDurability = serde_json::from_value(json).unwrap();
            assert_eq!(d, parsed);
        }
    }
}
