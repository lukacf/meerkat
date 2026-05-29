//! meerkat-runtime — v9 runtime control-plane for Meerkat agent lifecycle.
//!
//! This crate implements the runtime/control-plane layer of the v9 Canonical
//! Lifecycle specification. It sits between surfaces (CLI, RPC, REST, MCP)
//! and core (`meerkat-core`), managing:
//!
//! - Input acceptance, validation, and queueing
//! - InputState lifecycle tracking
//! - Policy resolution (what to do with each input)
//! - Runtime state machine (Initializing ↔ Idle ↔ Attached ↔ Running ↔ Retired/Stopped/Destroyed)
//! - Retire/recycle/reset lifecycle operations
//! - RuntimeEvent observability
//!
//! Core-facing types (RunPrimitive, RunEvent, CoreExecutor, etc.) live in
//! `meerkat-core::lifecycle`. This crate contains everything else.

#![cfg_attr(
    test,
    allow(
        dead_code,
        unused_imports,
        clippy::expect_used,
        clippy::large_futures,
        clippy::needless_borrow,
        clippy::panic,
        clippy::redundant_closure_for_method_calls,
        clippy::redundant_clone,
        clippy::type_complexity,
        clippy::unnecessary_to_owned,
        clippy::unwrap_used
    )
)]

#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

#[cfg(not(target_arch = "wasm32"))]
pub use ::tokio;

pub mod accept;
pub mod auth_machine;
pub mod coalescing;
pub mod comms_bridge;
pub mod comms_drain;
pub mod comms_trust_reconcile;
pub mod completion;
pub mod composition;
pub(crate) mod control_plane;
pub mod driver;
pub mod durability;
pub(crate) mod effect;
#[doc(hidden)]
pub mod generated;
pub mod handles;
pub mod identifiers;
pub mod ingress_types;
pub mod input;
pub mod input_ledger;
pub mod input_scope;
pub mod input_state;
pub mod interrupt_public_result;
pub mod meerkat_machine;
pub(crate) mod meerkat_machine_types;
pub mod mob_adapter;
pub mod mob_operator_authority;
pub mod ops_lifecycle;
pub mod peer_handling_mode;
pub mod policy;
pub mod policy_table;
#[allow(unused_imports)]
#[path = "generated/protocol_auth_lease_lifecycle_publication.rs"]
pub mod protocol_auth_lease_lifecycle_publication;
#[allow(unused_imports)]
#[path = "generated/protocol_comms_trust_reconcile.rs"]
pub mod protocol_comms_trust_reconcile;
#[allow(unused_imports)]
#[path = "generated/protocol_supervisor_trust_publish.rs"]
pub mod protocol_supervisor_trust_publish;
#[allow(unused_imports)]
#[path = "generated/protocol_supervisor_trust_revoke.rs"]
pub mod protocol_supervisor_trust_revoke;
pub mod queue;
pub mod runtime_event;
pub(crate) mod runtime_loop;
pub mod runtime_state;
pub mod service_ext;
pub(crate) mod silent_intent;
pub mod store;
pub mod traits;

use meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata as RuntimeStampedTurnMetadata;
use std::any::Any;
use std::sync::Arc;

struct SessionRuntimeBindingsAuthority;

pub(crate) fn session_runtime_bindings_authority() -> Arc<dyn Any + Send + Sync> {
    Arc::new(SessionRuntimeBindingsAuthority)
}

pub(crate) fn local_session_runtime_bindings_authority() -> Arc<dyn Any + Send + Sync> {
    session_runtime_bindings_authority()
}

pub fn session_runtime_bindings_have_machine_authority(
    bindings: &meerkat_core::SessionRuntimeBindings,
) -> bool {
    bindings
        .__runtime_authority()
        .is::<SessionRuntimeBindingsAuthority>()
}

// Re-exports for convenience
pub use accept::{AcceptOutcome, RejectReason};
pub use coalescing::{
    AggregateDescriptor, CoalescingResult, SupersessionScope, check_supersession,
    create_aggregate_input, is_coalescing_eligible,
};
pub use completion::{
    CompletionCleanupObservation, CompletionHandle, CompletionOutcome, CompletionWaitError,
};
pub use driver::{EphemeralRuntimeDriver, PersistentRuntimeDriver, PostAdmissionSignal};
pub use durability::DurabilityError;
pub use handles::{
    HandleDslAuthority, RuntimeAuthLeaseHandle, RuntimeCommsDrainHandle,
    RuntimeExternalToolSurfaceHandle, RuntimeInteractionStreamHandle,
    RuntimeMcpServerLifecycleHandle, RuntimeModelRoutingHandle, RuntimePeerCommsHandle,
    RuntimePeerInteractionHandle, RuntimeSessionAdmissionHandle, RuntimeSessionContextHandle,
    RuntimeTurnStateHandle,
};
pub use identifiers::{
    CausationId, ConversationId, CorrelationId, EventCodeId, IdempotencyKey, InputKind, KindId,
    LogicalRuntimeId, PolicyVersion, ProjectionRuleId, RuntimeEventId, SchemaId, SupersessionKey,
};
pub use ingress_types::{ContentShape, RequestId, ReservationKey};
pub use input::{
    ContinuationInput, ExternalEventInput, FlowStepInput, Input, InputDurability, InputHeader,
    InputOrigin, InputVisibility, OperationInput, PeerConvention, PeerInput, PromptInput,
    ResponseProgressPhase, ResponseTerminalStatus, peer_response_terminal_input,
    response_terminal_status_from_wire,
};
pub use input_ledger::InputLedger;
pub use input_scope::InputScope;
pub use input_state::{
    InputAbandonReason, InputLifecycleState, InputState, InputStateEvent, InputStateHistoryEntry,
    InputTerminalOutcome, PolicySnapshot, ReconstructionSource,
};
pub use meerkat_core::types::HandlingMode;
pub use meerkat_machine::{
    CommsDrainMode, CommsDrainPhase, DrainExitReason, MachineSessionControlAuthority,
    MeerkatConsumerSurface, MeerkatMachine, PeerIngressOwner, RuntimeBindingsError,
    RuntimeLifecycleFacts, RuntimeLoopQueueAdmissionPlan, classify_runtime_lifecycle_state,
    classify_runtime_loop_queue_admission, standalone_tool_visibility_owner,
};
pub use meerkat_machine_types::{
    HydratedSessionLlmState, ImageOperationRoutingRequest, ImageOperationRoutingResult,
    ModelRoutingApprovalDisposition, ModelRoutingRealtimePolicy, ResolvedSessionLlmReconfigure,
    SessionLlmCapabilitySurface, SessionLlmCapabilitySurfaceStatus, SessionLlmReconfigureHost,
    SessionLlmReconfigureReport, SessionLlmReconfigureRequest, SessionToolVisibilityDelta,
};
#[doc(hidden)]
pub use meerkat_machine_types::{
    MeerkatAdmittedInputSnapshot, MeerkatArchiveSnapshot, MeerkatBindingSnapshot,
    MeerkatCompletionWaiterSnapshot, MeerkatCompletionWaitersSnapshot, MeerkatControlSnapshot,
    MeerkatCursorSnapshot, MeerkatDrainSnapshot, MeerkatDriverKind, MeerkatInputsSnapshot,
    MeerkatMachineCatalogInput, MeerkatMachineCommandClassification,
    MeerkatMachineCommandClassificationRecord, MeerkatMachineCommandVariant,
    MeerkatMachineFieldlessRuntimeInternalInput, MeerkatMachineRuntimeInternalClassificationRecord,
    MeerkatMachineRuntimeInternalInput, MeerkatMachineRuntimeInternalReason,
    MeerkatMachineShellMechanicReason, MeerkatMachineSpineSnapshot, MeerkatOpsSnapshot,
    canonical_meerkat_machine_command_classifications,
    canonical_meerkat_machine_command_input_variant_manifest,
    canonical_meerkat_machine_command_manifest,
    canonical_meerkat_machine_runtime_internal_classifications,
    canonical_meerkat_machine_runtime_internal_fieldless_input_variant_manifest,
    canonical_meerkat_machine_runtime_internal_input_variant_manifest,
    canonical_meerkat_machine_runtime_internal_manifest,
};
pub use ops_lifecycle::{
    OpsLifecycleConfig, OpsLifecyclePersistenceRequest, PersistedOpsSnapshot,
    RuntimeOpsLifecycleRegistry,
};

#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "test-support")))]
#[doc(hidden)]
pub fn test_peer_comms_handle() -> Arc<dyn meerkat_core::handles::PeerCommsHandle> {
    test_peer_comms_handle_with_silent(std::iter::empty::<String>())
}

#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "test-support")))]
#[doc(hidden)]
#[allow(clippy::expect_used)]
pub fn test_peer_comms_handle_with_silent<I, S>(
    silent_intents: I,
) -> Arc<dyn meerkat_core::handles::PeerCommsHandle>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let silent_intents = silent_intents
        .into_iter()
        .map(Into::into)
        .collect::<Vec<_>>();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test peer-comms runtime should build");
        runtime.block_on(async move {
            let machine = MeerkatMachine::ephemeral();
            let session_id = meerkat_core::SessionId::new();
            let bindings = machine
                .prepare_bindings(session_id.clone())
                .await
                .expect("generated MeerkatMachine should prepare test peer-comms bindings");
            if !silent_intents.is_empty() {
                machine
                    .set_session_silent_intents(&session_id, silent_intents)
                    .await;
            }
            Arc::clone(bindings.peer_comms())
        })
    })
    .join()
    .expect("test peer-comms authority thread should finish")
}

#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "test-support")))]
#[doc(hidden)]
#[allow(clippy::expect_used)]
pub fn test_peer_input_candidate_from_interaction(
    interaction: meerkat_core::interaction::InboxInteraction,
    peer_id: meerkat_core::comms::PeerId,
) -> meerkat_core::interaction::PeerInputCandidate {
    use meerkat_core::interaction::{
        InteractionContent, InteractionId, PeerIngressEnvelopeFacts, PeerIngressEnvelopeKind,
        PeerIngressFact, PeerIngressIdentity,
    };

    let handle = test_peer_comms_handle();
    let facts = PeerIngressEnvelopeFacts {
        item_id: interaction.id.to_string(),
        from_peer: interaction.from.clone(),
        from_peer_id: peer_id,
        kind: match &interaction.content {
            InteractionContent::Message { body, .. } => {
                PeerIngressEnvelopeKind::Message { body: body.clone() }
            }
            InteractionContent::Request { intent, params, .. } => {
                PeerIngressEnvelopeKind::Request {
                    intent: intent.clone(),
                    params: params.clone(),
                }
            }
            InteractionContent::Response {
                in_reply_to,
                status,
                result,
                ..
            } => PeerIngressEnvelopeKind::Response {
                in_reply_to: in_reply_to.to_string(),
                status: *status,
                result: result.clone(),
            },
        },
    };
    let admission = handle
        .classify_external_envelope(facts)
        .expect("generated peer-comms authority should classify test interaction");
    let classification = admission.classification;
    let convention = match &interaction.content {
        InteractionContent::Message { .. } => meerkat_core::PeerIngressConvention::Message,
        InteractionContent::Request { intent, .. } => {
            if let Some(kind) = classification.lifecycle_kind {
                let peer = admission
                    .lifecycle_peer
                    .clone()
                    .expect("generated lifecycle classification should include a peer subject");
                meerkat_core::PeerIngressConvention::Lifecycle { kind, peer }
            } else {
                let request_id = admission
                    .request_id
                    .clone()
                    .expect("generated request classification should include request id");
                meerkat_core::PeerIngressConvention::Request {
                    request_id,
                    intent: intent.clone(),
                }
            }
        }
        InteractionContent::Response { status, .. } => {
            let in_reply_to = admission
                .request_id
                .as_deref()
                .and_then(|id| uuid::Uuid::parse_str(id).ok())
                .map(InteractionId)
                .expect("generated response classification should include in-reply-to id");
            meerkat_core::PeerIngressConvention::Response {
                in_reply_to,
                status: *status,
            }
        }
    };
    let ingress = PeerIngressFact::peer(
        interaction.id,
        classification.class,
        classification.kind,
        Some(classification.auth),
        PeerIngressIdentity::new(peer_id, interaction.from.clone(), convention),
    );
    let mut candidate = meerkat_core::interaction::PeerInputCandidate::new(
        interaction,
        ingress,
        admission.lifecycle_peer,
    );
    candidate.response_terminality = classification.response_terminality;
    candidate
}

/// Stamp prompt turn metadata with the runtime-owned input semantics.
///
/// This helper exists for runtime-backed service-turn paths that already hold
/// machine admission and must pass a runtime-classified prompt turn into the
/// session layer. New prompt materialization should prefer `MeerkatMachine`
/// input admission so the machine creates this metadata directly.
pub fn runtime_stamped_prompt_turn_metadata(
    metadata: Option<RuntimeStampedTurnMetadata>,
) -> RuntimeStampedTurnMetadata {
    let input = Input::Prompt(PromptInput::from_content_input(
        meerkat_core::ContentInput::Text(String::new()),
        metadata,
    ));
    let semantics = runtime_prompt_semantics_from_machine(&input);
    runtime_loop::for_input(&input, semantics)
}

#[allow(clippy::expect_used)]
fn runtime_prompt_semantics_from_machine(input: &Input) -> ingress_types::RuntimeInputSemantics {
    let mut authority = meerkat_machine::dsl_authority::new_initialized_authority(
        "generated runtime prompt machine authority must initialize",
    );
    let transition = meerkat_machine::dsl::MeerkatMachineMutator::apply(
        &mut authority,
        meerkat_machine::dsl::MeerkatMachineInput::ResolveAdmissionPlan {
            input_id: input.id().to_string(),
            input_kind: meerkat_machine::dsl::AdmissionInputKind::from(input.kind()),
            requested_lane: input
                .handling_mode()
                .map(meerkat_machine::dsl::InputLane::from),
            silent_intent_match: false,
            existing_superseded_input_id: None,
            runtime_running: false,
            active_turn_boundary_available: false,
            without_wake: false,
        },
    )
    .expect("generated admission authority must accept runtime prompt metadata");

    transition
        .into_effects()
        .into_iter()
        .find_map(|effect| match effect {
            meerkat_machine::dsl::MeerkatMachineEffect::AdmissionResolved {
                runtime_boundary,
                runtime_execution_kind,
                runtime_peer_response_terminal_apply_intent,
                ..
            } => Some(ingress_types::RuntimeInputSemantics {
                boundary: runtime_boundary.into(),
                execution_kind: runtime_execution_kind.into(),
                peer_response_terminal_apply_intent: runtime_peer_response_terminal_apply_intent
                    .map(Into::into),
            }),
            _ => None,
        })
        .expect("generated admission authority must emit prompt runtime semantics")
}

#[cfg(test)]
mod runtime_prompt_metadata_tests {
    #[test]
    fn runtime_stamped_prompt_turn_metadata_uses_generated_prompt_semantics() {
        let metadata = super::runtime_stamped_prompt_turn_metadata(None);
        assert_eq!(
            metadata.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn)
        );
        assert!(metadata.peer_response_terminal_apply_intent.is_none());
    }
}

#[doc(hidden)]
pub mod machine_schema_exports {
    pub fn meerkat_machine_schema() -> meerkat_machine_schema::MachineSchema {
        meerkat_machine_schema::catalog::dsl::meerkat_machine_schema_metadata()
            .attach_to(crate::meerkat_machine::dsl::MeerkatMachineState::schema())
    }

    pub fn auth_machine_schema() -> meerkat_machine_schema::MachineSchema {
        meerkat_machine_schema::catalog::dsl::auth_machine_schema_metadata()
            .attach_to(crate::auth_machine::dsl::AuthMachineState::schema())
    }
}
pub use interrupt_public_result::{
    UserInterruptObservation, UserInterruptPublicResult, resolve_user_interrupt_public_result,
};
pub use peer_handling_mode::{PeerHandlingModeError, validate_peer_handling_mode};
pub use policy::{
    ApplyMode, ConsumePoint, DrainPolicy, PolicyDecision, QueueMode, RoutingDisposition, WakeMode,
};
pub use policy_table::{DefaultPolicyTable, generated_default_policy_version};
pub use queue::InputQueue;
pub use runtime_event::{
    InputLifecycleEvent, RunLifecycleEvent, RuntimeEvent, RuntimeEventEnvelope,
    RuntimeProjectionEvent, RuntimeStateChangeEvent, RuntimeTopologyEvent,
};
pub use runtime_state::{RuntimeState, RuntimeStateTransitionError};
pub use service_ext::{RuntimeMode, SessionServiceRuntimeExt};
pub use store::{InMemoryRuntimeStore, RuntimeStore, RuntimeStoreError, SessionDelta};
pub use traits::{
    DestroyReport, RecoveryReport, RecycleReport, ResetReport, RetireReport, RuntimeControlPlane,
    RuntimeControlPlaneError, RuntimeDriver, RuntimeDriverError,
};
