//! Authority adapter for the MeerkatMachine DSL.
//!
//! Bridges between the runtime's distributed state and the DSL's flat
//! state representation. The DSL validates transition legality; the
//! runtime shell executes the actual mutations.

use std::collections::BTreeSet;

use super::dsl as mm_dsl;
use crate::identifiers::LogicalRuntimeId;
use crate::runtime_state::RuntimeState;
use meerkat_core::lifecycle::RunId;
use meerkat_core::types::SessionId;

/// Map DSL transition errors into plain strings with context.
pub(crate) fn map_error(err: mm_dsl::MeerkatMachineTransitionError, context: &str) -> String {
    match err {
        mm_dsl::MeerkatMachineTransitionError::NoMatchingTransition { phase, trigger } => {
            format!("DSL authority ({context}): no matching transition from {phase} for {trigger}")
        }
        mm_dsl::MeerkatMachineTransitionError::GuardRejected { phase, trigger } => {
            format!(
                "DSL authority ({context}): guard rejected transition from {phase} for {trigger}"
            )
        }
    }
}

pub(crate) fn write_back_phase(dsl_phase: mm_dsl::MeerkatPhase) -> RuntimeState {
    match dsl_phase {
        mm_dsl::MeerkatPhase::Initializing => RuntimeState::Initializing,
        mm_dsl::MeerkatPhase::Idle => RuntimeState::Idle,
        mm_dsl::MeerkatPhase::Attached => RuntimeState::Attached,
        mm_dsl::MeerkatPhase::Running => RuntimeState::Running,
        mm_dsl::MeerkatPhase::Retired => RuntimeState::Retired,
        mm_dsl::MeerkatPhase::Stopped => RuntimeState::Stopped,
        mm_dsl::MeerkatPhase::Destroyed => RuntimeState::Destroyed,
    }
}

pub(crate) fn current_run_id_from_dsl(run_id: &mm_dsl::RunId) -> Option<RunId> {
    uuid::Uuid::parse_str(&run_id.0).ok().map(RunId::from_uuid)
}

pub(crate) fn pre_run_phase_to_runtime_state(phase: mm_dsl::PreRunPhase) -> RuntimeState {
    match phase {
        mm_dsl::PreRunPhase::Idle => RuntimeState::Idle,
        mm_dsl::PreRunPhase::Attached => RuntimeState::Attached,
        mm_dsl::PreRunPhase::Retired => RuntimeState::Retired,
    }
}

pub(crate) fn runtime_phase_from_authority(
    authority: &mm_dsl::MeerkatMachineAuthority,
) -> RuntimeState {
    write_back_phase(authority.state.lifecycle_phase)
}

pub(crate) fn visible_runtime_phase_from_authority(
    authority: &mm_dsl::MeerkatMachineAuthority,
) -> RuntimeState {
    if authority.state.lifecycle_phase == mm_dsl::MeerkatPhase::Running
        && authority.state.pre_run_phase == Some(mm_dsl::PreRunPhase::Retired)
    {
        RuntimeState::Retired
    } else {
        runtime_phase_from_authority(authority)
    }
}

pub(crate) fn current_run_id_from_authority(
    authority: &mm_dsl::MeerkatMachineAuthority,
) -> Option<RunId> {
    authority
        .state
        .current_run_id
        .as_ref()
        .and_then(current_run_id_from_dsl)
}

pub(crate) fn pre_run_phase_from_authority(
    authority: &mm_dsl::MeerkatMachineAuthority,
) -> Option<RuntimeState> {
    authority
        .state
        .pre_run_phase
        .map(pre_run_phase_to_runtime_state)
}

pub(crate) fn project_phase(state: RuntimeState) -> mm_dsl::MeerkatPhase {
    match state {
        RuntimeState::Initializing => mm_dsl::MeerkatPhase::Initializing,
        RuntimeState::Idle => mm_dsl::MeerkatPhase::Idle,
        RuntimeState::Attached => mm_dsl::MeerkatPhase::Attached,
        RuntimeState::Running => mm_dsl::MeerkatPhase::Running,
        RuntimeState::Retired => mm_dsl::MeerkatPhase::Retired,
        RuntimeState::Stopped => mm_dsl::MeerkatPhase::Stopped,
        RuntimeState::Destroyed => mm_dsl::MeerkatPhase::Destroyed,
    }
}

pub(crate) fn project_state(
    session_id: &SessionId,
    runtime_phase: RuntimeState,
    runtime_id: Option<&LogicalRuntimeId>,
    current_run_id: Option<&RunId>,
    pre_run_phase: Option<RuntimeState>,
    silent_intent_overrides: BTreeSet<String>,
    active_fence_token: Option<u64>,
) -> mm_dsl::MeerkatMachineState {
    let (effective_phase, effective_current_run_id, effective_pre_run_phase) =
        match (runtime_phase, current_run_id, pre_run_phase) {
            (RuntimeState::Running, None, pre_run_phase) => (
                crate::runtime_state::run_return_phase_from_pre_run_phase(pre_run_phase),
                None,
                None,
            ),
            (RuntimeState::Running | RuntimeState::Retired, current_run_id, pre_run_phase) => {
                (runtime_phase, current_run_id, pre_run_phase)
            }
            (phase, _, _) => (phase, None, None),
        };

    mm_dsl::MeerkatMachineState {
        lifecycle_phase: project_phase(effective_phase),
        session_id: Some(mm_dsl::SessionId::from_domain(session_id)),
        active_runtime_id: runtime_id.map(mm_dsl::AgentRuntimeId::from_domain),
        active_fence_token: active_fence_token.map(mm_dsl::FenceToken::from),
        current_run_id: effective_current_run_id.map(mm_dsl::RunId::from_domain),
        pre_run_phase: effective_pre_run_phase.and_then(pre_run_phase_from_runtime_state),
        turn_phase: super::dsl::TurnPhase::Ready,
        primitive_kind: None,
        admitted_content_shape: None,
        vision_enabled: false,
        image_tool_results_enabled: false,
        tool_calls_pending: 0,
        pending_op_refs: std::collections::BTreeSet::new(),
        barrier_operation_ids: std::collections::BTreeSet::new(),
        has_barrier_ops: false,
        barrier_satisfied: false,
        boundary_count: 0,
        cancel_after_boundary: false,
        terminal_outcome: None,
        extraction_attempts: 0,
        max_extraction_retries: 0,
        silent_intent_overrides,
        // Absorbed substate fields — initialised to DSL defaults.
        // These are projected from their respective authority owners
        // during the Phase 3 cutover; until then they carry defaults.
        registration_phase: super::dsl::RegistrationPhase::Queuing,
        drain_phase: super::dsl::DrainPhase::Inactive,
        drain_mode: None,
        next_staged_visibility_revision: 0,
        active_filter: super::dsl::ToolFilter::All,
        staged_filter: super::dsl::ToolFilter::All,
        active_visibility_revision: 0,
        staged_visibility_revision: 0,
        active_deferred_names: std::collections::BTreeSet::new(),
        staged_deferred_names: std::collections::BTreeSet::new(),
        input_phases: std::collections::BTreeMap::new(),
        input_terminal_kind: std::collections::BTreeMap::new(),
        input_superseded_by: std::collections::BTreeMap::new(),
        input_aggregate_id: std::collections::BTreeMap::new(),
        input_abandon_reason: std::collections::BTreeMap::new(),
        input_abandon_attempt_count: std::collections::BTreeMap::new(),
        input_attempt_counts: std::collections::BTreeMap::new(),
        input_run_associations: std::collections::BTreeMap::new(),
        input_boundary_sequences: std::collections::BTreeMap::new(),
        next_admission_seq: 0,
        input_admission_seq: std::collections::BTreeMap::new(),
        input_lane: std::collections::BTreeMap::new(),
        op_statuses: std::collections::BTreeMap::new(),
        op_completion_seq: std::collections::BTreeMap::new(),
        op_terminal_outcomes: std::collections::BTreeMap::new(),
        op_terminal_payload: std::collections::BTreeMap::new(),
        op_kinds: std::collections::BTreeMap::new(),
        op_peer_ready: std::collections::BTreeMap::new(),
        op_progress_counts: std::collections::BTreeMap::new(),
        active_op_count: 0,
        wait_active: false,
        wait_operation_ids: std::collections::BTreeSet::new(),
        next_completion_seq: 0,
        known_surfaces: std::collections::BTreeSet::new(),
        visible_surfaces: std::collections::BTreeSet::new(),
        surface_base_state: std::collections::BTreeMap::new(),
        surface_pending_op: std::collections::BTreeMap::new(),
        surface_staged_op: std::collections::BTreeMap::new(),
        surface_staged_intent_sequence: std::collections::BTreeMap::new(),
        next_staged_intent_sequence: 0,
        surface_pending_task_sequence: std::collections::BTreeMap::new(),
        next_pending_task_sequence: 0,
        surface_pending_lineage_sequence: std::collections::BTreeMap::new(),
        surface_inflight_calls: std::collections::BTreeMap::new(),
        surface_last_delta_operation: std::collections::BTreeMap::new(),
        surface_last_delta_phase: std::collections::BTreeMap::new(),
        snapshot_epoch: 0,
        snapshot_aligned_epoch: 0,
        surface_draining_since_ms: std::collections::BTreeMap::new(),
        surface_removal_timeout_at_ms: std::collections::BTreeMap::new(),
        surface_removal_applied_at_turn: std::collections::BTreeMap::new(),
        surface_phase: super::dsl::SurfacePhase::Operating,
        removal_timeout_ms: 30000,
        realtime_intent_present: false,
        realtime_binding_state: super::dsl::RealtimeBindingState::Unbound,
        realtime_binding_authority_epoch: None,
        realtime_reattach_required: false,
        realtime_next_authority_epoch: 1,
        realtime_reconnect_attempt_count: 0,
        realtime_reconnect_next_retry_at_ms: None,
        realtime_reconnect_deadline_at_ms: None,
        live_topology_phase: super::dsl::LiveTopologyPhase::Idle,
        mcp_server_states: std::collections::BTreeMap::new(),
        pending_peer_requests: std::collections::BTreeMap::new(),
        inbound_peer_requests: std::collections::BTreeMap::new(),
        last_session_context_updated_at_ms: 0,
        reserved_interaction_streams: std::collections::BTreeSet::new(),
        attached_interaction_streams: std::collections::BTreeSet::new(),
        realtime_product_turn_phase: mm_dsl::RealtimeProductTurnPhase::Idle,
        realtime_projection_freshness: mm_dsl::RealtimeProjectionFreshness::Clean,
        realtime_projection_frontier_ms: 0,
        realtime_reconnect_policy: mm_dsl::RealtimeReconnectPolicy::CleanExit,
        peer_ingress_owner_kind: super::dsl::PeerIngressOwnerKind::Unattached,
        peer_ingress_comms_runtime_id: None,
        peer_ingress_mob_id: None,
        supervisor_binding_kind: super::dsl::SupervisorBindingKind::Unbound,
        supervisor_bound_name: None,
        supervisor_bound_peer_id: None,
        supervisor_bound_address: None,
        supervisor_bound_epoch: None,
        // Track-B (R5): peer-projection state — initialised empty in
        // the authority projection. The real values flow through the
        // DSL state machine as `PublishLocalEndpoint` /
        // `AddDirectPeerEndpoint` / `ApplyMobPeerOverlay` transitions
        // are applied.
        local_endpoint: None,
        direct_peer_endpoints: std::collections::BTreeSet::new(),
        mob_overlay_peer_endpoints: std::collections::BTreeSet::new(),
        peer_projection_epoch: 0,
        mob_overlay_epoch: 0,
    }
}

/// Map a persisted pre-run phase (as a [`RuntimeState`]) into the typed
/// [`mm_dsl::PreRunPhase`] carried in the DSL state. Only `Idle`, `Attached`,
/// and `Retired` are valid pre-run markers — any other [`RuntimeState`]
/// indicates the session is not in a run-in-progress shape and the DSL
/// treats the pre-run slot as absent.
pub(crate) fn pre_run_phase_from_runtime_state(state: RuntimeState) -> Option<mm_dsl::PreRunPhase> {
    match state {
        RuntimeState::Idle => Some(mm_dsl::PreRunPhase::Idle),
        RuntimeState::Attached => Some(mm_dsl::PreRunPhase::Attached),
        RuntimeState::Retired => Some(mm_dsl::PreRunPhase::Retired),
        RuntimeState::Initializing
        | RuntimeState::Running
        | RuntimeState::Stopped
        | RuntimeState::Destroyed => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_and_write_back_round_trips() {
        for state in [
            RuntimeState::Initializing,
            RuntimeState::Idle,
            RuntimeState::Attached,
            RuntimeState::Running,
            RuntimeState::Retired,
            RuntimeState::Stopped,
            RuntimeState::Destroyed,
        ] {
            let dsl = project_phase(state);
            let back = write_back_phase(dsl);
            assert_eq!(back, state);
        }
    }

    #[test]
    fn map_error_includes_context() {
        let err = mm_dsl::MeerkatMachineTransitionError::NoMatchingTransition {
            phase: "Idle".into(),
            trigger: "Destroy".into(),
        };
        let msg = map_error(err, "test_context");
        assert!(msg.contains("test_context"));
        assert!(msg.contains("Idle"));
    }
}
