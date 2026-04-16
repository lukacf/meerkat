//! Authority adapter for the MobMachine DSL.
//!
//! This module bridges between the actor's distributed state and the DSL's
//! flat state representation. The DSL validates transition legality; the actor
//! shell executes the actual mutations.
//!
//! Pattern: project → apply → write_back → handle effects.

use crate::error::MobError;
use crate::ids;
use crate::machines::mob_machine as mob_dsl;
use crate::runtime::MobState;

use std::sync::atomic::Ordering;

// ---------------------------------------------------------------------------
// Transition result returned to the actor
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) struct MobTransitionResult {
    pub new_phase: MobPhaseResult,
    pub effects: Vec<mob_dsl::MobMachineEffect>,
}

/// Phase after a transition, mapped back to the domain `MobState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MobPhaseResult {
    Running,
    Stopped,
    Completed,
    Destroyed,
}

impl MobPhaseResult {
    pub fn to_mob_state(self) -> MobState {
        match self {
            Self::Running => MobState::Running,
            Self::Stopped => MobState::Stopped,
            Self::Completed => MobState::Completed,
            Self::Destroyed => MobState::Destroyed,
        }
    }

    pub fn to_dsl_phase(self) -> mob_dsl::MobPhase {
        match self {
            Self::Running => mob_dsl::MobPhase::Running,
            Self::Stopped => mob_dsl::MobPhase::Stopped,
            Self::Completed => mob_dsl::MobPhase::Completed,
            Self::Destroyed => mob_dsl::MobPhase::Destroyed,
        }
    }
}

pub(crate) fn dsl_phase_to_result(phase: mob_dsl::MobPhase) -> MobPhaseResult {
    match phase {
        mob_dsl::MobPhase::Running => MobPhaseResult::Running,
        mob_dsl::MobPhase::Stopped => MobPhaseResult::Stopped,
        mob_dsl::MobPhase::Completed => MobPhaseResult::Completed,
        mob_dsl::MobPhase::Destroyed => MobPhaseResult::Destroyed,
    }
}

// ---------------------------------------------------------------------------
// Projection: actor state → DSL flat state
// ---------------------------------------------------------------------------

/// Project the actor's distributed state into the DSL flat state struct.
///
/// This reads from the actor's shared state (roster, run tracking, etc.) to
/// construct the 7-field DSL state. The actor loop is sequential, so the
/// projection is always consistent.
pub(crate) fn project_mob_state(
    phase: MobState,
    live_runtime_ids: &std::collections::BTreeSet<ids::AgentRuntimeId>,
    externally_addressable_ids: &std::collections::BTreeSet<ids::AgentRuntimeId>,
    fence_tokens: &std::collections::BTreeMap<ids::AgentRuntimeId, ids::FenceToken>,
    active_run_count: u64,
    pending_spawn_count: u64,
    coordinator_bound: bool,
) -> mob_dsl::MobMachineState {
    mob_dsl::MobMachineState {
        lifecycle_phase: match phase {
            MobState::Running | MobState::Creating => mob_dsl::MobPhase::Running,
            MobState::Stopped => mob_dsl::MobPhase::Stopped,
            MobState::Completed => mob_dsl::MobPhase::Completed,
            MobState::Destroyed => mob_dsl::MobPhase::Destroyed,
        },
        live_runtime_ids: live_runtime_ids
            .iter()
            .map(mob_dsl::AgentRuntimeId::from_domain)
            .collect(),
        externally_addressable_runtime_ids: externally_addressable_ids
            .iter()
            .map(mob_dsl::AgentRuntimeId::from_domain)
            .collect(),
        runtime_fence_tokens: fence_tokens
            .iter()
            .map(|(rid, ft)| {
                (
                    mob_dsl::AgentRuntimeId::from_domain(rid),
                    mob_dsl::FenceToken::from_domain(*ft),
                )
            })
            .collect(),
        active_run_count,
        pending_spawn_count,
        coordinator_bound,
    }
}

// ---------------------------------------------------------------------------
// Input conversion
// ---------------------------------------------------------------------------

/// Inputs that the actor shell can submit to the DSL authority.
///
/// These carry real domain types. The adapter converts them to DSL inputs.
#[derive(Debug)]
pub(crate) enum MobAuthorityInput {
    RunFlow,
    CancelFlow,
    Spawn {
        identity: ids::AgentIdentity,
        runtime_id: ids::AgentRuntimeId,
        fence_token: ids::FenceToken,
        generation: ids::Generation,
        external_addressable: bool,
    },
    Retire {
        runtime_id: ids::AgentRuntimeId,
    },
    Respawn {
        runtime_id: ids::AgentRuntimeId,
    },
    RetireAll,
    Wire,
    Unwire,
    ExternalTurn,
    InternalTurn,
    SubmitWork {
        runtime_id: ids::AgentRuntimeId,
        fence_token: ids::FenceToken,
        work_ref: ids::WorkRef,
        origin: ids::WorkOrigin,
    },
    CancelAllWork {
        runtime_id: ids::AgentRuntimeId,
        fence_token: ids::FenceToken,
    },
    Stop,
    Resume,
    Complete,
    Reset,
    Destroy,
    TaskCreate,
    TaskUpdate,
    RecordOperatorActionProvenance,
    SetSpawnPolicy,
    Shutdown,
    ForceCancel,
    SubscribeAgentEvents,
    SubscribeAllAgentEvents,
    SubscribeMobEvents,
}

pub(crate) fn convert_input(input: &MobAuthorityInput) -> mob_dsl::MobMachineInput {
    match input {
        MobAuthorityInput::RunFlow => mob_dsl::MobMachineInput::RunFlow,
        MobAuthorityInput::CancelFlow => mob_dsl::MobMachineInput::CancelFlow,
        MobAuthorityInput::Spawn {
            identity,
            runtime_id,
            fence_token,
            generation,
            external_addressable,
        } => mob_dsl::MobMachineInput::Spawn {
            agent_identity: mob_dsl::AgentIdentity::from_domain(identity),
            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(runtime_id),
            fence_token: mob_dsl::FenceToken::from_domain(*fence_token),
            generation: mob_dsl::Generation::from_domain(*generation),
            external_addressable: *external_addressable,
        },
        MobAuthorityInput::Retire { runtime_id } => mob_dsl::MobMachineInput::Retire {
            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(runtime_id),
        },
        MobAuthorityInput::Respawn { runtime_id } => mob_dsl::MobMachineInput::Respawn {
            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(runtime_id),
        },
        MobAuthorityInput::RetireAll => mob_dsl::MobMachineInput::RetireAll,
        MobAuthorityInput::Wire => mob_dsl::MobMachineInput::Wire,
        MobAuthorityInput::Unwire => mob_dsl::MobMachineInput::Unwire,
        MobAuthorityInput::ExternalTurn => mob_dsl::MobMachineInput::ExternalTurn,
        MobAuthorityInput::InternalTurn => mob_dsl::MobMachineInput::InternalTurn,
        MobAuthorityInput::SubmitWork {
            runtime_id,
            fence_token,
            work_ref,
            origin,
        } => mob_dsl::MobMachineInput::SubmitWork {
            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(runtime_id),
            fence_token: mob_dsl::FenceToken::from_domain(*fence_token),
            work_id: mob_dsl::WorkId::from_work_ref(work_ref),
            origin: match origin {
                ids::WorkOrigin::External => "External".to_owned(),
                ids::WorkOrigin::Internal => "Internal".to_owned(),
            },
        },
        MobAuthorityInput::CancelAllWork {
            runtime_id,
            fence_token,
        } => mob_dsl::MobMachineInput::CancelAllWork {
            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(runtime_id),
            fence_token: mob_dsl::FenceToken::from_domain(*fence_token),
        },
        MobAuthorityInput::Stop => mob_dsl::MobMachineInput::Stop,
        MobAuthorityInput::Resume => mob_dsl::MobMachineInput::Resume,
        MobAuthorityInput::Complete => mob_dsl::MobMachineInput::Complete,
        MobAuthorityInput::Reset => mob_dsl::MobMachineInput::Reset,
        MobAuthorityInput::Destroy => mob_dsl::MobMachineInput::Destroy,
        MobAuthorityInput::TaskCreate => mob_dsl::MobMachineInput::TaskCreate,
        MobAuthorityInput::TaskUpdate => mob_dsl::MobMachineInput::TaskUpdate,
        MobAuthorityInput::RecordOperatorActionProvenance => {
            mob_dsl::MobMachineInput::RecordOperatorActionProvenance
        }
        MobAuthorityInput::SetSpawnPolicy => mob_dsl::MobMachineInput::SetSpawnPolicy,
        MobAuthorityInput::Shutdown => mob_dsl::MobMachineInput::Shutdown,
        MobAuthorityInput::ForceCancel => mob_dsl::MobMachineInput::ForceCancel,
        MobAuthorityInput::SubscribeAgentEvents => mob_dsl::MobMachineInput::SubscribeAgentEvents,
        MobAuthorityInput::SubscribeAllAgentEvents => {
            mob_dsl::MobMachineInput::SubscribeAllAgentEvents
        }
        MobAuthorityInput::SubscribeMobEvents => mob_dsl::MobMachineInput::SubscribeMobEvents,
    }
}

// ---------------------------------------------------------------------------
// Signal conversion
// ---------------------------------------------------------------------------

/// Signals that the actor shell can submit to the DSL authority.
///
/// These are internal events (not from external commands).
#[derive(Debug)]
pub(crate) enum MobAuthoritySignal {
    ObserveRuntimeReady {
        runtime_id: ids::AgentRuntimeId,
        fence_token: ids::FenceToken,
    },
    RetireMember {
        runtime_id: ids::AgentRuntimeId,
        fence_token: ids::FenceToken,
    },
    ObserveRuntimeRetired {
        runtime_id: ids::AgentRuntimeId,
        fence_token: ids::FenceToken,
    },
    ResetMember {
        identity: ids::AgentIdentity,
        runtime_id: ids::AgentRuntimeId,
        fence_token: ids::FenceToken,
        generation: ids::Generation,
        external_addressable: bool,
    },
    RespawnMember {
        identity: ids::AgentIdentity,
        runtime_id: ids::AgentRuntimeId,
        fence_token: ids::FenceToken,
        generation: ids::Generation,
        external_addressable: bool,
    },
    DestroyMob,
    ObserveRuntimeDestroyed {
        runtime_id: ids::AgentRuntimeId,
        fence_token: ids::FenceToken,
    },
    MarkCompleted,
    StartRun,
    FinishRun,
    BeginCleanup,
    FinishCleanup,
    InitializeOrchestrator,
    BindCoordinator,
    UnbindCoordinator,
    StageSpawn,
    CompleteSpawn,
    StartFlow,
    CompleteFlow,
    StopOrchestrator,
    ResumeOrchestrator,
    DestroyOrchestrator,
    ForceCancelMember,
    MemberPeerExposed,
    MemberTerminalized,
    OperationPeerTrusted,
    PeerInputAdmitted,
    CreateRun,
}

pub(crate) fn convert_signal(signal: &MobAuthoritySignal) -> mob_dsl::MobMachineSignal {
    match signal {
        MobAuthoritySignal::ObserveRuntimeReady {
            runtime_id,
            fence_token,
        } => mob_dsl::MobMachineSignal::ObserveRuntimeReady {
            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(runtime_id),
            fence_token: mob_dsl::FenceToken::from_domain(*fence_token),
        },
        MobAuthoritySignal::RetireMember {
            runtime_id,
            fence_token,
        } => mob_dsl::MobMachineSignal::RetireMember {
            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(runtime_id),
            fence_token: mob_dsl::FenceToken::from_domain(*fence_token),
        },
        MobAuthoritySignal::ObserveRuntimeRetired {
            runtime_id,
            fence_token,
        } => mob_dsl::MobMachineSignal::ObserveRuntimeRetired {
            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(runtime_id),
            fence_token: mob_dsl::FenceToken::from_domain(*fence_token),
        },
        MobAuthoritySignal::ResetMember {
            identity,
            runtime_id,
            fence_token,
            generation,
            external_addressable,
        } => mob_dsl::MobMachineSignal::ResetMember {
            agent_identity: mob_dsl::AgentIdentity::from_domain(identity),
            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(runtime_id),
            fence_token: mob_dsl::FenceToken::from_domain(*fence_token),
            generation: mob_dsl::Generation::from_domain(*generation),
            external_addressable: *external_addressable,
        },
        MobAuthoritySignal::RespawnMember {
            identity,
            runtime_id,
            fence_token,
            generation,
            external_addressable,
        } => mob_dsl::MobMachineSignal::RespawnMember {
            agent_identity: mob_dsl::AgentIdentity::from_domain(identity),
            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(runtime_id),
            fence_token: mob_dsl::FenceToken::from_domain(*fence_token),
            generation: mob_dsl::Generation::from_domain(*generation),
            external_addressable: *external_addressable,
        },
        MobAuthoritySignal::DestroyMob => mob_dsl::MobMachineSignal::DestroyMob,
        MobAuthoritySignal::ObserveRuntimeDestroyed {
            runtime_id,
            fence_token,
        } => mob_dsl::MobMachineSignal::ObserveRuntimeDestroyed {
            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(runtime_id),
            fence_token: mob_dsl::FenceToken::from_domain(*fence_token),
        },
        MobAuthoritySignal::MarkCompleted => mob_dsl::MobMachineSignal::MarkCompleted,
        MobAuthoritySignal::StartRun => mob_dsl::MobMachineSignal::StartRun,
        MobAuthoritySignal::FinishRun => mob_dsl::MobMachineSignal::FinishRun,
        MobAuthoritySignal::BeginCleanup => mob_dsl::MobMachineSignal::BeginCleanup,
        MobAuthoritySignal::FinishCleanup => mob_dsl::MobMachineSignal::FinishCleanup,
        MobAuthoritySignal::InitializeOrchestrator => {
            mob_dsl::MobMachineSignal::InitializeOrchestrator
        }
        MobAuthoritySignal::BindCoordinator => mob_dsl::MobMachineSignal::BindCoordinator,
        MobAuthoritySignal::UnbindCoordinator => mob_dsl::MobMachineSignal::UnbindCoordinator,
        MobAuthoritySignal::StageSpawn => mob_dsl::MobMachineSignal::StageSpawn,
        MobAuthoritySignal::CompleteSpawn => mob_dsl::MobMachineSignal::CompleteSpawn,
        MobAuthoritySignal::StartFlow => mob_dsl::MobMachineSignal::StartFlow,
        MobAuthoritySignal::CompleteFlow => mob_dsl::MobMachineSignal::CompleteFlow,
        MobAuthoritySignal::StopOrchestrator => mob_dsl::MobMachineSignal::StopOrchestrator,
        MobAuthoritySignal::ResumeOrchestrator => mob_dsl::MobMachineSignal::ResumeOrchestrator,
        MobAuthoritySignal::DestroyOrchestrator => mob_dsl::MobMachineSignal::DestroyOrchestrator,
        MobAuthoritySignal::ForceCancelMember => mob_dsl::MobMachineSignal::ForceCancelMember,
        MobAuthoritySignal::MemberPeerExposed => mob_dsl::MobMachineSignal::MemberPeerExposed,
        MobAuthoritySignal::MemberTerminalized => mob_dsl::MobMachineSignal::MemberTerminalized,
        MobAuthoritySignal::OperationPeerTrusted => mob_dsl::MobMachineSignal::OperationPeerTrusted,
        MobAuthoritySignal::PeerInputAdmitted => mob_dsl::MobMachineSignal::PeerInputAdmitted,
        MobAuthoritySignal::CreateRun => mob_dsl::MobMachineSignal::CreateRun,
    }
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

pub(crate) fn map_error(err: mob_dsl::MobMachineTransitionError, context: &str) -> MobError {
    match err {
        mob_dsl::MobMachineTransitionError::NoMatchingTransition { phase, trigger } => {
            MobError::Internal(format!(
                "DSL authority rejected {context}: no matching transition from {phase} for {trigger}"
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Write-back: DSL state → actor observable
// ---------------------------------------------------------------------------

/// Write back the DSL-owned phase to the actor's observable AtomicU8.
///
/// Other DSL state fields (live_runtime_ids, active_run_count, etc.) are NOT
/// written back — they are projections of distributed actor state that the
/// shell mutates as part of handling effects. The DSL authority validates that
/// these mutations are legal; the shell performs them.
pub(crate) fn write_back_phase(
    dsl_phase: mob_dsl::MobPhase,
    observable: &std::sync::atomic::AtomicU8,
) {
    let mob_state = match dsl_phase {
        mob_dsl::MobPhase::Running => MobState::Running,
        mob_dsl::MobPhase::Stopped => MobState::Stopped,
        mob_dsl::MobPhase::Completed => MobState::Completed,
        mob_dsl::MobPhase::Destroyed => MobState::Destroyed,
    };
    observable.store(mob_state as u8, Ordering::Release);
}

// ---------------------------------------------------------------------------
// Top-level authority
// ---------------------------------------------------------------------------

/// The unified DSL authority for the MobMachine.
///
/// Replaces both `MobLifecycleAuthority` and `MobOrchestratorAuthority`.
/// Stateless — all state is projected from the actor on each call.
pub(crate) struct MobDslAuthority;

impl MobDslAuthority {
    /// Validate and apply an input transition.
    ///
    /// Projects the actor's current state, runs the DSL apply(), and returns
    /// the transition result with effects. The actor shell must:
    /// 1. Call `write_back_phase()` with the result's `new_phase`
    /// 2. Execute each effect (roster mutations, event emission, etc.)
    pub(crate) fn apply_input(
        dsl_state: mob_dsl::MobMachineState,
        input: &MobAuthorityInput,
    ) -> Result<MobTransitionResult, MobError> {
        let dsl_input = convert_input(input);
        let mut dsl_auth = mob_dsl::MobMachineAuthority::from_state(dsl_state);
        let transition = mob_dsl::MobMachineMutator::apply(&mut dsl_auth, dsl_input)
            .map_err(|e| map_error(e, &format!("{input:?}")))?;
        Ok(MobTransitionResult {
            new_phase: dsl_phase_to_result(transition.to_phase),
            effects: transition.effects,
        })
    }

    /// Validate and apply a signal transition.
    ///
    /// Same pattern as `apply_input` but for internal signals.
    pub(crate) fn apply_signal(
        dsl_state: mob_dsl::MobMachineState,
        signal: &MobAuthoritySignal,
    ) -> Result<MobTransitionResult, MobError> {
        let dsl_signal = convert_signal(signal);
        let mut dsl_auth = mob_dsl::MobMachineAuthority::from_state(dsl_state);
        let transition = dsl_auth
            .apply_signal(dsl_signal)
            .map_err(|e| map_error(e, &format!("{signal:?}")))?;
        Ok(MobTransitionResult {
            new_phase: dsl_phase_to_result(transition.to_phase),
            effects: transition.effects,
        })
    }

    /// Check if an input transition is legal without applying it.
    pub(crate) fn can_accept_input(
        dsl_state: mob_dsl::MobMachineState,
        input: &MobAuthorityInput,
    ) -> bool {
        let dsl_input = convert_input(input);
        let mut dsl_auth = mob_dsl::MobMachineAuthority::from_state(dsl_state);
        mob_dsl::MobMachineMutator::apply(&mut dsl_auth, dsl_input).is_ok()
    }

    /// Check if a signal transition is legal without applying it.
    pub(crate) fn can_accept_signal(
        dsl_state: mob_dsl::MobMachineState,
        signal: &MobAuthoritySignal,
    ) -> bool {
        let dsl_signal = convert_signal(signal);
        let mut dsl_auth = mob_dsl::MobMachineAuthority::from_state(dsl_state);
        dsl_auth.apply_signal(dsl_signal).is_ok()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn default_state() -> mob_dsl::MobMachineState {
        mob_dsl::MobMachineState::default()
    }

    #[test]
    fn stop_from_running_transitions_to_stopped() {
        let state = default_state();
        let result = MobDslAuthority::apply_input(state, &MobAuthorityInput::Stop)
            .expect("stop should succeed");
        assert_eq!(result.new_phase, MobPhaseResult::Stopped);
    }

    #[test]
    fn resume_from_stopped_transitions_to_running() {
        let mut state = default_state();
        state.lifecycle_phase = mob_dsl::MobPhase::Stopped;
        let result = MobDslAuthority::apply_input(state, &MobAuthorityInput::Resume)
            .expect("resume should succeed");
        assert_eq!(result.new_phase, MobPhaseResult::Running);
    }

    #[test]
    fn destroy_from_running_transitions_to_destroyed() {
        let state = default_state();
        let result = MobDslAuthority::apply_input(state, &MobAuthorityInput::Destroy)
            .expect("destroy should succeed");
        assert_eq!(result.new_phase, MobPhaseResult::Destroyed);
    }

    #[test]
    fn mark_completed_requires_no_active_runs() {
        let mut state = default_state();
        state.active_run_count = 1;
        let result = MobDslAuthority::apply_signal(state, &MobAuthoritySignal::MarkCompleted);
        assert!(
            result.is_err(),
            "should reject MarkCompleted with active runs"
        );
    }

    #[test]
    fn mark_completed_succeeds_with_zero_runs() {
        let state = default_state();
        let result = MobDslAuthority::apply_signal(state, &MobAuthoritySignal::MarkCompleted)
            .expect("mark completed");
        assert_eq!(result.new_phase, MobPhaseResult::Completed);
    }

    #[test]
    fn stage_spawn_increments_pending() {
        let state = default_state();
        // StageSpawn is a signal — we can't directly observe the state change
        // from the result (it's effects-based), but the transition should succeed.
        let result = MobDslAuthority::apply_signal(state, &MobAuthoritySignal::StageSpawn)
            .expect("stage spawn");
        assert_eq!(result.new_phase, MobPhaseResult::Running);
        assert!(!result.effects.is_empty());
    }

    #[test]
    fn complete_spawn_requires_pending_spawns() {
        let state = default_state();
        let result = MobDslAuthority::apply_signal(state, &MobAuthoritySignal::CompleteSpawn);
        assert!(
            result.is_err(),
            "should reject CompleteSpawn with zero pending"
        );
    }

    #[test]
    fn can_accept_input_probes_without_mutation() {
        let state = default_state();
        assert!(MobDslAuthority::can_accept_input(
            state.clone(),
            &MobAuthorityInput::Stop
        ));
        assert!(!MobDslAuthority::can_accept_input(
            state,
            &MobAuthorityInput::Resume
        ));
    }

    #[test]
    fn creating_phase_maps_to_running() {
        // Creating is a domain-only phase that maps to Running in the DSL.
        let state = project_mob_state(
            MobState::Creating,
            &Default::default(),
            &Default::default(),
            &Default::default(),
            0,
            0,
            true,
        );
        assert_eq!(state.lifecycle_phase, mob_dsl::MobPhase::Running);
    }

    #[test]
    fn spawn_adds_to_live_runtime_ids() {
        let state = default_state();
        let identity = ids::AgentIdentity::from("worker");
        let runtime_id = ids::AgentRuntimeId::initial(identity.clone());
        let fence_token = ids::FenceToken::new(1);
        let generation = ids::Generation::INITIAL;

        let result = MobDslAuthority::apply_input(
            state,
            &MobAuthorityInput::Spawn {
                identity,
                runtime_id,
                fence_token,
                generation,
                external_addressable: true,
            },
        )
        .expect("spawn should succeed");

        assert_eq!(result.new_phase, MobPhaseResult::Running);
        // Effects should include RequestRuntimeBinding
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, mob_dsl::MobMachineEffect::RequestRuntimeBinding { .. }))
        );
    }

    #[test]
    fn submit_work_requires_member_present() {
        let state = default_state(); // no live_runtime_ids
        let runtime_id = ids::AgentRuntimeId::initial(ids::AgentIdentity::from("worker"));
        let result = MobDslAuthority::apply_input(
            state,
            &MobAuthorityInput::SubmitWork {
                runtime_id,
                fence_token: ids::FenceToken::new(1),
                work_ref: ids::WorkRef::new(),
                origin: ids::WorkOrigin::External,
            },
        );
        assert!(result.is_err(), "should reject SubmitWork with no members");
    }

    #[test]
    fn destroy_from_all_valid_phases() {
        for phase in [
            mob_dsl::MobPhase::Running,
            mob_dsl::MobPhase::Stopped,
            mob_dsl::MobPhase::Completed,
        ] {
            let mut state = default_state();
            state.lifecycle_phase = phase;
            let result = MobDslAuthority::apply_input(state, &MobAuthorityInput::Destroy)
                .unwrap_or_else(|_| panic!("destroy should work from {phase:?}"));
            assert_eq!(result.new_phase, MobPhaseResult::Destroyed);
        }
    }

    #[test]
    fn destroy_from_destroyed_is_rejected() {
        let mut state = default_state();
        state.lifecycle_phase = mob_dsl::MobPhase::Destroyed;
        let result = MobDslAuthority::apply_input(state, &MobAuthorityInput::Destroy);
        assert!(result.is_err());
    }

    #[test]
    fn run_flow_requires_coordinator() {
        // RunFlow does NOT guard on live_runtime_ids (member presence is checked
        // at step dispatch, not flow admission). It only requires coordinator_bound.
        let state = default_state(); // no members, but coordinator_bound = true
        let result = MobDslAuthority::apply_input(state, &MobAuthorityInput::RunFlow);
        assert!(
            result.is_ok(),
            "RunFlow should succeed even without members when coordinator_bound"
        );

        let mut state = default_state();
        state.coordinator_bound = false;
        let result = MobDslAuthority::apply_input(state, &MobAuthorityInput::RunFlow);
        assert!(result.is_err(), "RunFlow needs coordinator_bound");

        let mut state = default_state();
        state
            .live_runtime_ids
            .insert(mob_dsl::AgentRuntimeId::from("worker:0"));
        state.coordinator_bound = true;
        let result = MobDslAuthority::apply_input(state, &MobAuthorityInput::RunFlow);
        assert!(
            result.is_ok(),
            "RunFlow should succeed with members + coordinator"
        );
    }
}
