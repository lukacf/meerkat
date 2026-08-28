//! Mob runtime: actor, builder, handle, and primitives.
//!
//! The mob runtime uses an actor pattern where all mutations (retire, wire,
//! unwire, etc.) are serialized through a single command channel. Spawn
//! provisioning is parallelized, then finalized through the same actor.
//!
//! The public `MobHandle` surface is being consolidated behind one top-level
//! machine command seam. Mutations, event surfaces, and diagnostics are routed
//! through that seam. Canonical member lifecycle projection remains an
//! intentional lock-free read path over shared roster/session truth so
//! `Retiring` and supersession windows stay observable even while disposal work
//! is in flight.

use crate::backend::MobBackendKind;
use crate::build;
use crate::definition::MobDefinition;
use crate::error::MobError;
use crate::event::{MemberRef, MobEventKind, NewMobEvent};
use crate::ids::{
    AgentIdentity, AgentRuntimeId, FenceToken, FlowId, Generation, MobId, ProfileName, RunId,
    WorkOrigin, WorkRef, WorkSpec,
};
use crate::roster::{Roster, RosterEntry};
use crate::run::{FlowRunConfig, MobRun};
use crate::storage::MobStorage;
use crate::store::{MobEventStore, MobRunStore};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use meerkat_client::LlmClient;
use meerkat_core::agent::{AgentToolDispatcher, CommsRuntime as CoreCommsRuntime};
use meerkat_core::comms::{CommsCommand, EventStream, PeerName, StreamError};
use meerkat_core::error::ToolError;
use meerkat_core::service::SessionService;
use meerkat_core::types::{ContentInput, SessionId, ToolCallView, ToolDef, ToolResult};
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use tokio::process::{Child, Command};
use tokio::sync::{RwLock, mpsc, oneshot};

/// Conditional type alias for the runtime adapter.
///
/// When `runtime-adapter` is enabled, this resolves to the concrete
/// `MeerkatMachine` adapter. Otherwise it is a zero-sized unit so callsites
/// that thread this through builder/actor plumbing can compile unconditionally.
#[cfg(feature = "runtime-adapter")]
pub(crate) type RuntimeAdapterOption = Option<Arc<meerkat_runtime::MeerkatMachine>>;
#[cfg(not(feature = "runtime-adapter"))]
pub(crate) type RuntimeAdapterOption = Option<()>;

pub(crate) const FLOW_MEMBER_ID_PREFIX: &str = "__flow_";
pub(crate) const FLOW_SYSTEM_MEMBER_ID_PREFIX: &str = "__flow_system_";

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use actor::IdentityStatusProjectionOrder;

pub(crate) fn flow_system_member_id() -> AgentIdentity {
    crate::ids::AgentIdentity::flow_system_provenance()
}

/// Upper bound between reconciler scans when no wake signal fires. The
/// placed-kickoff, placed-completion, and remote-turn reconcilers are
/// event-driven (machine-state watch, mob event subscription, retry
/// deadlines); this tick only bounds drift from signals those wake sources
/// cannot observe. It must stay slow: the historical fast idle tick made
/// every quiescent scan's state clone / ledger replay an idle-CPU driver on
/// restore-scale mobs.
pub(crate) const RECONCILE_SAFETY_INTERVAL: meerkat_core::time_compat::Duration =
    meerkat_core::time_compat::Duration::from_secs(30);

/// Absolute next-scan deadline for a reconciler loop.
///
/// The deadline is anchored at the last COMPLETED scan ([`Self::rearm`]) and
/// persisted across loop iterations; wakes may only pull it EARLIER
/// ([`Self::pull_earlier`]). Persisting the absolute instant outside the
/// `select!` wait future is load-bearing: `select!` drops and recreates the
/// sleep future on every wake, so a deadline computed inside the wait would
/// reset under sustained wake traffic and the safety scan would starve
/// (the mobkit `ReconcileCadence::next_safety_deadline` lesson).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ReconcileScanDeadline {
    next_scan_at: meerkat_core::time_compat::Instant,
}

impl ReconcileScanDeadline {
    /// Seed the unconditional first scan one debounce interval from now.
    ///
    /// The tokio watch does not fire `changed()` for the value present at
    /// subscribe time, and custody recovered during resume exists at spawn
    /// with no subsequent publish, so the first scan must never wait for a
    /// wake.
    pub(crate) fn first_scan(
        now: meerkat_core::time_compat::Instant,
        debounce: meerkat_core::time_compat::Duration,
    ) -> Self {
        Self {
            next_scan_at: now + debounce,
        }
    }

    /// A wake signal arrived: schedule a scan within `debounce`, without ever
    /// pushing an already-earlier deadline later.
    pub(crate) fn pull_earlier(
        &mut self,
        now: meerkat_core::time_compat::Instant,
        debounce: meerkat_core::time_compat::Duration,
    ) {
        self.next_scan_at = self.next_scan_at.min(now + debounce);
    }

    /// A scan completed: re-anchor the deadline `delay` from now. This is the
    /// ONLY place the deadline may move later.
    pub(crate) fn rearm(
        &mut self,
        now: meerkat_core::time_compat::Instant,
        delay: meerkat_core::time_compat::Duration,
    ) {
        self.next_scan_at = now + delay;
    }

    /// Remaining sleep until the deadline (zero when already due).
    pub(crate) fn sleep_duration(
        &self,
        now: meerkat_core::time_compat::Instant,
    ) -> meerkat_core::time_compat::Duration {
        self.next_scan_at.saturating_duration_since(now)
    }
}

/// Await the next actor-published machine-state change, disabling the watcher
/// permanently once the actor drops the sender.
///
/// A closed watch completes immediately on every poll, so leaving it
/// selectable after `Err` would busy-wake the caller's loop; setting the
/// `Option` to `None` parks the arm on a pending future instead (a destroyed
/// mob must not spin its reconcilers).
pub(crate) async fn wait_for_machine_state_change(
    changes: &mut Option<handle::MobMachineStateChanges>,
) {
    match changes {
        Some(watch) => {
            if watch.changed().await.is_err() {
                *changes = None;
            }
        }
        None => std::future::pending::<()>().await,
    }
}

#[cfg(test)]
mod reconcile_cadence_tests {
    use super::ReconcileScanDeadline;
    use meerkat_core::time_compat::{Duration, Instant};

    const DEBOUNCE: Duration = Duration::from_millis(100);

    #[test]
    fn reconcile_scan_deadline_survives_recreated_waits() {
        let start = Instant::now();
        let mut deadline = ReconcileScanDeadline::first_scan(start, DEBOUNCE);
        let armed = start + DEBOUNCE;
        assert_eq!(deadline.sleep_duration(start), DEBOUNCE);

        // Sustained wake traffic keeps pulling; the deadline must never move
        // later than its current anchor, so the scan fires despite each wake
        // recreating the select! sleep future.
        for wake_ms in [10_u64, 20, 40, 80, 95] {
            let now = start + Duration::from_millis(wake_ms);
            deadline.pull_earlier(now, DEBOUNCE);
            assert!(
                deadline.sleep_duration(start) <= DEBOUNCE,
                "a wake must never postpone the pending scan"
            );
        }
        assert_eq!(
            deadline.sleep_duration(armed),
            Duration::ZERO,
            "the originally-armed deadline still fires under sustained wakes"
        );
    }

    #[test]
    fn reconcile_scan_deadline_rearm_anchors_at_scan_completion() {
        let start = Instant::now();
        let mut deadline = ReconcileScanDeadline::first_scan(start, DEBOUNCE);

        let scan_done = start + Duration::from_millis(250);
        let safety = Duration::from_secs(30);
        deadline.rearm(scan_done, safety);
        assert_eq!(deadline.sleep_duration(scan_done), safety);

        // A wake after re-arm pulls the deadline down to the debounce window.
        let wake = scan_done + Duration::from_secs(1);
        deadline.pull_earlier(wake, DEBOUNCE);
        assert_eq!(deadline.sleep_duration(wake), DEBOUNCE);

        // A later wake cannot push it back out again.
        deadline.pull_earlier(wake + Duration::from_millis(1), safety);
        assert!(deadline.sleep_duration(wake) <= DEBOUNCE);
    }
}

pub(crate) mod actor;
mod actor_turn_executor;
pub mod bridge;
pub mod bridge_protocol;
mod builder;
pub mod composition;
pub mod conditions;
mod delegation;
mod disposal;
mod edge_locks;
mod event_pump;
mod event_router;
mod events;
mod flow;
pub mod flow_frame_engine;
mod handle;
mod identity_local_services;
#[cfg(any(test, feature = "test-support"))]
mod identity_recovery_test_support;
#[cfg(feature = "experimental-gpt-live")]
mod live_bridge_operation;
pub(crate) use handle::MemberTurnLlmIdentityAppliedSender;
#[cfg(any(test, feature = "test-support"))]
pub(crate) use identity_recovery_test_support::trigger_identity_recovery_fail_stop;
#[cfg(any(test, feature = "test-support"))]
pub use identity_recovery_test_support::{
    IdentityRecoveryFailStopPoint, arm_identity_recovery_fail_stop_for_test,
};
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub mod host_actor;
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub mod host_materialize;
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub mod host_observation;
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub(crate) mod host_reply;
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
mod host_schedule;
#[cfg(feature = "runtime-adapter")]
pub mod local_bridge;
// Member-side operator upcall lane (composed by the materializer, D-X2);
// the module body is the upcall lane's deliverable.
mod member_history_proxy;
// Controlling-side live-channel bridge proxy (phase 6b, DEC-P6B-C6). NOTE
// for the lead's consolidated gate edit (ADJ-P6B-17): this module is a
// `BridgeReply` consumer by construction and joins BRIDGE_CLASSIFIER_FILES.
mod member_live_proxy;
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub(crate) mod member_operator_forwarder;
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub mod member_upcall;
mod mob_member_lifecycle_projection;
mod mob_runtime_bridge_authority;
mod mobpack_execution;
mod ops_adapter;
mod panic_capture;
pub mod path;
mod pending_spawn_lineage;
mod placed_carrier_cleanup;
mod provision_guard;
mod provisioner;
pub mod reconcile;
// Flow-spine lane file (remote flow tickets); declared here so the events
// lane's pump can hand pages to `RemoteFlowTicketRegistry` by name (ADJ-P6-10
// cross-lane seams). If the flow wave also adds this line, the lead dedups.
mod placed_completion_reconciler;
mod placed_kickoff_reconciler;
pub mod recovery;
mod remote_flow_ticket;
mod remote_turn_reconciler;
mod roster_authority;
pub(crate) mod scope_gate;
mod session_service;
mod spawn_policy;
mod spawn_profile_authority;
pub mod spec_compiler;
pub mod state;
mod supervisor;
mod supervisor_bridge;
mod terminalization;
mod tools;
pub mod topology;
mod transaction;
pub mod turn_executor;
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub(crate) mod upcall_responder;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests;

#[cfg(feature = "runtime-adapter")]
use actor::MobActor;
#[cfg(feature = "runtime-adapter")]
use actor_turn_executor::ActorFlowTurnExecutor;
use flow::FlowEngine;
#[cfg(feature = "runtime-adapter")]
use provisioner::MultiBackendProvisioner;
use provisioner::{MobProvisioner, ProvisionMemberRequest};
use state::MobCommand;
use tools::compose_external_tools_for_profile;

pub use crate::roster::{MobMemberKickoffPhase, MobMemberKickoffSnapshot};
pub use builder::MobBuilder;
pub use builder::{
    ControllingAcceptorConfig, LocalMemberAcceptorMaterialSource, MemberAcceptorRegistration,
};
pub use delegation::{
    BOUNDED_DELEGATION_REPORT_INSTRUCTION_V1, DelegationCancellationHandle,
    DelegationExecutionError, DelegationExecutionHandle, DelegationExecutionOutcome,
    DelegationExecutionRequest, DelegationExecutionService, DelegationExecutionSource,
    DelegationMemberOptions, DelegationParentContext, DelegationTerminalizedExecution,
    DelegationTurnTerminal, LiveDelegationTerminalEvidence, render_bounded_delegation_task,
};
pub use event_router::{MobEventRouterConfig, MobEventRouterHandle};
pub use flow_frame_engine::{FlowFrameKernel, FlowFrameMutator};
pub use handle::{
    AdaptiveDriverCapability, AdaptiveLayerAdmission, AdaptiveLayerAdmissionRequest,
    AdaptiveLayerAttempt, AdaptiveLayerDisposition, AdaptiveLayerPhaseView,
    AdaptiveLayerResultDigest, AdaptiveLayerRetention, AdaptiveLayerRunStart,
    AdaptiveLayerSetupFault, AdaptiveLayerSetupFaultObservation, AdaptiveLayerSnapshot,
    AdaptivePlanningDecisionKind, AdaptiveRunLimits, AdaptiveRunPhaseView, AdaptiveRunSnapshot,
    AdaptiveStopReasonView, BoundedFlowResult, BoundedHelperResult, BoundedHelperResultStatus,
    BoundedHelperRunOutcome, BoundedMemberRunError, BoundedResultSpec, BoundedTurnFailure,
    BoundedTurnResult, BoundedTurnWaitError, CurrentMobAdmission,
    DEFAULT_BOUNDED_HELPER_RESULT_BYTES, DurableBoundedMemberState, DurableBoundedWorkRecovery,
    DurableBoundedWorkState, ExternalMemberBindingMode, ExternalMemberForwardingHookRef,
    ExternalMemberForwardingHooks, ExternalMemberForwardingStatus,
    ExternalMemberObservationSnapshot, ExternalMemberOwnerRef, ExternalMemberReachability,
    ExternalMemberRebindStatus, ExternalPeerBindingSpec, FlowRunHandle, FlowRunWaitError,
    FlowTargetProvisioner, ForkMemberBoundedRunOutcome, ForkMemberResult,
    HELPER_RESULT_TRUNCATION_MARKER, HelperOptions, HelperResult, HostBindReport, HostBindRequest,
    HostCapabilityReport, HostRevokeReport, InitializeAdaptiveRunRequest, MemberBoundedTurnResult,
    MemberDeliveryReceipt, MemberHandle, MemberRespawnReceipt, MemberTurnEventSender,
    MemberTurnHandle, MemberTurnOptions, MobDestroyError, MobDestroyReport, MobEventsSubscription,
    MobEventsSubscriptionConfig, MobEventsView, MobHandle, MobMachineStateChanges,
    MobMemberListEntry, MobMemberSnapshot, MobMemberStatus, MobPeerConnectivitySnapshot,
    MobRespawnError, MobSpawnManyFailure, MobUnreachablePeer, MobWireMembersBatchReport,
    PeerMessageReceipt, PeerTarget, PreviousMemberCleanupReport, SpawnContinuityIntent,
    SpawnCustomizationContext, SpawnMemberAdmission, SpawnMemberAdmissionObservations,
    SpawnMemberCustomizer, SpawnMemberSpec, SpawnResult, SpawnSource, SpawnSystemPromptOverride,
    SpawnToolAdmission, SupervisorRotationReport, WorkBoundedTurnResult, WorkDeliveryReceipt,
    WorkTurnHandle, mob_error_wire_code, profile_to_wire, stored_realm_profile_to_wire,
};
pub(crate) use handle::{
    CanonicalOpsOwnerContext, ExactTurnCompletionSender, FlowOperationCustody, MemberSpawnReceipt,
};
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub use host_schedule::HostObservationScheduleMobHost;
pub use identity_local_services::{
    IdentityLocalExternalToolsError, IdentityLocalExternalToolsProvider,
    IdentityLocalMaterializationKey,
};
#[cfg(feature = "experimental-gpt-live")]
pub use live_bridge_operation::{
    DEFAULT_LIVE_BRIDGE_OUTPUT_BYTES, DurableMemberLiveBridgeOperationExecutor,
    LiveBridgeAcceptedExecution, LiveBridgeExecutionSnapshot,
    LiveBridgeOperationCancellationHandle, LiveBridgeOperationCancellationSignal,
    LiveBridgeOperationExecutor, LiveBridgeOperationRequest, LiveBridgeOperationService,
    LiveBridgeOperationStartError, LiveBridgeOperationTerminal, LiveBridgeOperationTerminalError,
    LiveBridgeOperationTerminalFuture,
};
pub use member_history_proxy::MemberHistoryPageDomain;
pub use member_live_proxy::MemberLiveStatusDomain;
#[cfg(feature = "runtime-adapter")]
pub use mobpack_execution::run_mobpack_callable;
pub use mobpack_execution::{MobpackCallableConfig, MobpackRunOutcome, MobpackRunSpec};
use pending_spawn_lineage::{PendingSpawnInsertImpact, PendingSpawnLineage};
pub use reconcile::{
    EnsureMemberOutcome, MemberFilter, ReconcileFailure, ReconcileOptions, ReconcileReport,
    ReconcileStage,
};
pub use recovery::RestoreIncompatible;
use roster_authority::{RosterAuthority, RosterMutator};
pub use session_service::{
    AuthorizedSessionResume, MobSessionService, PersistedSessionAuthorityReadCost,
    ResumeRejectionKind, ResumeSessionLoad, ResumeVerdictTerminality, SessionResumeAuthority,
    SessionResumeLifecycle, SessionResumeMaterialization, SessionResumePreparationReceipt,
    SessionResumeRejection, SessionResumeVerdict, materialize_nonpersistent_session_resume_verdict,
};
pub use spawn_policy::{SpawnPolicy, SpawnSpec};
use spawn_profile_authority::{
    AuthorizedSpawnProfileMaterial, authorize_spawn_profile_input,
    authorize_spawn_profile_material, require_authorized_effect,
};
#[cfg(not(target_arch = "wasm32"))]
pub use spec_compiler::FactoryChainSpawnBasePromptSource;
pub use spec_compiler::{SpawnBasePromptSource, StaticSpawnBasePromptSource};
#[cfg(test)]
pub(crate) use state::MobDslT2Snapshot;
#[cfg(test)]
pub(crate) use state::MobLifecycleSnapshot;
pub use state::MobOrchestratorSnapshot;
pub use state::MobState;
pub(crate) use supervisor_bridge::MobSupervisorBridge;
pub use turn_executor::{
    FlowTurnExecutor, FlowTurnFailureDisposition, FlowTurnOutcome, FlowTurnTicket,
    TimeoutDisposition,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct MobFlowTrackerSnapshot {
    pub run_task_ids: BTreeSet<RunId>,
    pub cancel_token_ids: BTreeSet<RunId>,
    pub stream_ids: BTreeSet<RunId>,
    pub tracked_flows: BTreeMap<RunId, FlowId>,
}

/// Placement-first runtime-owner predicate.
///
/// A host-materialized member may carry a `BackendPeer(Some(session_id))`, but
/// that session id belongs to the member host. Callers must consult this
/// machine-owned fact before selecting any provisioner API whose generic
/// `Some(session_id)` arm addresses the controller-local session backend.
pub(crate) fn member_runtime_is_host_owned(
    state: &crate::machines::mob_machine::MobMachineState,
    identity: &AgentIdentity,
) -> bool {
    state
        .member_placement
        .contains_key(&crate::machines::mob_machine::AgentIdentity::from_domain(
            identity,
        ))
}

/// Single recovery predicate for a member edge's trust intent. A durable
/// retirement-start marker owns teardown for the whole incident edge, so both
/// local trust repair and remote route derivation must suppress it.
pub(crate) fn recovery_member_edge_trust_is_desired(
    state: &crate::machines::mob_machine::MobMachineState,
    edge: &crate::machines::mob_machine::WiringEdge,
) -> bool {
    use crate::machines::mob_machine as mob_dsl;

    [&edge.a, &edge.b].iter().all(|identity| {
        state
            .identity_to_runtime
            .get(*identity)
            .and_then(|runtime_id| state.member_state_markers.get(runtime_id))
            != Some(&mob_dsl::MobMemberState::Retiring)
    })
}

/// Single owner of the §6.2 route-install derivation rule (multi-host mobs
/// ADJ-P4-1): every eligible wired edge endpoint placed on a BOUND host yields
/// an `Install` obligation. Called from controlling recovery (bare authority,
/// before the actor exists) and from the actor's rebind/drive re-derivation
/// (optionally host-scoped) — one rule, two call sites, zero drift.
pub(crate) fn derive_install_obligations(
    state: &crate::machines::mob_machine::MobMachineState,
    host_filter: Option<&crate::machines::mob_machine::HostId>,
) -> BTreeSet<crate::machines::mob_machine::RouteInstallObligation> {
    use crate::machines::mob_machine as mob_dsl;

    let mut derived = BTreeSet::new();
    for edge in &state.wiring_edges {
        if !recovery_member_edge_trust_is_desired(state, edge) {
            // A durable Started carrier makes teardown, not trust repair, the
            // recovery intent. Reinstalling a route for an incident edge can
            // resurrect stale trust just before retirement unwinds it.
            continue;
        }
        for endpoint in [&edge.a, &edge.b] {
            let Some(host) = state.member_placement.get(endpoint) else {
                continue;
            };
            if let Some(filter) = host_filter
                && filter != host
            {
                continue;
            }
            if state.host_bind_phase.get(host) != Some(&mob_dsl::HostBindPhase::Bound) {
                continue;
            }
            derived.insert(mob_dsl::RouteInstallObligation {
                edge: edge.clone(),
                host: host.clone(),
                kind: mob_dsl::RouteObligationKind::Install,
            });
        }
    }
    derived
}
