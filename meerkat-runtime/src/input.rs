//! §8 Input types — the 6 input variants accepted by the runtime layer.
//!
//! Core never sees these. The runtime's policy table resolves each Input
//! to a PolicyDecision, then the runtime translates accepted Inputs into
//! RunPrimitive for core consumption.

use chrono::{DateTime, Utc};
use meerkat_core::lifecycle::InputId;
use meerkat_core::lifecycle::run_primitive::{
    ConversationAppend, ConversationAppendRole, ConversationContextAppend, CoreRenderable,
    RuntimeTurnMetadata,
};
use meerkat_core::ops::{OpEvent, OperationId};
use meerkat_core::types::HandlingMode;
use meerkat_core::{
    BlobStore, BlobStoreError, MissingBlobBehavior, PeerConversationProjection,
    PeerResponseProgressProjectionPhase, PeerResponseTerminalCorrelationId,
    PeerResponseTerminalDisplayIdentity, PeerResponseTerminalFact, PeerResponseTerminalFactError,
    PeerResponseTerminalProjectionStatus, PeerResponseTerminalRenderPayload,
    PeerResponseTerminalRouteIdentity, PeerResponseTerminalSource,
    PeerResponseTerminalTransportIdentity, externalize_content_blocks, hydrate_content_blocks,
};
use serde::{Deserialize, Serialize};

use crate::identifiers::{
    CorrelationId, IdempotencyKey, InputKind, KindId, LogicalRuntimeId, SupersessionKey,
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
        /// Canonical comms peer id used by machine/runtime policy and schema
        /// projection.
        peer_id: String,
        /// Optional display/source label admitted at peer ingress. This is
        /// presentation metadata only; routing and trust must use typed ingress
        /// facts before the runtime input seam or the canonical `peer_id`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_identity: Option<String>,
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

    /// Typed kind for policy dispatch.
    pub fn kind(&self) -> InputKind {
        match self {
            Input::Prompt(_) => InputKind::Prompt,
            Input::Peer(p) => match &p.convention {
                Some(PeerConvention::Message) | None => InputKind::PeerMessage,
                Some(PeerConvention::Request { .. }) => InputKind::PeerRequest,
                Some(PeerConvention::ResponseProgress { .. }) => InputKind::PeerResponseProgress,
                Some(PeerConvention::ResponseTerminal { .. }) => InputKind::PeerResponseTerminal,
            },
            Input::FlowStep(_) => InputKind::FlowStep,
            Input::ExternalEvent(_) => InputKind::ExternalEvent,
            Input::Continuation(_) => InputKind::Continuation,
            Input::Operation(_) => InputKind::Operation,
        }
    }

    /// Wrapped kind identifier (for newtype-discipline call sites).
    pub fn kind_id(&self) -> KindId {
        KindId::new(self.kind())
    }

    /// Handling-mode hint for ordinary work admitted through the runtime.
    pub fn handling_mode(&self) -> Option<HandlingMode> {
        match self {
            Input::Prompt(prompt) => prompt.turn_metadata.as_ref()?.handling_mode,
            Input::FlowStep(flow_step) => flow_step.turn_metadata.as_ref()?.handling_mode,
            Input::ExternalEvent(event) => Some(event.handling_mode),
            Input::Continuation(continuation) => Some(continuation.handling_mode),
            Input::Peer(peer) => peer.handling_mode,
            Input::Operation(_) => None,
        }
    }
}

fn migrate_legacy_payload_blocks(event: &mut ExternalEventInput) -> Result<(), BlobStoreError> {
    let Some(obj) = event.payload.as_object_mut() else {
        return Ok(());
    };
    let Some(blocks_value) = obj.remove("blocks") else {
        return Ok(());
    };
    if event.blocks.is_some() {
        return Ok(());
    }
    let blocks = serde_json::from_value::<Vec<meerkat_core::types::ContentBlock>>(blocks_value)
        .map_err(|err| {
            BlobStoreError::Internal(format!("failed to decode payload blocks: {err}"))
        })?;
    event.blocks = Some(blocks);
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
            migrate_legacy_payload_blocks(event)?;
            if let Some(blocks) = event.blocks.as_mut() {
                externalize_content_blocks(blob_store, blocks).await?;
            }
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
            migrate_legacy_payload_blocks(event)?;
            if let Some(blocks) = event.blocks.as_mut() {
                hydrate_content_blocks(blob_store, blocks, missing_behavior).await?;
            }
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
    /// Legacy textual body for this peer input.
    ///
    /// Message-style peer traffic uses this directly. Request/response prompt
    /// projection is runtime-owned and must be reconstructed from
    /// `convention + payload + source` rather than helper-rendered prose.
    pub body: String,
    /// Structured peer payload, when one exists.
    ///
    /// For `Request`, this is the request params. For `Response*`, this is the
    /// response result payload. Message traffic leaves this unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    /// Optional multimodal content blocks. When present, `body` serves as the
    /// text projection (backwards compat), and `blocks` carries the full content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<meerkat_core::types::ContentBlock>>,
    /// Optional handling-mode override for actionable peer inputs.
    /// When present on Message/Request/no-convention, overrides kind-based
    /// policy defaults. Forbidden on ResponseProgress and ResponseTerminal
    /// (enforced by [`validate_peer_handling_mode`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handling_mode: Option<HandlingMode>,
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

/// Phase of a response progress update. This is the core projection enum, not
/// a runtime-local duplicate.
pub type ResponseProgressPhase = PeerResponseProgressProjectionPhase;

/// Terminal status of a response. This is the core projection enum, not a
/// runtime-local duplicate.
pub type ResponseTerminalStatus = PeerResponseTerminalProjectionStatus;

pub fn response_terminal_status_from_wire(
    status: meerkat_contracts::PeerResponseTerminalStatusWire,
) -> ResponseTerminalStatus {
    match status {
        meerkat_contracts::PeerResponseTerminalStatusWire::Completed => {
            PeerResponseTerminalProjectionStatus::Completed
        }
        meerkat_contracts::PeerResponseTerminalStatusWire::Failed => {
            PeerResponseTerminalProjectionStatus::Failed
        }
        meerkat_contracts::PeerResponseTerminalStatusWire::Cancelled => {
            PeerResponseTerminalProjectionStatus::Cancelled
        }
    }
}

pub fn peer_response_terminal_input(
    peer_id: meerkat_core::comms::PeerId,
    display_name: Option<meerkat_core::comms::PeerName>,
    request_id: meerkat_core::PeerCorrelationId,
    status: meerkat_contracts::PeerResponseTerminalStatusWire,
    result: serde_json::Value,
) -> Input {
    let correlation_id = CorrelationId::from_uuid(request_id.as_uuid());
    let request_id = request_id.to_string();
    let peer_id = peer_id.to_string();
    let display_identity = display_name.map_or_else(|| peer_id.clone(), |name| name.as_string());

    Input::Peer(PeerInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Peer {
                peer_id,
                display_identity: Some(display_identity),
                runtime_id: None,
            },
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: Some(correlation_id),
        },
        convention: Some(PeerConvention::ResponseTerminal {
            request_id,
            status: response_terminal_status_from_wire(status),
        }),
        body: String::new(),
        payload: Some(result),
        blocks: None,
        handling_mode: None,
    })
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
    /// Multimodal content does NOT live here canonically; use `blocks`.
    pub payload: serde_json::Value,
    /// Optional multimodal blocks carried by the external event. This is the
    /// canonical owner for multimodal external-event content.
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
    /// Build a continuation for waking an idle session after a detached
    /// background operation reaches terminal state.
    ///
    /// Properties: `Derived` durability, invisible to transcript and operator,
    /// `System` origin, `Steer` handling mode.
    pub fn detached_background_op_completed() -> Self {
        Self {
            header: InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: chrono::Utc::now(),
                source: InputOrigin::System,
                durability: InputDurability::Derived,
                visibility: InputVisibility {
                    transcript_eligible: false,
                    operator_eligible: false,
                },
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            reason: "detached_background_op_completed".to_string(),
            handling_mode: HandlingMode::Steer,
            request_id: None,
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

/// Build the core-owned peer conversation projection for a runtime peer input.
///
/// Peer-response terminal context projection is deliberately excluded here:
/// admission must not store it as pre-machine truth. Runtime-loop batch
/// construction uses [`runtime_input_projection_for_machine_batch`] after the
/// machine-selected input is dequeued.
pub(crate) fn peer_projection_from_peer_input(
    peer: &PeerInput,
) -> Option<PeerConversationProjection> {
    peer_projection_from_peer_input_with_id(peer, peer_canonical_id(peer)?.as_str())
}

fn peer_projection_from_peer_input_with_id(
    peer: &PeerInput,
    peer_id: &str,
) -> Option<PeerConversationProjection> {
    let peer_id = peer_id.to_string();

    match &peer.convention {
        Some(PeerConvention::Message) => Some(PeerConversationProjection::Message { peer_id }),
        Some(PeerConvention::Request { request_id, intent }) => {
            let peer_id = match meerkat_core::comms::PeerId::parse(peer_id.as_str()) {
                Ok(peer_id) => peer_id,
                Err(error) => {
                    tracing::warn!(
                        peer_id,
                        error = %error,
                        "dropping peer request projection with non-canonical peer_id"
                    );
                    return None;
                }
            };
            Some(PeerConversationProjection::Request {
                peer_id,
                display_name: peer_display_label(peer),
                request_id: request_id.clone(),
                intent: intent.clone(),
                payload: peer.payload.clone(),
            })
        }
        Some(PeerConvention::ResponseProgress { request_id, phase }) => {
            Some(PeerConversationProjection::ResponseProgress {
                peer_id,
                request_id: request_id.clone(),
                phase: *phase,
                payload: peer.payload.clone(),
            })
        }
        Some(PeerConvention::ResponseTerminal { .. }) => None,
        None => None,
    }
}

pub(crate) fn peer_response_terminal_fact(
    peer: &PeerInput,
) -> Result<Option<PeerResponseTerminalFact>, PeerResponseTerminalFactError> {
    let InputOrigin::Peer {
        peer_id,
        display_identity,
        runtime_id,
    } = &peer.header.source
    else {
        return Ok(None);
    };
    let Some(PeerConvention::ResponseTerminal { request_id, status }) = &peer.convention else {
        return Ok(None);
    };

    let transport_identity = runtime_id
        .as_ref()
        .map(ToString::to_string)
        .map(PeerResponseTerminalTransportIdentity::parse)
        .transpose()?;
    let source = PeerResponseTerminalSource::new(
        transport_identity,
        PeerResponseTerminalRouteIdentity::parse(peer_id.clone())?,
        PeerResponseTerminalDisplayIdentity::parse(
            display_identity
                .as_ref()
                .ok_or(PeerResponseTerminalFactError::MissingDisplayIdentity)?
                .clone(),
        )?,
    );
    Ok(Some(PeerResponseTerminalFact::new(
        source,
        PeerResponseTerminalCorrelationId::parse(request_id)?,
        *status,
        PeerResponseTerminalRenderPayload::new(peer.payload.clone()),
    )))
}

pub(crate) fn validate_peer_response_terminal_fact(
    input: &Input,
) -> Result<(), PeerResponseTerminalFactError> {
    let Input::Peer(peer) = input else {
        return Ok(());
    };
    peer_response_terminal_fact(peer).map(|_| ())
}

/// Lift an [`Input`] to its core peer projection when it is a peer input with a
/// peer-origin header.
#[cfg(test)]
pub(crate) fn peer_projection(input: &Input) -> Option<PeerConversationProjection> {
    let Input::Peer(peer) = input else {
        return None;
    };
    peer_projection_from_peer_input(peer)
}

fn peer_canonical_id(peer: &PeerInput) -> Option<String> {
    let InputOrigin::Peer { peer_id, .. } = &peer.header.source else {
        return None;
    };
    Some(peer_id.clone())
}

fn peer_display_label(peer: &PeerInput) -> Option<String> {
    let InputOrigin::Peer {
        display_identity, ..
    } = &peer.header.source
    else {
        return None;
    };

    display_identity
        .as_ref()
        .map(|label| label.trim())
        .filter(|label| !label.is_empty())
        .map(ToOwned::to_owned)
}

/// Rendered prompt-text projection for a peer input.
pub(crate) fn peer_prompt_text(peer: &PeerInput) -> String {
    peer_projection_from_peer_input(peer)
        .map(|projection| {
            let prompt = projection.prompt_text();
            if prompt.is_empty() {
                peer.body.clone()
            } else {
                prompt
            }
        })
        .unwrap_or_else(|| peer.body.clone())
}

/// Optional block prefix for peer message inputs.
pub(crate) fn peer_block_prefix_text(peer: &PeerInput) -> Option<String> {
    if matches!(peer.convention, Some(PeerConvention::Message))
        && let Some(prefix) = rendered_message_prefix(&peer.body)
    {
        return Some(prefix);
    }

    peer_projection_from_peer_input(peer).and_then(|projection| projection.block_prefix_text())
}

fn rendered_message_prefix(body: &str) -> Option<String> {
    let prefix = body.lines().next()?.trim();
    if prefix.starts_with("[COMMS MESSAGE from ") && prefix.ends_with(']') {
        Some(prefix.to_string())
    } else {
        None
    }
}

fn rendered_message_body_text(body: &str, prefix: &str) -> Option<String> {
    let text = body
        .lines()
        .skip_while(|line| line.trim() != prefix)
        .skip(1)
        .map(str::trim)
        .filter(|line| !line.is_empty() && !is_media_projection_line(line))
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() { None } else { Some(text) }
}

fn is_media_projection_line(line: &str) -> bool {
    (line.starts_with("[image:") || line.starts_with("[video:")) && line.ends_with(']')
}

fn blocks_include_text_projection(
    blocks: &[meerkat_core::types::ContentBlock],
    expected: &str,
) -> bool {
    let expected = expected.trim();
    if expected.is_empty() {
        return true;
    }
    if blocks
        .iter()
        .any(|block| matches!(block, meerkat_core::types::ContentBlock::Text { text } if text.trim() == expected))
    {
        return true;
    }
    let joined = blocks
        .iter()
        .filter_map(|block| match block {
            meerkat_core::types::ContentBlock::Text { text } => Some(text.trim()),
            _ => None,
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    joined == expected
}

pub(crate) fn input_prompt_text(input: &Input) -> String {
    match input {
        Input::Prompt(p) => p.text.clone(),
        Input::Peer(p) => peer_prompt_text(p),
        Input::FlowStep(f) => f.instructions.clone(),
        Input::ExternalEvent(e) => external_event_projection_text(e),
        Input::Continuation(continuation) => format!("[Continuation] {}", continuation.reason),
        Input::Operation(operation) => {
            format!(
                "[Operation {}] {:?}",
                operation.operation_id, operation.event
            )
        }
    }
}

fn external_event_projection_text(event: &ExternalEventInput) -> String {
    let source_name = match &event.header.source {
        InputOrigin::External { source_name } if !source_name.trim().is_empty() => {
            source_name.as_str()
        }
        _ => event.event_type.as_str(),
    };
    let body = event
        .payload
        .get("body")
        .and_then(serde_json::Value::as_str)
        .map(str::trim);

    meerkat_core::interaction::format_external_event_projection(source_name, body)
}

fn input_to_append(input: &Input) -> Option<ConversationAppend> {
    if matches!(
        input,
        Input::Peer(PeerInput {
            convention: Some(PeerConvention::ResponseTerminal { .. }),
            ..
        })
    ) {
        return None;
    }

    let content = match input {
        Input::Prompt(p) if p.blocks.is_some() => CoreRenderable::Blocks {
            blocks: p.blocks.clone().unwrap_or_default(),
        },
        Input::Peer(p) if p.blocks.is_some() => {
            let raw_blocks = p.blocks.clone().unwrap_or_default();
            if let Some(prefix) = peer_block_prefix_text(p) {
                let mut blocks = vec![meerkat_core::types::ContentBlock::Text {
                    text: prefix.clone(),
                }];
                if let Some(body_text) = rendered_message_body_text(&p.body, &prefix)
                    && !blocks_include_text_projection(&raw_blocks, &body_text)
                {
                    blocks.push(meerkat_core::types::ContentBlock::Text { text: body_text });
                }
                blocks.extend(raw_blocks);
                CoreRenderable::Blocks { blocks }
            } else {
                let body_already_in_blocks = raw_blocks.first().is_some_and(|b| {
                    matches!(b, meerkat_core::types::ContentBlock::Text { text } if text == &p.body)
                });
                if p.body.is_empty() || body_already_in_blocks {
                    CoreRenderable::Blocks { blocks: raw_blocks }
                } else {
                    let mut blocks = vec![meerkat_core::types::ContentBlock::Text {
                        text: p.body.clone(),
                    }];
                    blocks.extend(raw_blocks);
                    CoreRenderable::Blocks { blocks }
                }
            }
        }
        Input::FlowStep(f) if f.blocks.is_some() => CoreRenderable::Blocks {
            blocks: f.blocks.clone().unwrap_or_default(),
        },
        Input::ExternalEvent(e) if e.blocks.is_some() => CoreRenderable::Blocks {
            blocks: e.blocks.clone().unwrap_or_default(),
        },
        Input::Prompt(_) | Input::Peer(_) | Input::FlowStep(_) | Input::ExternalEvent(_) => {
            CoreRenderable::Text {
                text: input_prompt_text(input),
            }
        }
        Input::Continuation(_) | Input::Operation(_) => return None,
    };

    Some(ConversationAppend {
        role: ConversationAppendRole::User,
        content,
    })
}

fn input_to_context_append(input: &Input) -> Option<ConversationContextAppend> {
    let projection = match input {
        Input::Peer(peer) => peer_projection_from_peer_input(peer)?,
        _ => return None,
    };

    Some(ConversationContextAppend {
        key: projection.context_key()?,
        content: CoreRenderable::Text {
            text: projection.prompt_text(),
        },
    })
}

fn peer_response_terminal_context_append(
    peer: &PeerInput,
) -> Result<Option<ConversationContextAppend>, PeerResponseTerminalFactError> {
    let Some(fact) = peer_response_terminal_fact(peer)? else {
        return Ok(None);
    };

    Ok(Some(ConversationContextAppend {
        key: fact.context_key(),
        content: CoreRenderable::Text {
            text: fact.prompt_text(),
        },
    }))
}

pub(crate) fn runtime_input_projection(
    input: &Input,
) -> crate::ingress_types::RuntimeInputProjection {
    crate::ingress_types::RuntimeInputProjection {
        append: input_to_append(input),
        context_append: input_to_context_append(input),
    }
}

pub(crate) fn runtime_input_projection_for_machine_batch(
    input: &Input,
) -> crate::ingress_types::RuntimeInputProjection {
    let mut projection = runtime_input_projection(input);
    if let Input::Peer(peer) = input
        && let Ok(Some(context_append)) = peer_response_terminal_context_append(peer)
    {
        projection.context_append = Some(context_append);
    }
    projection
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
            payload: None,
            blocks: None,
            handling_mode: None,
        });
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["input_type"], "peer");
        let parsed: Input = serde_json::from_value(json).unwrap();
        assert!(matches!(parsed, Input::Peer(_)));
    }

    #[test]
    fn peer_message_blocks_prefix_uses_rendered_display_label_not_canonical_origin() {
        let mut header = make_header();
        header.source = InputOrigin::Peer {
            peer_id: "canonical-peer-id".into(),
            display_identity: Some("display-agent".into()),
            runtime_id: None,
        };
        let input = Input::Peer(PeerInput {
            header,
            convention: Some(PeerConvention::Message),
            body: "[COMMS MESSAGE from display-agent]\ncaption\n[image: image/png]".into(),
            payload: None,
            blocks: Some(vec![
                meerkat_core::types::ContentBlock::Text {
                    text: "caption".into(),
                },
                meerkat_core::types::ContentBlock::Image {
                    media_type: "image/png".into(),
                    data: "abc".into(),
                },
            ]),
            handling_mode: None,
        });

        let Input::Peer(peer) = &input else {
            panic!("expected peer input");
        };
        assert_eq!(
            peer_projection_from_peer_input(peer)
                .and_then(|projection| projection.block_prefix_text())
                .as_deref(),
            Some("[COMMS MESSAGE from canonical-peer-id]")
        );

        let projection = runtime_input_projection(&input);
        let append = projection.append.expect("conversation append");
        let CoreRenderable::Blocks { blocks } = append.content else {
            panic!("expected multimodal blocks");
        };
        assert_eq!(
            blocks.first(),
            Some(&meerkat_core::types::ContentBlock::Text {
                text: "[COMMS MESSAGE from display-agent]".into()
            })
        );
        assert!(!meerkat_core::types::text_content(&blocks).contains("canonical-peer-id"));
    }

    #[test]
    fn peer_response_terminal_context_is_deferred_to_machine_batch_projection() {
        let route_id = "018f6f79-7a82-7c4e-a552-a3b86f9630f2";
        let request_id = "018f6f79-7a82-7c4e-a552-a3b86f9630f1";
        let mut header = make_header();
        header.source = InputOrigin::Peer {
            peer_id: route_id.into(),
            display_identity: Some("display-agent".into()),
            runtime_id: None,
        };
        let input = Input::Peer(PeerInput {
            header,
            convention: Some(PeerConvention::ResponseTerminal {
                request_id: request_id.into(),
                status: ResponseTerminalStatus::Completed,
            }),
            body: "legacy response body".into(),
            payload: Some(serde_json::json!({"answer":"ok"})),
            blocks: None,
            handling_mode: None,
        });

        let Input::Peer(peer) = &input else {
            panic!("expected peer input");
        };
        let expected_canonical_key = format!("peer_response_terminal:{route_id}:{request_id}");
        assert!(
            peer_projection_from_peer_input(peer).is_none(),
            "terminal peer response projection must not be built before machine batch selection"
        );

        let projection = runtime_input_projection(&input);
        assert!(
            projection.context_append.is_none(),
            "admission projection must not store terminal peer response context"
        );
        let projection = runtime_input_projection_for_machine_batch(&input);
        let context = projection.context_append.expect("context append");
        assert_eq!(context.key, expected_canonical_key);
        let CoreRenderable::Text { text } = context.content else {
            panic!("expected text context");
        };
        assert!(text.contains("from display-agent"));
        assert!(!text.contains(route_id));
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
            payload: Some(serde_json::json!({"name": "agent-1"})),
            blocks: None,
            handling_mode: None,
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
            payload: Some(serde_json::json!({"ok": true})),
            blocks: None,
            handling_mode: None,
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
            payload: Some(serde_json::json!({"progress": "working"})),
            blocks: None,
            handling_mode: None,
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
    fn legacy_external_event_payload_blocks_migrate_to_canonical_blocks_owner() {
        let mut input = Input::ExternalEvent(ExternalEventInput {
            header: make_header(),
            event_type: "webhook.received".into(),
            payload: serde_json::json!({
                "body": "see image",
                "blocks": [
                    { "type": "text", "text": "caption text" },
                    { "type": "image", "media_type": "image/png", "source": "inline", "data": "abc123" }
                ]
            }),
            blocks: None,
            handling_mode: HandlingMode::Queue,
            render_metadata: None,
        });

        match &mut input {
            Input::ExternalEvent(event) => {
                migrate_legacy_payload_blocks(event).unwrap();
                assert!(event.payload.get("blocks").is_none());
                assert_eq!(event.payload["body"], "see image");
                assert_eq!(event.blocks.as_ref().map(Vec::len), Some(2));
            }
            other => panic!("Expected ExternalEvent, got {other:?}"),
        }
    }

    #[test]
    fn continuation_input_serde() {
        let input = Input::Continuation(ContinuationInput::detached_background_op_completed());
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["input_type"], "continuation");
        let parsed: Input = serde_json::from_value(json).unwrap();
        match parsed {
            Input::Continuation(continuation) => {
                assert_eq!(continuation.handling_mode, HandlingMode::Steer);
                assert_eq!(continuation.reason, "detached_background_op_completed");
            }
            other => panic!("Expected Continuation, got {other:?}"),
        }
    }

    #[test]
    fn continuation_input_accepts_legacy_system_generated_tag() {
        let input = Input::Continuation(ContinuationInput::detached_background_op_completed());
        let mut json = serde_json::to_value(&input).unwrap();
        json["input_type"] = serde_json::Value::String("system_generated".into());
        let parsed: Input = serde_json::from_value(json).unwrap();
        match parsed {
            Input::Continuation(continuation) => {
                assert_eq!(continuation.reason, "detached_background_op_completed");
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
        assert_eq!(prompt.kind(), InputKind::Prompt);

        let peer_msg = Input::Peer(PeerInput {
            header: make_header(),
            convention: Some(PeerConvention::Message),
            body: "hi".into(),
            payload: None,
            blocks: None,
            handling_mode: None,
        });
        assert_eq!(peer_msg.kind(), InputKind::PeerMessage);

        let peer_req = Input::Peer(PeerInput {
            header: make_header(),
            convention: Some(PeerConvention::Request {
                request_id: "r".into(),
                intent: "i".into(),
            }),
            body: "hi".into(),
            payload: Some(serde_json::json!({"subject": "x"})),
            blocks: None,
            handling_mode: None,
        });
        assert_eq!(peer_req.kind(), InputKind::PeerRequest);

        let continuation = Input::Continuation(ContinuationInput {
            header: make_header(),
            reason: "continue".into(),
            handling_mode: HandlingMode::Steer,
            request_id: None,
        });
        assert_eq!(continuation.kind(), InputKind::Continuation);

        let operation = Input::Operation(OperationInput {
            header: make_header(),
            operation_id: OperationId::new(),
            event: OpEvent::Cancelled {
                id: OperationId::new(),
            },
        });
        assert_eq!(operation.kind(), InputKind::Operation);
    }

    #[test]
    fn input_source_variants() {
        let sources = vec![
            InputOrigin::Operator,
            InputOrigin::Peer {
                peer_id: "p1".into(),
                display_identity: None,
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

    #[test]
    fn peer_input_without_handling_mode_deserializes_as_none() {
        // Simulate old serialized PeerInput without the handling_mode field.
        let json = serde_json::json!({
            "input_type": "peer",
            "header": serde_json::to_value(make_header()).unwrap(),
            "convention": { "convention_type": "message" },
            "body": "hello"
        });
        let parsed: Input = serde_json::from_value(json).unwrap();
        match parsed {
            Input::Peer(p) => assert!(p.handling_mode.is_none()),
            other => panic!("Expected Peer, got {other:?}"),
        }
    }

    #[test]
    fn peer_input_with_queue_handling_mode_roundtrips() {
        let input = Input::Peer(PeerInput {
            header: make_header(),
            convention: Some(PeerConvention::Message),
            body: "hi".into(),
            payload: None,
            blocks: None,
            handling_mode: Some(HandlingMode::Queue),
        });
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["handling_mode"], "queue");
        let parsed: Input = serde_json::from_value(json).unwrap();
        match parsed {
            Input::Peer(p) => assert_eq!(p.handling_mode, Some(HandlingMode::Queue)),
            other => panic!("Expected Peer, got {other:?}"),
        }
    }

    #[test]
    fn peer_response_terminal_input_owns_wire_status_mapping() {
        let peer_id = meerkat_core::comms::PeerId::from_uuid(
            uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000161").unwrap(),
        );
        let display_name = meerkat_core::comms::PeerName::new("analyst").unwrap();
        let request_id = meerkat_core::PeerCorrelationId::from_uuid(
            uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000162").unwrap(),
        );
        let input = peer_response_terminal_input(
            peer_id,
            Some(display_name),
            request_id,
            meerkat_contracts::PeerResponseTerminalStatusWire::Cancelled,
            serde_json::json!({"ok": false}),
        );

        match input {
            Input::Peer(PeerInput {
                header:
                    InputHeader {
                        source:
                            InputOrigin::Peer {
                                peer_id,
                                display_identity,
                                runtime_id,
                            },
                        durability: InputDurability::Durable,
                        correlation_id,
                        ..
                    },
                convention: Some(PeerConvention::ResponseTerminal { request_id, status }),
                payload: Some(payload),
                handling_mode: None,
                ..
            }) => {
                assert_eq!(peer_id, "00000000-0000-4000-8000-000000000161");
                assert_eq!(display_identity.as_deref(), Some("analyst"));
                assert_eq!(runtime_id, None);
                assert_eq!(request_id, "00000000-0000-4000-8000-000000000162");
                assert_eq!(
                    correlation_id,
                    Some(CorrelationId::from_uuid(
                        uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000162").unwrap()
                    ))
                );
                assert_eq!(status, ResponseTerminalStatus::Cancelled);
                assert_eq!(payload["ok"], false);
            }
            other => panic!("expected terminal peer input, got {other:?}"),
        }
    }

    #[test]
    fn peer_input_with_steer_handling_mode_roundtrips() {
        let input = Input::Peer(PeerInput {
            header: make_header(),
            convention: Some(PeerConvention::Message),
            body: "hi".into(),
            payload: None,
            blocks: None,
            handling_mode: Some(HandlingMode::Steer),
        });
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["handling_mode"], "steer");
        let parsed: Input = serde_json::from_value(json).unwrap();
        match parsed {
            Input::Peer(p) => assert_eq!(p.handling_mode, Some(HandlingMode::Steer)),
            other => panic!("Expected Peer, got {other:?}"),
        }
    }

    #[test]
    fn peer_input_handling_mode_not_serialized_when_none() {
        let input = Input::Peer(PeerInput {
            header: make_header(),
            convention: Some(PeerConvention::Message),
            body: "hi".into(),
            payload: None,
            blocks: None,
            handling_mode: None,
        });
        let json = serde_json::to_value(&input).unwrap();
        assert!(json.get("handling_mode").is_none());
    }
}
