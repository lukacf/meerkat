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
pub mod member_live;
pub mod member_observation;
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
#[path = "generated/protocol_auth_release_oauth_flow_drain.rs"]
pub mod protocol_auth_release_oauth_flow_drain;
#[allow(unused_imports)]
#[path = "generated/protocol_comms_trust_reconcile.rs"]
pub mod protocol_comms_trust_reconcile;
#[allow(unused_imports)]
#[path = "generated/protocol_supervisor_trust_publish.rs"]
pub mod protocol_supervisor_trust_publish;
#[allow(unused_imports)]
#[path = "generated/protocol_supervisor_trust_revoke.rs"]
pub mod protocol_supervisor_trust_revoke;
pub(crate) mod queue;
pub mod runtime_event;
pub(crate) mod runtime_loop;
pub mod runtime_state;
pub mod service_ext;
pub(crate) mod silent_intent;
pub mod store;
pub mod terminal_status;
pub mod traits;

use meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata as RuntimeStampedTurnMetadata;
use std::any::Any;
use std::sync::Arc;

pub(crate) struct SessionRuntimeBindingsAuthority {
    pub(crate) session_id: meerkat_core::SessionId,
    pub(crate) epoch_id: meerkat_core::RuntimeEpochId,
    pub(crate) dsl_authority: Arc<std::sync::Mutex<meerkat_machine::dsl::MeerkatMachineAuthority>>,
    pub(crate) teardown_gate: Arc<handles::HandleTeardownGate>,
    pub(crate) materialization_claim_id: Option<uuid::Uuid>,
    pub(crate) materialization_claim_state:
        Arc<std::sync::Mutex<RuntimeActorMaterializationClaimState>>,
    /// Compatibility capability for cloneable `prepare_bindings()` results.
    /// It is minted only while the registration is unattached and does not
    /// itself reserve the exact materialization claim. `begin_*` atomically
    /// converts it into a one-shot claim if the window is still vacant.
    pub(crate) legacy_actor_materialization_generation: Option<u64>,
    pub(crate) release_materialization_claim_on_drop: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeActorMaterializationClaimPhase {
    Vacant,
    Prepared,
    Staged,
    ActorCreating,
    ActorMaterializedPendingCommit,
    RetainedActor,
    Aborting,
}

pub(crate) struct RuntimeActorMaterializationClaimState {
    pub(crate) current: Option<uuid::Uuid>,
    pub(crate) phase: RuntimeActorMaterializationClaimPhase,
    /// True only when this registry entry was inserted by a unique,
    /// rollback-owning actor materialization that has not yet attached an
    /// executor. A successor unique prepare inherits this exact rollback
    /// authority; cloneable compatibility bindings and pre-existing committed
    /// registrations never gain it.
    pub(crate) rollback_registration_available: bool,
    /// Monotonic incarnation of the cloneable compatibility binding window.
    /// Executor attachment increments this under the same mutex as the claim
    /// phase, permanently fencing bindings that escaped an older window.
    pub(crate) legacy_capability_generation: u64,
    pub(crate) changed: Arc<crate::tokio::sync::Notify>,
}

impl RuntimeActorMaterializationClaimState {
    pub(crate) fn new(rollback_registration_available: bool) -> Self {
        Self {
            current: None,
            phase: RuntimeActorMaterializationClaimPhase::Vacant,
            rollback_registration_available,
            legacy_capability_generation: 0,
            changed: Arc::new(crate::tokio::sync::Notify::new()),
        }
    }

    pub(crate) fn exact_claim_is(
        &self,
        claim_id: uuid::Uuid,
        phases: &[RuntimeActorMaterializationClaimPhase],
    ) -> bool {
        self.current == Some(claim_id) && phases.contains(&self.phase)
    }
}

impl Drop for SessionRuntimeBindingsAuthority {
    fn drop(&mut self) {
        if !self.release_materialization_claim_on_drop {
            return;
        }
        let Some(claim_id) = self.materialization_claim_id else {
            return;
        };
        let changed = {
            let mut state = self
                .materialization_claim_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !state.exact_claim_is(
                claim_id,
                &[
                    RuntimeActorMaterializationClaimPhase::Prepared,
                    RuntimeActorMaterializationClaimPhase::Staged,
                ],
            ) {
                return;
            }
            state.current = None;
            state.phase = RuntimeActorMaterializationClaimPhase::Vacant;
            Arc::clone(&state.changed)
        };
        changed.notify_waiters();
    }
}

// Constructor mirrors the opaque session-binding authority payload exactly;
// keeping each carrier explicit prevents partial or reordered minting.
#[allow(clippy::too_many_arguments)]
pub(crate) fn session_runtime_bindings_authority(
    session_id: meerkat_core::SessionId,
    epoch_id: meerkat_core::RuntimeEpochId,
    dsl_authority: Arc<std::sync::Mutex<meerkat_machine::dsl::MeerkatMachineAuthority>>,
    teardown_gate: Arc<handles::HandleTeardownGate>,
    materialization_claim_id: Option<uuid::Uuid>,
    materialization_claim_state: Arc<std::sync::Mutex<RuntimeActorMaterializationClaimState>>,
    legacy_actor_materialization_generation: Option<u64>,
    release_materialization_claim_on_drop: bool,
) -> Arc<dyn Any + Send + Sync> {
    Arc::new(SessionRuntimeBindingsAuthority {
        session_id,
        epoch_id,
        dsl_authority,
        teardown_gate,
        materialization_claim_id,
        materialization_claim_state,
        legacy_actor_materialization_generation,
        release_materialization_claim_on_drop,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn local_session_runtime_bindings_authority(
    session_id: meerkat_core::SessionId,
    epoch_id: meerkat_core::RuntimeEpochId,
    dsl_authority: Arc<std::sync::Mutex<meerkat_machine::dsl::MeerkatMachineAuthority>>,
    teardown_gate: Arc<handles::HandleTeardownGate>,
    materialization_claim_id: Option<uuid::Uuid>,
    materialization_claim_state: Arc<std::sync::Mutex<RuntimeActorMaterializationClaimState>>,
    legacy_actor_materialization_generation: Option<u64>,
    release_materialization_claim_on_drop: bool,
) -> Arc<dyn Any + Send + Sync> {
    session_runtime_bindings_authority(
        session_id,
        epoch_id,
        dsl_authority,
        teardown_gate,
        materialization_claim_id,
        materialization_claim_state,
        legacy_actor_materialization_generation,
        release_materialization_claim_on_drop,
    )
}

pub fn session_runtime_bindings_have_machine_authority(
    bindings: &meerkat_core::SessionRuntimeBindings,
) -> bool {
    bindings
        .__runtime_authority()
        .is::<SessionRuntimeBindingsAuthority>()
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeActorMaterializationError {
    #[error("invalid runtime binding materialization authority: {0}")]
    InvalidAuthority(String),
    #[error("runtime binding registration no longer admits actor materialization")]
    RegistrationClosed,
}

/// Exclusive actor-create permit for one exact prepared runtime binding.
///
/// The persistent session service acquires this immediately before it starts
/// building the live actor. Dropping an uncommitted permit restores the prior
/// prepared/staged phase; committing it records that the actor exists but is
/// still owned by the surrounding materialization transaction until executor
/// attachment or an explicit retained-actor commit.
pub struct RuntimeActorMaterializationPermit {
    claim_id: uuid::Uuid,
    claim_state: Arc<std::sync::Mutex<RuntimeActorMaterializationClaimState>>,
    session_id: meerkat_core::SessionId,
    epoch_id: meerkat_core::RuntimeEpochId,
    dsl_authority: Arc<std::sync::Mutex<meerkat_machine::dsl::MeerkatMachineAuthority>>,
    teardown_gate: Arc<handles::HandleTeardownGate>,
    previous_phase: RuntimeActorMaterializationClaimPhase,
    transactional: bool,
    phase_policy: RuntimeActorMaterializationPhasePolicy,
    _mutation_guard: Option<crate::tokio::sync::OwnedMutexGuard<()>>,
    committed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeActorMaterializationPhasePolicy {
    RejectRetired,
    RequireRetired,
}

impl RuntimeActorMaterializationPermit {
    pub fn commit(mut self) -> Result<(), RuntimeActorMaterializationError> {
        let generated_authority = self
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        validate_materialization_registration_authority(
            &self.session_id,
            &self.epoch_id,
            &self.teardown_gate,
            &generated_authority,
            self.phase_policy,
        )?;
        let mut state = self
            .claim_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.exact_claim_is(
            self.claim_id,
            &[RuntimeActorMaterializationClaimPhase::ActorCreating],
        ) {
            return Err(RuntimeActorMaterializationError::RegistrationClosed);
        }
        let changed = if self.transactional {
            state.phase = RuntimeActorMaterializationClaimPhase::ActorMaterializedPendingCommit;
            None
        } else {
            state.current = None;
            state.phase = RuntimeActorMaterializationClaimPhase::RetainedActor;
            state.rollback_registration_available = false;
            Some(Arc::clone(&state.changed))
        };
        self.committed = true;
        drop(state);
        if let Some(changed) = changed {
            changed.notify_waiters();
        }
        Ok(())
    }
}

impl Drop for RuntimeActorMaterializationPermit {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let mut state = self
            .claim_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.exact_claim_is(
            self.claim_id,
            &[RuntimeActorMaterializationClaimPhase::ActorCreating],
        ) {
            if self.transactional {
                state.phase = self.previous_phase;
            } else {
                state.current = None;
                state.phase = RuntimeActorMaterializationClaimPhase::Vacant;
                Arc::clone(&state.changed).notify_waiters();
            }
        }
    }
}

fn validated_session_runtime_bindings_authority(
    bindings: &meerkat_core::SessionRuntimeBindings,
) -> Result<&SessionRuntimeBindingsAuthority, RuntimeActorMaterializationError> {
    let authority = bindings
        .__runtime_authority()
        .downcast_ref::<SessionRuntimeBindingsAuthority>()
        .ok_or_else(|| {
            RuntimeActorMaterializationError::InvalidAuthority(
                "session runtime bindings lack MeerkatMachine authority".to_string(),
            )
        })?;
    if bindings.session_id() != &authority.session_id || bindings.epoch_id() != &authority.epoch_id
    {
        return Err(RuntimeActorMaterializationError::InvalidAuthority(
            "session runtime binding identity does not match its machine authority".into(),
        ));
    }
    Ok(authority)
}

fn validate_materialization_registration_authority(
    session_id: &meerkat_core::SessionId,
    epoch_id: &meerkat_core::RuntimeEpochId,
    teardown_gate: &Arc<handles::HandleTeardownGate>,
    generated_authority: &meerkat_machine::dsl::MeerkatMachineAuthority,
    phase_policy: RuntimeActorMaterializationPhasePolicy,
) -> Result<(), RuntimeActorMaterializationError> {
    let state = generated_authority.state();
    let expected_session_id = meerkat_machine::dsl::SessionId::from_domain(session_id);
    let expected_epoch_id = meerkat_machine::dsl::RuntimeEpochId::from_domain(epoch_id);
    let runtime_phase =
        meerkat_machine::dsl_authority::runtime_phase_from_authority(generated_authority);
    if !teardown_gate.is_open()
        || state.session_id.as_ref() != Some(&expected_session_id)
        || state.registration_phase == meerkat_machine::dsl::RegistrationPhase::Draining
        || matches!(
            runtime_phase,
            crate::runtime_state::RuntimeState::Stopped
                | crate::runtime_state::RuntimeState::Destroyed
        )
        || match phase_policy {
            RuntimeActorMaterializationPhasePolicy::RejectRetired => {
                runtime_phase == crate::runtime_state::RuntimeState::Retired
            }
            RuntimeActorMaterializationPhasePolicy::RequireRetired => {
                runtime_phase != crate::runtime_state::RuntimeState::Retired
            }
        }
        || state
            .active_runtime_epoch_id
            .as_ref()
            .is_some_and(|epoch_id| epoch_id != &expected_epoch_id)
    {
        return Err(RuntimeActorMaterializationError::RegistrationClosed);
    }
    Ok(())
}

/// Begin exclusive construction of the live actor for an exact prepared
/// binding. This is the cancellation-safe successor to the read-only validator.
pub fn begin_session_runtime_actor_materialization(
    bindings: &meerkat_core::SessionRuntimeBindings,
) -> Result<RuntimeActorMaterializationPermit, RuntimeActorMaterializationError> {
    begin_session_runtime_actor_materialization_with_phase_policy(
        bindings,
        RuntimeActorMaterializationPhasePolicy::RejectRetired,
        None,
    )
}

/// Begin actor construction for the exact machine-authorized
/// Archived+Retired revival midpoint.
///
/// The public archived-resume service path remains closed; only a caller that
/// already holds MeerkatMachine's session-control capability may construct the
/// temporary live actor that the same revival transaction will promote to
/// Active+Idle before executor attachment.
pub async fn begin_session_runtime_actor_materialization_for_archived_resume(
    bindings: &meerkat_core::SessionRuntimeBindings,
    authorization: crate::meerkat_machine::ArchivedSessionActorMaterializationAuthorization,
) -> Result<RuntimeActorMaterializationPermit, RuntimeActorMaterializationError> {
    authorization.begin(bindings).await
}

fn begin_session_runtime_actor_materialization_with_phase_policy(
    bindings: &meerkat_core::SessionRuntimeBindings,
    phase_policy: RuntimeActorMaterializationPhasePolicy,
    mutation_guard: Option<crate::tokio::sync::OwnedMutexGuard<()>>,
) -> Result<RuntimeActorMaterializationPermit, RuntimeActorMaterializationError> {
    let authority = validated_session_runtime_bindings_authority(bindings)?;
    let generated_authority = authority
        .dsl_authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    validate_materialization_registration_authority(
        &authority.session_id,
        &authority.epoch_id,
        &authority.teardown_gate,
        &generated_authority,
        phase_policy,
    )?;
    let (claim_id, previous_phase, transactional) = {
        let mut state = authority
            .materialization_claim_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(claim_id) = authority.materialization_claim_id {
            if !state.exact_claim_is(
                claim_id,
                &[
                    RuntimeActorMaterializationClaimPhase::Prepared,
                    RuntimeActorMaterializationClaimPhase::Staged,
                ],
            ) {
                return Err(RuntimeActorMaterializationError::RegistrationClosed);
            }
            let previous = state.phase;
            state.phase = RuntimeActorMaterializationClaimPhase::ActorCreating;
            (claim_id, previous, true)
        } else if authority.legacy_actor_materialization_generation
            == Some(state.legacy_capability_generation)
            && state.current.is_none()
            && state.phase == RuntimeActorMaterializationClaimPhase::Vacant
        {
            let claim_id = uuid::Uuid::new_v4();
            state.current = Some(claim_id);
            state.phase = RuntimeActorMaterializationClaimPhase::ActorCreating;
            (
                claim_id,
                RuntimeActorMaterializationClaimPhase::Vacant,
                false,
            )
        } else {
            return Err(RuntimeActorMaterializationError::RegistrationClosed);
        }
    };
    drop(generated_authority);
    Ok(RuntimeActorMaterializationPermit {
        claim_id,
        claim_state: Arc::clone(&authority.materialization_claim_state),
        session_id: authority.session_id.clone(),
        epoch_id: authority.epoch_id.clone(),
        dsl_authority: Arc::clone(&authority.dsl_authority),
        teardown_gate: Arc::clone(&authority.teardown_gate),
        previous_phase,
        transactional,
        phase_policy,
        _mutation_guard: mutation_guard,
        committed: false,
    })
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
    ContinuationInput, ContinuationKind, ExternalEventInput, FlowStepInput, Input, InputDurability,
    InputHeader, InputOrigin, InputVisibility, OperationInput, PeerConvention, PeerInput,
    PromptInput, ResponseProgressPhase, ResponseTerminalStatus, peer_response_terminal_input,
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
    ArchivedSessionActorMaterializationAuthorization,
    CommittedRuntimeExecutorAttachmentPublicationLease, CommsDrainMode, CommsDrainPhase,
    DrainExitReason, EnsureRuntimeExecutorAttachment, LocalSessionMaterializationMode,
    MachineServiceTurnCommitLease, MachineServiceTurnIdentity, MachineSessionArchiveLease,
    MachineSessionControlAuthority, MeerkatConsumerSurface, MeerkatMachine, PeerIngressOwner,
    PendingRuntimeExecutorAttachment, PreparedArchivedResumeCommitLease,
    PreparedAttachedSessionActorRecovery, PreparedRuntimeExecutorAttachmentRetirement,
    PreparedSessionMaterialization, PromotedArchivedResumeCommitLease, RuntimeBindingsError,
    RuntimeCleanupTaskSpawner, RuntimeExecutorAttachmentRetirementCompletion,
    RuntimeExecutorAttachmentWitness, RuntimeLifecycleFacts, RuntimeLoopQueueAdmissionPlan,
    StandaloneSessionRuntimeAuthorities, classify_runtime_lifecycle_state,
    classify_runtime_loop_queue_admission, standalone_session_runtime_authorities,
    standalone_tool_visibility_owner,
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
    OffDrainResponder, SupervisorBridgeCommandAdmissionRoute,
    SupervisorBridgeCommandClassificationRecord, SupervisorBridgeCommandKind,
    SupervisorBridgeCommandRealization, canonical_meerkat_machine_command_classifications,
    canonical_meerkat_machine_command_input_variant_manifest,
    canonical_meerkat_machine_command_manifest,
    canonical_meerkat_machine_runtime_internal_classifications,
    canonical_meerkat_machine_runtime_internal_fieldless_input_variant_manifest,
    canonical_meerkat_machine_runtime_internal_input_variant_manifest,
    canonical_meerkat_machine_runtime_internal_manifest,
    canonical_supervisor_bridge_command_classifications,
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
                    .await
                    .expect("set silent intents");
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
            InteractionContent::Message { body, .. }
            | InteractionContent::IncarnationFencedMessage { body, .. } => {
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
    // R084: the admitted sender identity comes from the machine-echoed
    // canonical peer id on the classification effect, not the local input.
    let canonical_from_peer_id = admission
        .from_peer_id
        .expect("generated envelope classification should echo the canonical sender peer id");
    let classification = admission.classification;
    let convention = match &interaction.content {
        InteractionContent::Message { .. }
        | InteractionContent::IncarnationFencedMessage { .. } => {
            meerkat_core::PeerIngressConvention::Message
        }
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
        PeerIngressIdentity::new(canonical_from_peer_id, interaction.from.clone(), convention),
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
            continuation_kind: meerkat_machine::dsl::AdmissionContinuationKind::from(
                input.continuation_kind(),
            ),
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
                live_interrupt_required,
                ..
            } => Some(ingress_types::RuntimeInputSemantics {
                boundary: runtime_boundary.into(),
                execution_kind: runtime_execution_kind.into(),
                execution_handling_mode: None,
                peer_response_terminal_apply_intent: runtime_peer_response_terminal_apply_intent
                    .map(Into::into),
                live_interrupt_required,
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
pub use runtime_event::{
    InputLifecycleEvent, RunLifecycleEvent, RuntimeEvent, RuntimeEventEnvelope,
    RuntimeProjectionEvent, RuntimeStateChangeEvent, RuntimeTopologyEvent,
};
pub use runtime_state::{RuntimeState, RuntimeStateTransitionError};
pub use service_ext::SessionServiceRuntimeExt;
pub use store::{InMemoryRuntimeStore, RuntimeStore, RuntimeStoreError, SessionDelta};
pub use traits::{
    DestroyReport, RecoveryReport, RecycleReport, ResetReport, RetireReport, RuntimeControlPlane,
    RuntimeControlPlaneError, RuntimeDriver, RuntimeDriverError,
};
