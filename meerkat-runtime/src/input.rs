//! §8 Input types — the 6 input variants accepted by the runtime layer.
//!
//! Core never sees these. Generated admission authority resolves each accepted
//! Input to a PolicyDecision, then the runtime translates accepted Inputs into
//! RunPrimitive for core consumption.

use chrono::{DateTime, Utc};
use meerkat_core::lifecycle::InputId;
use meerkat_core::lifecycle::run_primitive::{
    ConversationAppend, ConversationAppendRole, ConversationContextAppend, CoreRenderable,
    RuntimeTurnMetadata,
};
use meerkat_core::ops::{OpEvent, OperationId};
use meerkat_core::service::TurnToolOverlay;
use meerkat_core::types::{
    ContentInput, HandlingMode, SystemNoticeBlock, SystemNoticeDirection, SystemNoticeKind,
    SystemNoticePeer,
};
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
    Continuation(ContinuationInput),
    /// Explicit non-content operation/lifecycle input.
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

    /// Typed continuation discriminant threaded into admission. Only
    /// continuations carry a non-ordinary kind; every other input family is
    /// [`ContinuationKind::Ordinary`]. This is a typed pass-through of the
    /// producer-declared fact — admission never re-classifies continuation
    /// routing from reason strings or overlay dispatch-context keys.
    pub fn continuation_kind(&self) -> ContinuationKind {
        match self {
            Input::Continuation(continuation) => continuation.continuation_kind,
            _ => ContinuationKind::Ordinary,
        }
    }
}

/// Fail closed on the retired external-event shape that smuggled multimodal
/// blocks inside the payload JSON object. The typed [`ExternalEventInput::blocks`]
/// field is the only content owner; a payload-level `blocks` key is rejected
/// with a typed error instead of being migrated or silently passed through.
fn reject_legacy_payload_blocks(event: &ExternalEventInput) -> Result<(), BlobStoreError> {
    if event
        .payload
        .as_object()
        .is_some_and(|obj| obj.contains_key("blocks"))
    {
        return Err(BlobStoreError::Internal(format!(
            "external-event payload for event_type `{}` carries the retired payload-level \
             `blocks` key; multimodal content must use the typed `ExternalEventInput.blocks` owner",
            event.event_type
        )));
    }
    Ok(())
}

pub async fn externalize_input_images(
    blob_store: &dyn BlobStore,
    input: &mut Input,
) -> Result<(), BlobStoreError> {
    match input {
        Input::Prompt(prompt) => {
            if let ContentInput::Blocks(blocks) = &mut prompt.content {
                externalize_content_blocks(blob_store, blocks).await?;
            }
        }
        Input::Peer(peer) => {
            if let ContentInput::Blocks(blocks) = &mut peer.content {
                externalize_content_blocks(blob_store, blocks).await?;
            }
        }
        Input::FlowStep(flow_step) => {
            if let ContentInput::Blocks(blocks) = &mut flow_step.content {
                externalize_content_blocks(blob_store, blocks).await?;
            }
        }
        Input::ExternalEvent(event) => {
            reject_legacy_payload_blocks(event)?;
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
            if let ContentInput::Blocks(blocks) = &mut prompt.content {
                hydrate_content_blocks(blob_store, blocks, missing_behavior).await?;
            }
        }
        Input::Peer(peer) => {
            if let ContentInput::Blocks(blocks) = &mut peer.content {
                hydrate_content_blocks(blob_store, blocks, missing_behavior).await?;
            }
        }
        Input::FlowStep(flow_step) => {
            if let ContentInput::Blocks(blocks) = &mut flow_step.content {
                hydrate_content_blocks(blob_store, blocks, missing_behavior).await?;
            }
        }
        Input::ExternalEvent(event) => {
            reject_legacy_payload_blocks(event)?;
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
    /// The prompt content — the single typed owner of this input's content
    /// fact (plain text or multimodal blocks). The text projection is derived
    /// at read time via [`ContentInput::text_content`]; it is never stored
    /// separately.
    pub content: ContentInput,
    /// Runtime-authored typed transcript appends that travel with this turn.
    ///
    /// These are not operator-authored prompt content. The runtime projects
    /// them into model-facing text only when building the provider request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub typed_turn_appends: Vec<ConversationAppend>,
    /// Host-attached injected context delivered alongside (not inside) this
    /// turn's prompt. Each entry lowers into a separate
    /// [`ConversationAppendRole::InjectedContext`] transcript append placed
    /// immediately BEFORE the turn's user append, in order — the typed slot
    /// the content arrived in mints the transcript role. Additive on durable
    /// input persistence (absent on older persisted inputs).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub injected_context: Vec<ContentInput>,
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
            content: ContentInput::Text(text.into()),
            typed_turn_appends: Vec::new(),
            injected_context: Vec::new(),
            turn_metadata,
        }
    }

    /// Create a prompt from `ContentInput` (text or multimodal blocks).
    pub fn from_content_input(
        input: ContentInput,
        turn_metadata: Option<RuntimeTurnMetadata>,
    ) -> Self {
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
            content: input,
            typed_turn_appends: Vec::new(),
            injected_context: Vec::new(),
            turn_metadata,
        }
    }

    /// Attach host-attached injected context to this prompt input.
    pub fn with_injected_context(mut self, injected_context: Vec<ContentInput>) -> Self {
        self.injected_context = injected_context;
        self
    }
}

/// Peer-originated input from comms.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInput {
    pub header: InputHeader,
    /// The peer convention (message, request, response).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub convention: Option<PeerConvention>,
    /// The peer content — the single typed owner of this input's content fact
    /// (plain text or multimodal blocks). Message-style peer traffic uses this
    /// directly. Request/response prompt projection is runtime-owned and must
    /// be reconstructed from `convention + payload + source` rather than
    /// helper-rendered prose. The text projection is derived at read time via
    /// [`ContentInput::text_content`]; it is never stored separately.
    pub content: ContentInput,
    /// Structured peer payload, when one exists.
    ///
    /// For `Request`, this is the request params. For `Response*`, this is the
    /// response result payload. Message traffic leaves this unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    /// Optional handling-mode override for actionable peer inputs.
    /// When present on Message/Request/no-convention, overrides kind-based
    /// policy defaults. Forbidden on ResponseProgress; ResponseTerminal may
    /// carry a typed override for requester reaction urgency (enforced by
    /// [`validate_peer_handling_mode`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handling_mode: Option<HandlingMode>,
    /// Sender-declared content taint carried inside the signed comms
    /// envelope, when the sender made a declaration. `None` means "no
    /// declaration" — a real third state that must never be coalesced into
    /// [`meerkat_core::comms::SenderContentTaint::Clean`]. Content-adjacent
    /// payload only: it makes no admission or routing decision. Additive on
    /// durable input persistence (absent on older persisted inputs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_taint: Option<meerkat_core::comms::SenderContentTaint>,
    /// Host-attached injected context carried by supervisor-authored work
    /// deliveries (remote mob members over the supervisor bridge). Each entry
    /// lowers into a separate
    /// [`ConversationAppendRole::InjectedContext`] transcript append placed
    /// immediately BEFORE this input's peer append, in order. Forbidden on
    /// steer-mode deliveries (the steer realization path carries no
    /// transcript appends — enforced by
    /// [`crate::peer_handling_mode::validate_peer_handling_mode`]). Additive
    /// on durable input persistence (absent on older persisted inputs).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub injected_context: Vec<ContentInput>,
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
        injected_context: Vec::new(),
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
        content: ContentInput::Text(String::new()),
        payload: Some(result),
        handling_mode: None,
        // Bridge-projected terminal responses carry no comms envelope, so no
        // sender declaration exists.
        sender_taint: None,
    })
}

/// Flow step input from mob orchestration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowStepInput {
    pub header: InputHeader,
    /// Flow step identifier.
    pub step_id: String,
    /// Step instructions — the single typed owner of this input's content
    /// fact (plain text or multimodal blocks). The text projection is derived
    /// at read time via [`ContentInput::text_content`]; it is never stored
    /// separately.
    pub content: ContentInput,
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

/// Typed continuation discriminant carried on a [`ContinuationInput`].
///
/// The producer of a continuation declares how the runtime must re-enter the
/// session: an ordinary continuation resumes the pending run, while a WorkGraph
/// attention continuation re-enters as a fresh queued content turn. This is a
/// typed fact owned by the producer; admission threads it into
/// `MeerkatMachine::ResolveAdmissionPlan`, which owns the lane and run-apply
/// semantics derived from it. No downstream consumer re-classifies continuation
/// routing from continuation reason strings or overlay dispatch-context keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationKind {
    /// Ordinary continuation: resume the pending run at the run boundary.
    #[default]
    Ordinary,
    /// WorkGraph attention continuation: re-enter as a fresh queued content turn.
    WorkgraphAttention,
}

/// Explicit continuation request that asks the runtime to keep draining
/// ordinary work after a boundary-local event (for example, terminal peer
/// responses injected into session state).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContinuationInput {
    pub header: InputHeader,
    /// Stable reason for the continuation request.
    pub reason: String,
    /// Typed continuation discriminant owned by the producer. Admission threads
    /// it into `MeerkatMachine::ResolveAdmissionPlan` so the machine owns the
    /// lane and run-apply semantics for WorkGraph attention re-entry.
    #[serde(default)]
    pub continuation_kind: ContinuationKind,
    /// Ordinary-work handling mode for the continuation.
    #[serde(default)]
    pub handling_mode: HandlingMode,
    /// Optional request/correlation handle tied to the continuation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Optional per-turn tool visibility overlay for scoped continuations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_tool_overlay: Option<TurnToolOverlay>,
    /// Optional runtime-owned context projected into the next turn boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_append: Option<ConversationContextAppend>,
    /// Optional runtime-owned turn append used to force a continuation turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_append: Option<ConversationAppend>,
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
            continuation_kind: ContinuationKind::Ordinary,
            handling_mode: HandlingMode::Steer,
            request_id: None,
            flow_tool_overlay: None,
            context_append: None,
            turn_append: None,
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
                peer.content.text_content()
            } else {
                prompt
            }
        })
        .unwrap_or_else(|| peer.content.text_content())
}

pub(crate) fn input_prompt_text(input: &Input) -> String {
    match input {
        Input::Prompt(p) => p.content.text_content(),
        Input::Peer(p) => peer_prompt_text(p),
        Input::FlowStep(f) => f.content.text_content(),
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

fn peer_notice_renderable(peer: &PeerInput) -> Option<CoreRenderable> {
    let (peer_id, display_name) = match &peer.header.source {
        InputOrigin::Peer {
            peer_id,
            display_identity,
            ..
        } => (peer_id.clone(), display_identity.clone()),
        _ => return None,
    };
    use meerkat_core::types::CommsNoticeKind;
    let (kind, request_id, intent, status) = match &peer.convention {
        Some(PeerConvention::Message) | None => (CommsNoticeKind::Message, None, None, None),
        Some(PeerConvention::Request { request_id, intent }) => (
            CommsNoticeKind::Request,
            Some(request_id.clone()),
            Some(intent.clone()),
            None,
        ),
        Some(PeerConvention::ResponseProgress { request_id, phase }) => (
            CommsNoticeKind::ResponseProgress,
            Some(request_id.clone()),
            None,
            Some(format!("{phase:?}")),
        ),
        Some(PeerConvention::ResponseTerminal { request_id, status }) => (
            CommsNoticeKind::ResponseTerminal,
            Some(request_id.clone()),
            None,
            Some(format!("{status:?}")),
        ),
    };
    let summary = match kind {
        CommsNoticeKind::Request => intent.as_ref().map_or_else(
            || "Peer request".to_string(),
            |intent| format!("Peer request: {intent}"),
        ),
        CommsNoticeKind::ResponseProgress => "Peer response progress".to_string(),
        CommsNoticeKind::ResponseTerminal => "Peer response terminal".to_string(),
        CommsNoticeKind::Message | CommsNoticeKind::Other(_) => "Peer message".to_string(),
    };
    let content = match &peer.content {
        ContentInput::Text(body) if body.is_empty() => Vec::new(),
        ContentInput::Text(body) => {
            vec![meerkat_core::types::ContentBlock::Text { text: body.clone() }]
        }
        ContentInput::Blocks(blocks) => blocks.clone(),
    };
    // The peer routing identity is the canonical typed `PeerId`. Production
    // peer inputs always carry a hyphenated UUID here (the comms bridge stamps
    // `canonical_peer_id_string()`); parse once at this producer boundary. A
    // value that is not a valid `PeerId` cannot populate the typed identity, so
    // the notice renders without a peer identity (degraded projection) rather
    // than smuggling an unparseable string through the transcript.
    let notice_peer = meerkat_core::comms::PeerId::parse(&peer_id)
        .ok()
        .map(|id| SystemNoticePeer { id, display_name });
    Some(CoreRenderable::SystemNotice {
        kind: SystemNoticeKind::Comms,
        body: Some(summary.clone()),
        blocks: vec![SystemNoticeBlock::Comms {
            kind,
            direction: SystemNoticeDirection::Incoming,
            peer: notice_peer,
            // The envelope's sender-declared taint rides into the typed
            // transcript notice; `None` (no declaration) stays distinct from
            // an affirmative `Clean` declaration.
            sender_taint: peer.sender_taint,
            request_id,
            intent,
            status,
            summary: Some(summary),
            payload: peer.payload.clone(),
            content,
        }],
    })
}

fn external_event_notice_renderable(event: &ExternalEventInput) -> CoreRenderable {
    let source = match &event.header.source {
        InputOrigin::External { source_name } if !source_name.trim().is_empty() => {
            source_name.clone()
        }
        _ => event.event_type.clone(),
    };
    let body = event
        .payload
        .get("body")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|body| !body.is_empty())
        .map(ToOwned::to_owned);
    let summary = body.as_ref().map_or_else(
        || format!("External event via {source}"),
        std::clone::Clone::clone,
    );
    CoreRenderable::SystemNotice {
        kind: SystemNoticeKind::ExternalEvent,
        body: Some(summary.clone()),
        blocks: vec![SystemNoticeBlock::ExternalEvent {
            source,
            event_type: event.event_type.clone(),
            summary: Some(summary),
            body,
            payload: Some(event.payload.clone()),
            content: event.blocks.clone().unwrap_or_default(),
        }],
    }
}

fn input_to_append(input: &Input) -> Option<ConversationAppend> {
    // Terminal peer responses always carry their typed comms notice as the
    // turn's conversation append — with or without multimodal blocks. The
    // former `blocks: None => no append` special case left the mandatory
    // `AppendContextAndRun` reaction turn with a fabricated empty prompt,
    // which providers reject (Anthropic: "user messages must have non-empty
    // content") and which violates the typed-run-input doctrine that no
    // empty-string prompt is ever synthesized.
    let (role, content) = match input {
        Input::Prompt(p)
            if !p.typed_turn_appends.is_empty()
                && match &p.content {
                    ContentInput::Text(text) => text.trim().is_empty(),
                    ContentInput::Blocks(blocks) => blocks.is_empty(),
                } =>
        {
            return None;
        }
        Input::Prompt(p) => match &p.content {
            ContentInput::Blocks(blocks) => (
                ConversationAppendRole::User,
                CoreRenderable::Blocks {
                    blocks: blocks.clone(),
                },
            ),
            ContentInput::Text(_) => (
                ConversationAppendRole::User,
                CoreRenderable::Text {
                    text: input_prompt_text(input),
                },
            ),
        },
        Input::Peer(p) => peer_notice_renderable(p)
            .map(|content| (ConversationAppendRole::SystemNotice, content))?,
        Input::FlowStep(f) => (
            ConversationAppendRole::SystemNotice,
            CoreRenderable::SystemNotice {
                kind: SystemNoticeKind::Generic,
                body: Some(format!("Flow step {}", f.step_id)),
                blocks: vec![SystemNoticeBlock::RuntimeNotice {
                    category: "flow_step".to_string(),
                    detail: Some(f.content.text_content()),
                    payload: None,
                }],
            },
        ),
        Input::ExternalEvent(e) => (
            ConversationAppendRole::SystemNotice,
            external_event_notice_renderable(e),
        ),
        Input::Continuation(continuation) => return continuation.turn_append.clone(),
        Input::Operation(_) => return None,
    };

    Some(ConversationAppend { role, content })
}

fn input_to_context_append(input: &Input) -> Option<ConversationContextAppend> {
    let (projection, content) = match input {
        Input::Continuation(continuation) => {
            return continuation.context_append.clone();
        }
        Input::Peer(peer) => {
            let projection = peer_projection_from_peer_input(peer)?;
            let content = peer_notice_renderable(peer)?;
            (projection, content)
        }
        _ => return None,
    };

    Some(ConversationContextAppend {
        key: projection.context_key()?,
        content,
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
        content: CoreRenderable::SystemNotice {
            kind: SystemNoticeKind::Comms,
            body: Some("Peer terminal response context".to_string()),
            blocks: vec![SystemNoticeBlock::Comms {
                kind: meerkat_core::types::CommsNoticeKind::ResponseTerminal,
                direction: SystemNoticeDirection::Incoming,
                peer: Some(SystemNoticePeer {
                    id: fact.source.route_identity.peer_id(),
                    display_name: Some(fact.source.display_identity.to_string()),
                }),
                // This context append is derived from the machine-echoed
                // terminal apply intent, which carries no content facts - no
                // sender declaration is available here.
                sender_taint: None,
                request_id: Some(fact.correlation_id.to_string()),
                intent: None,
                status: Some(fact.status.label().to_string()),
                summary: Some("Peer terminal response".to_string()),
                payload: fact.render_payload.as_ref().cloned(),
                content: Vec::new(),
            }],
        },
    }))
}

/// Lower host-attached injected context entries into typed
/// `InjectedContext`-role transcript appends, preserving delivery order.
/// The typed slot the content arrived in mints the transcript role.
fn injected_context_appends(entries: &[ContentInput]) -> Vec<ConversationAppend> {
    entries
        .iter()
        .map(|entry| ConversationAppend {
            role: ConversationAppendRole::InjectedContext,
            content: match entry {
                ContentInput::Blocks(blocks) => CoreRenderable::Blocks {
                    blocks: blocks.clone(),
                },
                ContentInput::Text(text) => CoreRenderable::Text { text: text.clone() },
            },
        })
        .collect()
}

pub(crate) fn runtime_input_projection(
    input: &Input,
) -> crate::ingress_types::RuntimeInputProjection {
    crate::ingress_types::RuntimeInputProjection {
        injected_context_appends: match input {
            Input::Prompt(prompt) => injected_context_appends(&prompt.injected_context),
            Input::Peer(peer) => injected_context_appends(&peer.injected_context),
            _ => Vec::new(),
        },
        append: input_to_append(input),
        additional_appends: match input {
            Input::Prompt(prompt) => prompt.typed_turn_appends.clone(),
            _ => Vec::new(),
        },
        context_append: input_to_context_append(input),
        peer_response_terminal: None,
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
        // Carry the typed terminal-peer-response fact alongside the rendered
        // context append so the realtime/live consumer reads the typed fact
        // directly instead of re-parsing the flattened prose.
        if let Ok(fact) = peer_response_terminal_fact(peer) {
            projection.peer_response_terminal = fact;
        }
    }
    projection
}

pub(crate) fn context_append_to_pending_system_context_append(
    append: &ConversationContextAppend,
    peer_response_terminal: Option<&meerkat_core::PeerResponseTerminalFact>,
) -> meerkat_core::PendingSystemContextAppend {
    meerkat_core::PendingSystemContextAppend {
        content: append.content.clone(),
        source: Some(append.key.clone()),
        idempotency_key: Some(append.key.clone()),
        // Durable keyed context append (peer responses, etc.) — not a steer.
        source_kind: meerkat_core::session::SystemContextSource::Normal,
        // Carry the typed `PeerResponseTerminalFact` so the realtime consumer
        // reads it directly (mirrors the `source_kind` precedent that retired
        // the `runtime:steer:` string prefix). The fact threads from the
        // admitted `RuntimeInputProjection.peer_response_terminal` field.
        peer_response_terminal: peer_response_terminal.cloned(),
        accepted_at: meerkat_core::time_compat::SystemTime::now(),
    }
}

pub(crate) fn projection_to_pending_system_context_appends(
    input_id: &InputId,
    projection: &crate::ingress_types::RuntimeInputProjection,
) -> Vec<meerkat_core::PendingSystemContextAppend> {
    if let Some(append) = projection.context_append.as_ref() {
        return std::iter::once(context_append_to_pending_system_context_append(
            append,
            projection.peer_response_terminal.as_ref(),
        ))
        .filter(|append| !append.content.render_text().trim().is_empty())
        .collect();
    }

    projection
        .append
        .as_ref()
        .map(|append| {
            // The PRODUCER of a runtime-steer append sets the typed marker
            // here, at construction. This is the single source of truth for
            // the runtime-steer fact — no downstream code reclassifies the
            // `source` string. The `runtime:steer:` source/idempotency key is
            // retained only as a stable per-input idempotency identifier.
            let key = format!("runtime:steer:{input_id}");
            meerkat_core::PendingSystemContextAppend {
                content: append.content.clone(),
                source: Some(key.clone()),
                idempotency_key: Some(key),
                source_kind: meerkat_core::session::SystemContextSource::RuntimeSteer,
                // A runtime steer is never a terminal-peer-response projection.
                peer_response_terminal: None,
                accepted_at: meerkat_core::time_compat::SystemTime::now(),
            }
        })
        .into_iter()
        .filter(|append| !append.content.render_text().trim().is_empty())
        .collect()
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

    fn typed_runtime_notice_append(detail: &str) -> ConversationAppend {
        ConversationAppend {
            role: ConversationAppendRole::SystemNotice,
            content: CoreRenderable::SystemNotice {
                kind: meerkat_core::types::SystemNoticeKind::Generic,
                body: Some(detail.to_string()),
                blocks: vec![meerkat_core::types::SystemNoticeBlock::RuntimeNotice {
                    category: "test".to_string(),
                    detail: Some(detail.to_string()),
                    payload: None,
                }],
            },
        }
    }

    #[test]
    fn prompt_input_serde() {
        let input = Input::Prompt(PromptInput {
            injected_context: Vec::new(),
            header: make_header(),
            content: "hello".into(),
            typed_turn_appends: Vec::new(),
            turn_metadata: None,
        });
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["input_type"], "prompt");
        let parsed: Input = serde_json::from_value(json).unwrap();
        assert!(matches!(parsed, Input::Prompt(_)));
    }

    #[test]
    fn prompt_input_typed_turn_appends_project_without_user_text() {
        let append = typed_runtime_notice_append("peer delivery");
        let input = Input::Prompt(PromptInput {
            injected_context: Vec::new(),
            header: make_header(),
            content: ContentInput::Text(String::new()),
            typed_turn_appends: vec![append.clone()],
            turn_metadata: None,
        });

        let projection = runtime_input_projection(&input);
        assert!(
            projection.append.is_none(),
            "empty runtime-authored prompt carrier must not synthesize a user append"
        );
        assert_eq!(projection.additional_appends, vec![append]);
    }

    /// Injected context on a prompt input projects into a distinct
    /// `InjectedContext`-role append slot, preserving delivery order — never
    /// the generic `additional_appends` carrier (which chains AFTER the user
    /// append).
    #[test]
    fn prompt_input_injected_context_projects_before_user_append() {
        let input = Input::Prompt(PromptInput {
            injected_context: vec![
                ContentInput::Text("ambient alpha".to_string()),
                ContentInput::Text("ambient beta".to_string()),
            ],
            header: make_header(),
            content: "the prompt".into(),
            typed_turn_appends: Vec::new(),
            turn_metadata: None,
        });

        let projection = runtime_input_projection(&input);
        assert_eq!(projection.injected_context_appends.len(), 2);
        assert!(
            projection
                .injected_context_appends
                .iter()
                .all(|append| { append.role == ConversationAppendRole::InjectedContext })
        );
        assert_eq!(
            projection.injected_context_appends[0].content,
            CoreRenderable::Text {
                text: "ambient alpha".to_string()
            }
        );
        assert_eq!(
            projection.injected_context_appends[1].content,
            CoreRenderable::Text {
                text: "ambient beta".to_string()
            }
        );
        assert!(
            projection.additional_appends.is_empty(),
            "injected context must not ride the generic typed_turn_appends carrier"
        );
        assert!(projection.append.is_some(), "user append must survive");
    }

    /// Injected context riding a supervisor bridge delivery projects before
    /// the peer's own append (which lowers as a SystemNotice).
    #[test]
    fn peer_input_injected_context_projects_before_peer_append() {
        let mut header = make_header();
        header.source = InputOrigin::Peer {
            peer_id: "peer-1".into(),
            display_identity: Some("Peer One".into()),
            runtime_id: None,
        };
        let input = Input::Peer(PeerInput {
            injected_context: vec![ContentInput::Text("supervisor ambient".to_string())],
            sender_taint: None,
            header,
            convention: Some(PeerConvention::Message),
            content: "work content".into(),
            payload: None,
            handling_mode: None,
        });

        let projection = runtime_input_projection(&input);
        assert_eq!(projection.injected_context_appends.len(), 1);
        assert_eq!(
            projection.injected_context_appends[0].role,
            ConversationAppendRole::InjectedContext
        );
        assert!(
            projection.append.is_some(),
            "peer work append must survive alongside injected context"
        );
    }

    /// Absent `injected_context` deserializes to empty (pre-field persisted
    /// inputs stay readable) and empty is omitted on serialization.
    #[test]
    fn prompt_input_injected_context_serde_default_and_omission() {
        let input = Input::Prompt(PromptInput {
            injected_context: vec![ContentInput::Text("ambient".to_string())],
            header: make_header(),
            content: "hello".into(),
            typed_turn_appends: Vec::new(),
            turn_metadata: None,
        });
        let json = serde_json::to_value(&input).unwrap();
        assert!(json.get("injected_context").is_some());
        let parsed: Input = serde_json::from_value(json).unwrap();
        let Input::Prompt(prompt) = parsed else {
            panic!("expected prompt input");
        };
        assert_eq!(prompt.injected_context.len(), 1);

        let empty = Input::Prompt(PromptInput {
            injected_context: Vec::new(),
            header: make_header(),
            content: "hello".into(),
            typed_turn_appends: Vec::new(),
            turn_metadata: None,
        });
        let mut json = serde_json::to_value(&empty).unwrap();
        assert!(
            json.get("injected_context").is_none(),
            "empty injected context must be omitted on the wire"
        );
        // Pre-field persisted input (no key at all) deserializes to empty.
        json.as_object_mut().unwrap().remove("injected_context");
        let parsed: Input = serde_json::from_value(json).unwrap();
        let Input::Prompt(prompt) = parsed else {
            panic!("expected prompt input");
        };
        assert!(prompt.injected_context.is_empty());
    }

    #[test]
    fn prompt_input_typed_turn_appends_serde_roundtrip() {
        let append = typed_runtime_notice_append("typed appends persist");
        let input = Input::Prompt(PromptInput {
            injected_context: Vec::new(),
            header: make_header(),
            content: ContentInput::Text(String::new()),
            typed_turn_appends: vec![append.clone()],
            turn_metadata: None,
        });

        let json = serde_json::to_value(&input).unwrap();
        let parsed: Input = serde_json::from_value(json).unwrap();
        let Input::Prompt(prompt) = parsed else {
            panic!("expected prompt input");
        };
        assert_eq!(prompt.content.text_content(), "");
        assert_eq!(prompt.typed_turn_appends, vec![append]);
    }

    #[test]
    fn peer_input_message_serde() {
        let input = Input::Peer(PeerInput {
            injected_context: Vec::new(),
            sender_taint: None,
            header: make_header(),
            convention: Some(PeerConvention::Message),
            content: "hi there".into(),
            payload: None,
            handling_mode: None,
        });
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["input_type"], "peer");
        let parsed: Input = serde_json::from_value(json).unwrap();
        assert!(matches!(parsed, Input::Peer(_)));
    }

    #[test]
    fn peer_message_blocks_preserve_typed_comms_content_without_prefix_injection() {
        let peer_id = "018f6f79-7a82-7c4e-a552-a3b86f963005";
        let mut header = make_header();
        header.source = InputOrigin::Peer {
            peer_id: peer_id.into(),
            display_identity: Some("display-agent".into()),
            runtime_id: None,
        };
        let input = Input::Peer(PeerInput {
            injected_context: Vec::new(),
            sender_taint: None,
            header,
            convention: Some(PeerConvention::Message),
            content: ContentInput::Blocks(vec![
                meerkat_core::types::ContentBlock::Text {
                    text: "caption".into(),
                },
                meerkat_core::types::ContentBlock::Image {
                    media_type: "image/png".into(),
                    data: "abc".into(),
                },
            ]),
            payload: None,
            handling_mode: None,
        });

        let Input::Peer(peer) = &input else {
            panic!("expected peer input");
        };
        assert_eq!(
            peer_projection_from_peer_input(peer)
                .and_then(|projection| projection.block_prefix_text())
                .as_deref(),
            Some(format!("Peer message from {peer_id}").as_str())
        );

        let projection = runtime_input_projection(&input);
        let append = projection.append.expect("conversation append");
        let CoreRenderable::SystemNotice { blocks, .. } = append.content else {
            panic!("expected typed system notice");
        };
        let Some(meerkat_core::types::SystemNoticeBlock::Comms { content, peer, .. }) =
            blocks.first()
        else {
            panic!("expected comms block");
        };
        assert_eq!(
            peer.as_ref().and_then(|peer| peer.display_name.as_deref()),
            Some("display-agent")
        );
        assert_eq!(
            content.first(),
            Some(&meerkat_core::types::ContentBlock::Text {
                text: "caption".into()
            })
        );
    }

    /// Ask 5 gate (receiver end-to-end at the transcript notice): a peer
    /// input carrying the envelope's sender-declared taint produces a typed
    /// `SystemNoticeBlock::Comms` whose `sender_taint` preserves the
    /// declaration, and the model projection appends a marker for `Tainted`
    /// ONLY — `Clean` and `None` (no declaration) deliberately render
    /// identically while the typed field stays distinct.
    #[test]
    fn peer_message_sender_taint_reaches_typed_comms_notice_and_model_projection() {
        use meerkat_core::comms::SenderContentTaint;

        let notice_block = |declared: Option<SenderContentTaint>| {
            let mut header = make_header();
            header.source = InputOrigin::Peer {
                peer_id: "018f6f79-7a82-7c4e-a552-a3b86f963005".into(),
                display_identity: Some("display-agent".into()),
                runtime_id: None,
            };
            let input = Input::Peer(PeerInput {
                injected_context: Vec::new(),
                sender_taint: declared,
                header,
                convention: Some(PeerConvention::Message),
                content: "hello from peer".into(),
                payload: None,
                handling_mode: None,
            });
            let projection = runtime_input_projection(&input);
            let append = projection.append.expect("conversation append");
            let CoreRenderable::SystemNotice { blocks, .. } = append.content else {
                panic!("expected typed system notice");
            };
            blocks.first().cloned().expect("comms block")
        };

        let tainted_block = notice_block(Some(SenderContentTaint::Tainted));
        let clean_block = notice_block(Some(SenderContentTaint::Clean));
        let undeclared_block = notice_block(None);

        let taint_of = |block: &meerkat_core::types::SystemNoticeBlock| {
            let meerkat_core::types::SystemNoticeBlock::Comms { sender_taint, .. } = block else {
                panic!("expected comms block");
            };
            *sender_taint
        };
        assert_eq!(taint_of(&tainted_block), Some(SenderContentTaint::Tainted));
        assert_eq!(taint_of(&clean_block), Some(SenderContentTaint::Clean));
        assert_eq!(
            taint_of(&undeclared_block),
            None,
            "no declaration must stay None in the transcript, never coalesced into Clean"
        );

        let tainted_text = tainted_block.model_projection_text();
        let clean_text = clean_block.model_projection_text();
        let undeclared_text = undeclared_block.model_projection_text();
        assert!(
            tainted_text.contains("[sender declared this content tainted]"),
            "declared taint must be model-visible: {tainted_text}"
        );
        assert_eq!(
            clean_text, undeclared_text,
            "Clean and no-declaration deliberately render identically; the typed field is the carrier"
        );
        assert!(!clean_text.contains("tainted"));
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
            injected_context: Vec::new(),
            sender_taint: None,
            header,
            convention: Some(PeerConvention::ResponseTerminal {
                request_id: request_id.into(),
                status: ResponseTerminalStatus::Completed,
            }),
            content: "response body".into(),
            payload: Some(serde_json::json!({"answer":"ok"})),
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
        let CoreRenderable::SystemNotice { blocks, .. } = context.content else {
            panic!("expected typed context");
        };
        let Some(meerkat_core::types::SystemNoticeBlock::Comms { peer, .. }) = blocks.first()
        else {
            panic!("expected comms block");
        };
        assert_eq!(
            peer.as_ref().and_then(|peer| peer.display_name.as_deref()),
            Some("display-agent")
        );
        assert_eq!(
            peer.as_ref().map(|peer| peer.id),
            Some(meerkat_core::comms::PeerId::parse(route_id).expect("valid route id"))
        );
    }

    #[test]
    fn steer_projection_uses_context_append_as_pending_system_context() {
        let input_id = InputId::new();
        let projection = crate::ingress_types::RuntimeInputProjection {
            injected_context_appends: Vec::new(),
            append: Some(ConversationAppend {
                role: ConversationAppendRole::SystemNotice,
                content: CoreRenderable::Text {
                    text: "ordinary append must lose to context append".into(),
                },
            }),
            additional_appends: Vec::new(),
            context_append: Some(ConversationContextAppend {
                key: "peer_response_terminal:peer:req".into(),
                content: CoreRenderable::Text {
                    text: "terminal response is ready".into(),
                },
            }),
            peer_response_terminal: None,
        };

        let appends = projection_to_pending_system_context_appends(&input_id, &projection);

        assert_eq!(appends.len(), 1);
        assert_eq!(
            appends[0].content.render_text(),
            "terminal response is ready"
        );
        assert_eq!(
            appends[0].source.as_deref(),
            Some("peer_response_terminal:peer:req")
        );
        assert_eq!(
            appends[0].idempotency_key.as_deref(),
            Some("peer_response_terminal:peer:req")
        );
    }

    #[test]
    fn continuation_projection_can_carry_runtime_context_append() {
        let input = Input::Continuation(ContinuationInput {
            header: make_header(),
            reason: "workgraph_attention".into(),
            continuation_kind: ContinuationKind::WorkgraphAttention,
            handling_mode: HandlingMode::Steer,
            request_id: Some("binding-1".into()),
            flow_tool_overlay: Some(TurnToolOverlay {
                allowed_tools: Some(vec!["workgraph_add_evidence".into()]),
                blocked_tools: None,
                dispatch_context: Default::default(),
            }),
            context_append: Some(ConversationContextAppend {
                key: "workgraph_attention:binding-1:2:5".into(),
                content: CoreRenderable::Text {
                    text: "WorkGraph attention projection".into(),
                },
            }),
            turn_append: None,
        });
        let projection = runtime_input_projection_for_machine_batch(&input);
        let appends = projection_to_pending_system_context_appends(input.id(), &projection);

        assert_eq!(appends.len(), 1);
        assert_eq!(
            appends[0].content.render_text(),
            "WorkGraph attention projection"
        );
        assert_eq!(
            appends[0].source.as_deref(),
            Some("workgraph_attention:binding-1:2:5")
        );
        let metadata = crate::runtime_loop::for_input(
            &input,
            crate::ingress_types::RuntimeInputSemantics {
                boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
                execution_handling_mode: None,
                peer_response_terminal_apply_intent: None,
                live_interrupt_required: false,
            },
        );
        assert_eq!(
            metadata
                .flow_tool_overlay
                .and_then(|overlay| overlay.allowed_tools),
            Some(vec!["workgraph_add_evidence".into()])
        );
    }

    #[test]
    fn steer_projection_falls_back_to_ordinary_peer_append() {
        let mut header = make_header();
        header.source = InputOrigin::Peer {
            peer_id: "peer-a".into(),
            display_identity: Some("Peer A".into()),
            runtime_id: None,
        };
        let input = Input::Peer(PeerInput {
            injected_context: Vec::new(),
            sender_taint: None,
            header,
            convention: Some(PeerConvention::Message),
            content: "please look at this while you work".into(),
            payload: None,
            handling_mode: Some(HandlingMode::Steer),
        });
        let input_id = input.id().clone();
        let projection = runtime_input_projection(&input);

        let appends = projection_to_pending_system_context_appends(&input_id, &projection);

        assert_eq!(appends.len(), 1);
        let rendered = appends[0].content.render_text();
        assert!(
            rendered.contains("please look at this while you work"),
            "peer message append should be renderable as live system context: {rendered:?}"
        );
        assert_eq!(
            appends[0].source.as_deref(),
            Some(format!("runtime:steer:{input_id}").as_str())
        );
        assert_eq!(
            appends[0].idempotency_key.as_deref(),
            Some(format!("runtime:steer:{input_id}").as_str())
        );
    }

    #[test]
    fn steer_projection_filters_empty_context_and_empty_append() {
        let input_id = InputId::new();
        let context_projection = crate::ingress_types::RuntimeInputProjection {
            injected_context_appends: Vec::new(),
            append: None,
            additional_appends: Vec::new(),
            context_append: Some(ConversationContextAppend {
                key: "empty-context".into(),
                content: CoreRenderable::Text { text: "  ".into() },
            }),
            peer_response_terminal: None,
        };
        assert!(
            projection_to_pending_system_context_appends(&input_id, &context_projection).is_empty()
        );

        let append_projection = crate::ingress_types::RuntimeInputProjection {
            injected_context_appends: Vec::new(),
            append: Some(ConversationAppend {
                role: ConversationAppendRole::SystemNotice,
                content: CoreRenderable::Text { text: "\n".into() },
            }),
            additional_appends: Vec::new(),
            context_append: None,
            peer_response_terminal: None,
        };
        assert!(
            projection_to_pending_system_context_appends(&input_id, &append_projection).is_empty()
        );
    }

    #[test]
    fn peer_response_terminal_with_blocks_projects_append_and_context() {
        let route_id = "018f6f79-7a82-7c4e-a552-a3b86f9630f2";
        let request_id = "018f6f79-7a82-7c4e-a552-a3b86f9630f1";
        let mut header = make_header();
        header.source = InputOrigin::Peer {
            peer_id: route_id.into(),
            display_identity: Some("display-agent".into()),
            runtime_id: None,
        };
        let input = Input::Peer(PeerInput {
            injected_context: Vec::new(),
            sender_taint: None,
            header,
            convention: Some(PeerConvention::ResponseTerminal {
                request_id: request_id.into(),
                status: ResponseTerminalStatus::Completed,
            }),
            content: ContentInput::Blocks(vec![meerkat_core::types::ContentBlock::Image {
                media_type: "image/jpeg".into(),
                data: "abc".into(),
            }]),
            payload: Some(serde_json::json!({"answer":"ok"})),
            handling_mode: None,
        });

        let projection = runtime_input_projection_for_machine_batch(&input);
        let append = projection.append.expect("conversation append");
        let CoreRenderable::SystemNotice { blocks, .. } = append.content else {
            panic!("expected typed append");
        };
        let Some(meerkat_core::types::SystemNoticeBlock::Comms { content, peer, .. }) =
            blocks.first()
        else {
            panic!("expected comms block");
        };
        assert_eq!(
            peer.as_ref().and_then(|peer| peer.display_name.as_deref()),
            Some("display-agent")
        );
        assert!(matches!(
            content.first(),
            Some(meerkat_core::types::ContentBlock::Image { media_type, .. })
                if media_type == "image/jpeg"
        ));
        assert!(
            projection.context_append.is_some(),
            "terminal response must still apply runtime-owned context"
        );
    }

    #[test]
    fn peer_input_request_serde() {
        let input = Input::Peer(PeerInput {
            injected_context: Vec::new(),
            sender_taint: None,
            header: make_header(),
            convention: Some(PeerConvention::Request {
                request_id: "req-1".into(),
                intent: "mob.peer_added".into(),
            }),
            content: "Agent joined".into(),
            payload: Some(serde_json::json!({"name": "agent-1"})),
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
            injected_context: Vec::new(),
            sender_taint: None,
            header: make_header(),
            convention: Some(PeerConvention::ResponseTerminal {
                request_id: "req-1".into(),
                status: ResponseTerminalStatus::Completed,
            }),
            content: "Done".into(),
            payload: Some(serde_json::json!({"ok": true})),
            handling_mode: None,
        });
        let json = serde_json::to_value(&input).unwrap();
        let parsed: Input = serde_json::from_value(json).unwrap();
        assert!(matches!(parsed, Input::Peer(_)));
    }

    #[test]
    fn peer_input_response_progress_serde() {
        let input = Input::Peer(PeerInput {
            injected_context: Vec::new(),
            sender_taint: None,
            header: make_header(),
            convention: Some(PeerConvention::ResponseProgress {
                request_id: "req-1".into(),
                phase: ResponseProgressPhase::InProgress,
            }),
            content: "Working...".into(),
            payload: Some(serde_json::json!({"progress": "working"})),
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
            content: ContentInput::Blocks(vec![
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
    fn legacy_external_event_payload_blocks_are_rejected() {
        // The retired shape smuggled multimodal blocks inside the payload
        // JSON. It must fail closed with a typed error — never migrate.
        let event = ExternalEventInput {
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
        };

        let err = reject_legacy_payload_blocks(&event)
            .expect_err("payload-level blocks must fail closed");
        assert!(matches!(err, BlobStoreError::Internal(_)));
        // Payload is untouched: rejection never strips or rewrites it.
        assert!(event.payload.get("blocks").is_some());
        assert!(event.blocks.is_none());
    }

    #[test]
    fn external_event_payload_without_blocks_key_passes_rejection_gate() {
        let event = ExternalEventInput {
            header: make_header(),
            event_type: "webhook.received".into(),
            payload: serde_json::json!({ "body": "plain payload" }),
            blocks: Some(vec![meerkat_core::types::ContentBlock::Text {
                text: "typed owner content".into(),
            }]),
            handling_mode: HandlingMode::Queue,
            render_metadata: None,
        };

        reject_legacy_payload_blocks(&event)
            .expect("payload without a legacy blocks key must pass");
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
    fn continuation_input_rejects_legacy_system_generated_tag() {
        // The pre-rename `system_generated` tag is a retired persisted shape:
        // it must fail closed instead of being folded into `continuation`.
        let input = Input::Continuation(ContinuationInput::detached_background_op_completed());
        let mut json = serde_json::to_value(&input).unwrap();
        json["input_type"] = serde_json::Value::String("system_generated".into());
        serde_json::from_value::<Input>(json)
            .expect_err("legacy system_generated input_type tag must be rejected");
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
    fn operation_input_rejects_legacy_projected_tag() {
        // The pre-rename `projected` tag is a retired persisted shape: it
        // must fail closed instead of being folded into `operation`.
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
        serde_json::from_value::<Input>(json)
            .expect_err("legacy projected input_type tag must be rejected");
    }

    #[test]
    fn legacy_dual_carrier_input_shapes_are_rejected() {
        // The retired persisted shape stored the content fact twice: a textual
        // carrier (`text` / `body` / `instructions`) plus optional `blocks`.
        // The single typed `content` owner replaced both; old shapes must fail
        // closed instead of being coerced.
        let header = serde_json::to_value(make_header()).unwrap();

        let legacy_prompt = serde_json::json!({
            "input_type": "prompt",
            "header": header.clone(),
            "text": "hello",
            "blocks": null
        });
        serde_json::from_value::<Input>(legacy_prompt)
            .expect_err("legacy prompt text+blocks shape must be rejected");

        let legacy_peer = serde_json::json!({
            "input_type": "peer",
            "header": header.clone(),
            "convention": { "convention_type": "message" },
            "body": "hi there"
        });
        serde_json::from_value::<Input>(legacy_peer)
            .expect_err("legacy peer body+blocks shape must be rejected");

        let legacy_flow_step = serde_json::json!({
            "input_type": "flow_step",
            "header": header,
            "step_id": "step-1",
            "instructions": "analyze the data"
        });
        serde_json::from_value::<Input>(legacy_flow_step)
            .expect_err("legacy flow-step instructions+blocks shape must be rejected");
    }

    #[test]
    fn input_kind_id() {
        let prompt = Input::Prompt(PromptInput {
            injected_context: Vec::new(),
            header: make_header(),
            content: "hi".into(),
            typed_turn_appends: Vec::new(),
            turn_metadata: None,
        });
        assert_eq!(prompt.kind(), InputKind::Prompt);

        let peer_msg = Input::Peer(PeerInput {
            injected_context: Vec::new(),
            sender_taint: None,
            header: make_header(),
            convention: Some(PeerConvention::Message),
            content: "hi".into(),
            payload: None,
            handling_mode: None,
        });
        assert_eq!(peer_msg.kind(), InputKind::PeerMessage);

        let peer_req = Input::Peer(PeerInput {
            injected_context: Vec::new(),
            sender_taint: None,
            header: make_header(),
            convention: Some(PeerConvention::Request {
                request_id: "r".into(),
                intent: "i".into(),
            }),
            content: "hi".into(),
            payload: Some(serde_json::json!({"subject": "x"})),
            handling_mode: None,
        });
        assert_eq!(peer_req.kind(), InputKind::PeerRequest);

        let continuation = Input::Continuation(ContinuationInput {
            header: make_header(),
            reason: "continue".into(),
            continuation_kind: ContinuationKind::Ordinary,
            handling_mode: HandlingMode::Steer,
            request_id: None,
            flow_tool_overlay: None,
            context_append: None,
            turn_append: None,
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
        // Serialized PeerInput without the optional handling_mode field.
        let json = serde_json::json!({
            "input_type": "peer",
            "header": serde_json::to_value(make_header()).unwrap(),
            "convention": { "convention_type": "message" },
            "content": "hello"
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
            injected_context: Vec::new(),
            sender_taint: None,
            header: make_header(),
            convention: Some(PeerConvention::Message),
            content: "hi".into(),
            payload: None,
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
            meerkat_contracts::PeerResponseTerminalStatusWire::Completed,
            serde_json::json!({"ok": true}),
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
                assert_eq!(status, ResponseTerminalStatus::Completed);
                assert_eq!(payload["ok"], true);
            }
            other => panic!("expected terminal peer input, got {other:?}"),
        }
    }

    #[test]
    fn peer_response_terminal_validation_is_structural_only() {
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

        validate_peer_response_terminal_fact(&input)
            .expect("status support is generated admission authority, structural fact validation should pass");
    }

    #[test]
    fn peer_input_with_steer_handling_mode_roundtrips() {
        let input = Input::Peer(PeerInput {
            injected_context: Vec::new(),
            sender_taint: None,
            header: make_header(),
            convention: Some(PeerConvention::Message),
            content: "hi".into(),
            payload: None,
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
            injected_context: Vec::new(),
            sender_taint: None,
            header: make_header(),
            convention: Some(PeerConvention::Message),
            content: "hi".into(),
            payload: None,
            handling_mode: None,
        });
        let json = serde_json::to_value(&input).unwrap();
        assert!(json.get("handling_mode").is_none());
    }
}
