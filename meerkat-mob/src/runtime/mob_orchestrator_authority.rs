//! Generated-authority module for the MobOrchestrator machine.
//!
//! This module provides typed enums and a sealed mutator trait that enforces
//! all MobOrchestrator state mutations flow through the machine authority.
//! Handwritten shell code calls [`MobOrchestratorAuthority::apply`] and executes
//! returned effects; it cannot mutate canonical state directly.
//!
//! The transition table encoded here is the single source of truth, matching
//! the machine schema in `meerkat-machine-schema/src/catalog/mob_orchestrator.rs`:
//!
//! - 5 states: Creating, Running, Stopped, Completed, Destroyed
//! - 12 inputs: InitializeOrchestrator, BindCoordinator, UnbindCoordinator,
//!   StageSpawn, CompleteSpawn, StartFlow, CompleteFlow, StopOrchestrator,
//!   ResumeOrchestrator, MarkCompleted, DestroyOrchestrator, ForceCancelMember,
//!   RespawnMember
//! - 5 fields: coordinator_bound (bool), pending_spawn_count (u32),
//!   active_flow_count (u32), topology_revision (u32), supervisor_active (bool)
//! - 1 invariant: destroyed_is_terminal
//! - 14 transitions with guards
//! - 7 effects: ActivateSupervisor, DeactivateSupervisor, FlowActivated,
//!   FlowDeactivated, EmitOrchestratorNotice, MemberForceCancelled,
//!   MemberRespawnInitiated

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::error::MobError;
use crate::runtime::MobState;

// ---------------------------------------------------------------------------
// Typed input enum — mirrors the machine schema's input variants
// ---------------------------------------------------------------------------

/// Typed inputs for the MobOrchestrator machine.
///
/// Shell code classifies raw commands into these typed inputs, then calls
/// [`MobOrchestratorAuthority::apply`]. The authority decides transition legality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MobOrchestratorInput {
    InitializeOrchestrator,
    BindCoordinator,
    UnbindCoordinator,
    StageSpawn,
    CompleteSpawn,
    StartFlow,
    CompleteFlow,
    StopOrchestrator,
    ResumeOrchestrator,
    MarkCompleted,
    DestroyOrchestrator,
    ForceCancelMember,
    RespawnMember,
}

// ---------------------------------------------------------------------------
// Typed effect enum — mirrors the machine schema's effect variants
// ---------------------------------------------------------------------------

/// Effects emitted by MobOrchestrator transitions.
///
/// Shell code receives these from [`MobOrchestratorAuthority::apply`] and is
/// responsible for executing the side effects (e.g. activating supervisor,
/// calling topology service).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MobOrchestratorEffect {
    ActivateSupervisor,
    DeactivateSupervisor,
    FlowActivated,
    FlowDeactivated,
    EmitOrchestratorNotice,
    MemberForceCancelled,
    MemberRespawnInitiated,
}

// ---------------------------------------------------------------------------
// Transition result
// ---------------------------------------------------------------------------

/// Successful transition outcome from the MobOrchestrator authority.
#[derive(Debug)]
pub(crate) struct MobOrchestratorTransition {
    /// The phase after the transition.
    pub next_phase: MobState,
    /// Snapshot of all fields after the transition.
    pub snapshot: MobOrchestratorSnapshot,
    /// Effects to be executed by shell code.
    pub effects: Vec<MobOrchestratorEffect>,
}

// ---------------------------------------------------------------------------
// Observable snapshot
// ---------------------------------------------------------------------------

/// Observable snapshot of MobOrchestrator canonical fields.
///
/// Returned in every transition result for lock-free reads. The shell may
/// expose this for diagnostics but must not use it to decide transitions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MobOrchestratorSnapshot {
    pub coordinator_bound: bool,
    pub pending_spawn_count: u32,
    pub active_flow_count: u32,
    pub topology_revision: u32,
    pub supervisor_active: bool,
}

// ---------------------------------------------------------------------------
// Canonical machine state (fields)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct MobOrchestratorFields {
    coordinator_bound: bool,
    pending_spawn_count: u32,
    active_flow_count: u32,
    topology_revision: u32,
    supervisor_active: bool,
}

impl MobOrchestratorFields {
    fn to_snapshot(&self) -> MobOrchestratorSnapshot {
        MobOrchestratorSnapshot {
            coordinator_bound: self.coordinator_bound,
            pending_spawn_count: self.pending_spawn_count,
            active_flow_count: self.active_flow_count,
            topology_revision: self.topology_revision,
            supervisor_active: self.supervisor_active,
        }
    }
}

// ---------------------------------------------------------------------------
// Sealed mutator trait — only the authority implements this
// ---------------------------------------------------------------------------

mod sealed {
    pub trait Sealed {}
}

/// Sealed trait for MobOrchestrator state mutation.
///
/// Only [`MobOrchestratorAuthority`] implements this. Handwritten code cannot
/// create alternative implementations, ensuring single-source-of-truth
/// semantics for orchestrator state.
pub(crate) trait MobOrchestratorMutator: sealed::Sealed {
    /// Apply a typed input to the current machine state.
    ///
    /// Returns the transition result including next state, snapshot, and effects,
    /// or an error if the transition is not legal from the current state.
    fn apply(&mut self, input: MobOrchestratorInput)
    -> Result<MobOrchestratorTransition, MobError>;
}

// ---------------------------------------------------------------------------
// Authority implementation
// ---------------------------------------------------------------------------

/// The canonical authority for MobOrchestrator state.
///
/// Holds the canonical phase + fields and delegates all transitions through
/// the encoded transition table. Also maintains an `Arc<AtomicU8>` observable
/// cache so that `MobHandle` can read the current phase lock-free.
pub(crate) struct MobOrchestratorAuthority {
    /// Canonical phase.
    phase: MobState,
    /// Canonical machine-owned fields.
    fields: MobOrchestratorFields,
    /// Observable cache for lock-free reads.
    /// Set by the authority after each transition — never written elsewhere.
    observable: Arc<AtomicU8>,
}

impl sealed::Sealed for MobOrchestratorAuthority {}

impl MobOrchestratorAuthority {
    /// Create a new authority in Creating phase with default fields.
    pub(crate) fn new(observable: Arc<AtomicU8>) -> Self {
        observable.store(MobState::Creating as u8, Ordering::Release);
        Self {
            phase: MobState::Creating,
            fields: MobOrchestratorFields::default(),
            observable,
        }
    }

    /// Current phase (read from canonical state).
    pub(crate) fn phase(&self) -> MobState {
        self.phase
    }

    /// Current snapshot of all fields.
    pub(crate) fn snapshot(&self) -> MobOrchestratorSnapshot {
        self.fields.to_snapshot()
    }

    /// Check if a transition is legal without applying it.
    pub(crate) fn can_accept(&self, input: MobOrchestratorInput) -> bool {
        self.evaluate(input).is_ok()
    }

    /// Evaluate a transition without committing it.
    fn evaluate(
        &self,
        input: MobOrchestratorInput,
    ) -> Result<(MobState, MobOrchestratorFields, Vec<MobOrchestratorEffect>), MobError> {
        use MobOrchestratorInput::{
            BindCoordinator, CompleteFlow, CompleteSpawn, DestroyOrchestrator, ForceCancelMember,
            InitializeOrchestrator, MarkCompleted, RespawnMember, ResumeOrchestrator, StageSpawn,
            StartFlow, StopOrchestrator, UnbindCoordinator,
        };
        use MobState::{Completed, Creating, Destroyed, Running, Stopped};

        let phase = self.phase;
        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        let next_phase = match (phase, input) {
            // InitializeOrchestrator: Creating -> Running
            // Updates: supervisor_active = true
            // Emits: ActivateSupervisor
            (Creating, InitializeOrchestrator) => {
                fields.supervisor_active = true;
                effects.push(MobOrchestratorEffect::ActivateSupervisor);
                Running
            }

            // BindCoordinator: Running|Stopped|Completed -> Running
            // Guards: coordinator_is_not_bound
            // Updates: coordinator_bound = true, topology_revision += 1
            // Emits: EmitOrchestratorNotice
            (Running | Stopped | Completed, BindCoordinator) => {
                if fields.coordinator_bound {
                    return Err(MobError::Internal(
                        "guard failed: coordinator is already bound".into(),
                    ));
                }
                fields.coordinator_bound = true;
                fields.topology_revision = fields.topology_revision.saturating_add(1);
                effects.push(MobOrchestratorEffect::EmitOrchestratorNotice);
                Running
            }

            // UnbindCoordinator: Running|Stopped|Completed -> Stopped
            // Guards: coordinator_is_bound, no_pending_spawns
            // Updates: coordinator_bound = false, topology_revision += 1
            // Emits: EmitOrchestratorNotice
            (Running | Stopped | Completed, UnbindCoordinator) => {
                if !fields.coordinator_bound {
                    return Err(MobError::Internal(
                        "guard failed: coordinator is not bound".into(),
                    ));
                }
                if fields.pending_spawn_count != 0 {
                    return Err(MobError::Internal(
                        "guard failed: pending spawns exist".into(),
                    ));
                }
                fields.coordinator_bound = false;
                fields.topology_revision = fields.topology_revision.saturating_add(1);
                effects.push(MobOrchestratorEffect::EmitOrchestratorNotice);
                Stopped
            }

            // StageSpawn: Running -> Running
            // Guards: coordinator_is_bound
            // Updates: pending_spawn_count += 1, topology_revision += 1
            // Emits: EmitOrchestratorNotice
            (Running, StageSpawn) => {
                if !fields.coordinator_bound {
                    return Err(MobError::Internal(
                        "guard failed: coordinator is not bound (stage spawn requires bound coordinator)".into(),
                    ));
                }
                fields.pending_spawn_count = fields.pending_spawn_count.saturating_add(1);
                fields.topology_revision = fields.topology_revision.saturating_add(1);
                effects.push(MobOrchestratorEffect::EmitOrchestratorNotice);
                Running
            }

            // CompleteSpawn: Running|Stopped -> Running
            // Guards: has_pending_spawns
            // Updates: pending_spawn_count -= 1, topology_revision += 1
            // Emits: EmitOrchestratorNotice
            (Running | Stopped, CompleteSpawn) => {
                if fields.pending_spawn_count == 0 {
                    return Err(MobError::Internal(
                        "guard failed: no pending spawns to complete".into(),
                    ));
                }
                fields.pending_spawn_count -= 1;
                fields.topology_revision = fields.topology_revision.saturating_add(1);
                effects.push(MobOrchestratorEffect::EmitOrchestratorNotice);
                Running
            }

            // StartFlow: Running|Completed -> Running
            // Guards: coordinator_is_bound
            // Updates: active_flow_count += 1
            // Emits: FlowActivated, EmitOrchestratorNotice
            (Running | Completed, StartFlow) => {
                if !fields.coordinator_bound {
                    return Err(MobError::Internal(
                        "guard failed: coordinator is not bound (start flow requires bound coordinator)".into(),
                    ));
                }
                fields.active_flow_count = fields.active_flow_count.saturating_add(1);
                effects.push(MobOrchestratorEffect::FlowActivated);
                effects.push(MobOrchestratorEffect::EmitOrchestratorNotice);
                Running
            }

            // CompleteFlow: Running|Completed -> Running
            // Guards: has_active_flows
            // Updates: active_flow_count -= 1
            // Emits: FlowDeactivated, EmitOrchestratorNotice
            (Running | Completed, CompleteFlow) => {
                if fields.active_flow_count == 0 {
                    return Err(MobError::Internal(
                        "guard failed: no active flows to complete".into(),
                    ));
                }
                fields.active_flow_count -= 1;
                effects.push(MobOrchestratorEffect::FlowDeactivated);
                effects.push(MobOrchestratorEffect::EmitOrchestratorNotice);
                Running
            }

            // StopOrchestrator: Running|Completed -> Stopped
            // Guards: no_active_flows
            // Updates: supervisor_active = false, coordinator_bound = false,
            // topology_revision += 1 when unbinding the coordinator
            // Emits: DeactivateSupervisor, EmitOrchestratorNotice
            (Running | Completed, StopOrchestrator) => {
                if fields.active_flow_count != 0 {
                    return Err(MobError::Internal(
                        "guard failed: active flows exist (stop requires no active flows)".into(),
                    ));
                }
                fields.supervisor_active = false;
                if fields.coordinator_bound {
                    fields.coordinator_bound = false;
                    fields.topology_revision = fields.topology_revision.saturating_add(1);
                    effects.push(MobOrchestratorEffect::EmitOrchestratorNotice);
                }
                effects.push(MobOrchestratorEffect::DeactivateSupervisor);
                Stopped
            }

            // ResumeOrchestrator: Stopped -> Running
            // Updates: supervisor_active = true, coordinator_bound = true,
            // topology_revision += 1 if rebinding is required
            // Emits: ActivateSupervisor, EmitOrchestratorNotice when rebinding
            (Stopped, ResumeOrchestrator) => {
                if !fields.coordinator_bound {
                    fields.coordinator_bound = true;
                    fields.topology_revision = fields.topology_revision.saturating_add(1);
                    effects.push(MobOrchestratorEffect::EmitOrchestratorNotice);
                }
                fields.supervisor_active = true;
                effects.push(MobOrchestratorEffect::ActivateSupervisor);
                Running
            }

            // MarkCompleted: Running|Stopped -> Completed
            // Guards: no_active_flows, no_pending_spawns
            // Emits: EmitOrchestratorNotice
            (Running | Stopped, MarkCompleted) => {
                if fields.active_flow_count != 0 {
                    return Err(MobError::Internal(
                        "guard failed: active flows exist (mark completed requires no active flows)".into(),
                    ));
                }
                if fields.pending_spawn_count != 0 {
                    return Err(MobError::Internal(
                        "guard failed: pending spawns exist (mark completed requires no pending spawns)".into(),
                    ));
                }
                effects.push(MobOrchestratorEffect::EmitOrchestratorNotice);
                Completed
            }

            // DestroyOrchestrator: Stopped|Completed -> Destroyed
            // Guards: no_pending_spawns, no_active_flows
            // Updates: supervisor_active = false, coordinator_bound = false
            // Emits: DeactivateSupervisor, EmitOrchestratorNotice
            (Stopped | Completed, DestroyOrchestrator) => {
                if fields.pending_spawn_count != 0 {
                    return Err(MobError::Internal(
                        "guard failed: pending spawns exist (destroy requires no pending spawns)"
                            .into(),
                    ));
                }
                if fields.active_flow_count != 0 {
                    return Err(MobError::Internal(
                        "guard failed: active flows exist (destroy requires no active flows)"
                            .into(),
                    ));
                }
                fields.supervisor_active = false;
                fields.coordinator_bound = false;
                effects.push(MobOrchestratorEffect::DeactivateSupervisor);
                effects.push(MobOrchestratorEffect::EmitOrchestratorNotice);
                Destroyed
            }

            // ForceCancelMember: Running -> Running
            // Guards: coordinator_is_bound
            // Emits: MemberForceCancelled, EmitOrchestratorNotice
            (Running, ForceCancelMember) => {
                if !fields.coordinator_bound {
                    return Err(MobError::Internal(
                        "guard failed: coordinator is not bound (force cancel requires bound coordinator)".into(),
                    ));
                }
                effects.push(MobOrchestratorEffect::MemberForceCancelled);
                effects.push(MobOrchestratorEffect::EmitOrchestratorNotice);
                Running
            }

            // RespawnMember: Running -> Running
            // Guards: coordinator_is_bound
            // Updates: topology_revision += 1
            // Emits: MemberRespawnInitiated, EmitOrchestratorNotice
            (Running, RespawnMember) => {
                if !fields.coordinator_bound {
                    return Err(MobError::Internal(
                        "guard failed: coordinator is not bound (respawn requires bound coordinator)".into(),
                    ));
                }
                fields.topology_revision = fields.topology_revision.saturating_add(1);
                effects.push(MobOrchestratorEffect::MemberRespawnInitiated);
                effects.push(MobOrchestratorEffect::EmitOrchestratorNotice);
                Running
            }

            // All other combinations are illegal.
            _ => {
                let target = match input {
                    InitializeOrchestrator
                    | BindCoordinator
                    | ResumeOrchestrator
                    | StageSpawn
                    | CompleteSpawn
                    | StartFlow
                    | CompleteFlow
                    | ForceCancelMember
                    | RespawnMember => Running,
                    UnbindCoordinator | StopOrchestrator => Stopped,
                    MarkCompleted => Completed,
                    DestroyOrchestrator => Destroyed,
                };
                return Err(MobError::InvalidTransition {
                    from: phase,
                    to: target,
                });
            }
        };

        Ok((next_phase, fields, effects))
    }
}

impl MobOrchestratorMutator for MobOrchestratorAuthority {
    fn apply(
        &mut self,
        input: MobOrchestratorInput,
    ) -> Result<MobOrchestratorTransition, MobError> {
        let (next_phase, next_fields, effects) = self.evaluate(input)?;

        // Commit: update canonical state and observable cache.
        self.phase = next_phase;
        self.fields = next_fields;
        self.observable.store(next_phase as u8, Ordering::Release);

        Ok(MobOrchestratorTransition {
            next_phase,
            snapshot: self.fields.to_snapshot(),
            effects,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_authority() -> MobOrchestratorAuthority {
        MobOrchestratorAuthority::new(Arc::new(AtomicU8::new(0)))
    }

    fn make_running_authority() -> MobOrchestratorAuthority {
        let mut auth = make_authority();
        auth.apply(MobOrchestratorInput::InitializeOrchestrator)
            .expect("init");
        auth
    }

    #[test]
    fn initialize_transitions_to_running_and_activates_supervisor() {
        let mut auth = make_authority();
        let t = auth
            .apply(MobOrchestratorInput::InitializeOrchestrator)
            .expect("init should succeed from Creating");
        assert_eq!(t.next_phase, MobState::Running);
        assert!(t.snapshot.supervisor_active);
        assert!(
            t.effects
                .contains(&MobOrchestratorEffect::ActivateSupervisor)
        );
    }

    #[test]
    fn bind_coordinator_sets_bound_and_increments_revision() {
        let mut auth = make_running_authority();
        let t = auth
            .apply(MobOrchestratorInput::BindCoordinator)
            .expect("bind should succeed");
        assert!(t.snapshot.coordinator_bound);
        assert_eq!(t.snapshot.topology_revision, 1);
        assert!(
            t.effects
                .contains(&MobOrchestratorEffect::EmitOrchestratorNotice)
        );
    }

    #[test]
    fn bind_coordinator_rejects_when_already_bound() {
        let mut auth = make_running_authority();
        auth.apply(MobOrchestratorInput::BindCoordinator)
            .expect("first bind");
        let result = auth.apply(MobOrchestratorInput::BindCoordinator);
        assert!(result.is_err(), "second bind should fail");
    }

    #[test]
    fn unbind_coordinator_clears_bound_and_transitions_to_stopped() {
        let mut auth = make_running_authority();
        auth.apply(MobOrchestratorInput::BindCoordinator)
            .expect("bind");
        let t = auth
            .apply(MobOrchestratorInput::UnbindCoordinator)
            .expect("unbind should succeed");
        assert!(!t.snapshot.coordinator_bound);
        assert_eq!(t.next_phase, MobState::Stopped);
        assert_eq!(t.snapshot.topology_revision, 2);
    }

    #[test]
    fn unbind_rejects_when_not_bound() {
        let mut auth = make_running_authority();
        let result = auth.apply(MobOrchestratorInput::UnbindCoordinator);
        assert!(result.is_err());
    }

    #[test]
    fn unbind_rejects_with_pending_spawns() {
        let mut auth = make_running_authority();
        auth.apply(MobOrchestratorInput::BindCoordinator)
            .expect("bind");
        auth.apply(MobOrchestratorInput::StageSpawn)
            .expect("stage spawn");
        let result = auth.apply(MobOrchestratorInput::UnbindCoordinator);
        assert!(result.is_err(), "unbind should fail with pending spawns");
    }

    #[test]
    fn stage_spawn_increments_pending_and_revision() {
        let mut auth = make_running_authority();
        auth.apply(MobOrchestratorInput::BindCoordinator)
            .expect("bind");
        let t = auth
            .apply(MobOrchestratorInput::StageSpawn)
            .expect("stage spawn");
        assert_eq!(t.snapshot.pending_spawn_count, 1);
        assert_eq!(t.snapshot.topology_revision, 2);
    }

    #[test]
    fn stage_spawn_rejects_without_coordinator() {
        let mut auth = make_running_authority();
        let result = auth.apply(MobOrchestratorInput::StageSpawn);
        assert!(result.is_err());
    }

    #[test]
    fn complete_spawn_decrements_pending() {
        let mut auth = make_running_authority();
        auth.apply(MobOrchestratorInput::BindCoordinator)
            .expect("bind");
        auth.apply(MobOrchestratorInput::StageSpawn).expect("stage");
        let t = auth
            .apply(MobOrchestratorInput::CompleteSpawn)
            .expect("complete spawn");
        assert_eq!(t.snapshot.pending_spawn_count, 0);
    }

    #[test]
    fn complete_spawn_rejects_zero_pending() {
        let mut auth = make_running_authority();
        let result = auth.apply(MobOrchestratorInput::CompleteSpawn);
        assert!(result.is_err());
    }

    #[test]
    fn start_flow_increments_active_and_emits_activated() {
        let mut auth = make_running_authority();
        auth.apply(MobOrchestratorInput::BindCoordinator)
            .expect("bind");
        let t = auth
            .apply(MobOrchestratorInput::StartFlow)
            .expect("start flow");
        assert_eq!(t.snapshot.active_flow_count, 1);
        assert!(t.effects.contains(&MobOrchestratorEffect::FlowActivated));
    }

    #[test]
    fn complete_flow_decrements_active_and_emits_deactivated() {
        let mut auth = make_running_authority();
        auth.apply(MobOrchestratorInput::BindCoordinator)
            .expect("bind");
        auth.apply(MobOrchestratorInput::StartFlow).expect("start");
        let t = auth
            .apply(MobOrchestratorInput::CompleteFlow)
            .expect("complete flow");
        assert_eq!(t.snapshot.active_flow_count, 0);
        assert!(t.effects.contains(&MobOrchestratorEffect::FlowDeactivated));
    }

    #[test]
    fn complete_flow_rejects_zero_active() {
        let mut auth = make_running_authority();
        let result = auth.apply(MobOrchestratorInput::CompleteFlow);
        assert!(result.is_err());
    }

    #[test]
    fn stop_deactivates_supervisor() {
        let mut auth = make_running_authority();
        let t = auth
            .apply(MobOrchestratorInput::StopOrchestrator)
            .expect("stop");
        assert_eq!(t.next_phase, MobState::Stopped);
        assert!(!t.snapshot.supervisor_active);
        assert!(!t.snapshot.coordinator_bound);
        assert!(
            t.effects
                .contains(&MobOrchestratorEffect::DeactivateSupervisor)
        );
    }

    #[test]
    fn stop_rejects_with_active_flows() {
        let mut auth = make_running_authority();
        auth.apply(MobOrchestratorInput::BindCoordinator)
            .expect("bind");
        auth.apply(MobOrchestratorInput::StartFlow)
            .expect("start flow");
        let result = auth.apply(MobOrchestratorInput::StopOrchestrator);
        assert!(result.is_err());
    }

    #[test]
    fn resume_reactivates_supervisor_and_rebinds_coordinator() {
        let mut auth = make_running_authority();
        auth.apply(MobOrchestratorInput::BindCoordinator)
            .expect("bind");
        auth.apply(MobOrchestratorInput::StopOrchestrator)
            .expect("stop");
        let t = auth
            .apply(MobOrchestratorInput::ResumeOrchestrator)
            .expect("resume");
        assert_eq!(t.next_phase, MobState::Running);
        assert!(t.snapshot.supervisor_active);
        assert!(t.snapshot.coordinator_bound);
        assert!(
            t.effects
                .contains(&MobOrchestratorEffect::ActivateSupervisor)
        );
    }

    #[test]
    fn resume_binds_when_coordinator_is_absent() {
        let mut auth = make_running_authority();
        auth.apply(MobOrchestratorInput::StopOrchestrator)
            .expect("stop");
        let t = auth
            .apply(MobOrchestratorInput::ResumeOrchestrator)
            .expect("resume should rebind coordinator");
        assert!(t.snapshot.coordinator_bound);
        assert!(t.snapshot.supervisor_active);
    }

    #[test]
    fn mark_completed_transitions_to_completed() {
        let mut auth = make_running_authority();
        let t = auth
            .apply(MobOrchestratorInput::MarkCompleted)
            .expect("mark completed");
        assert_eq!(t.next_phase, MobState::Completed);
    }

    #[test]
    fn mark_completed_rejects_with_active_flows() {
        let mut auth = make_running_authority();
        auth.apply(MobOrchestratorInput::BindCoordinator)
            .expect("bind");
        auth.apply(MobOrchestratorInput::StartFlow).expect("start");
        let result = auth.apply(MobOrchestratorInput::MarkCompleted);
        assert!(result.is_err());
    }

    #[test]
    fn mark_completed_rejects_with_pending_spawns() {
        let mut auth = make_running_authority();
        auth.apply(MobOrchestratorInput::BindCoordinator)
            .expect("bind");
        auth.apply(MobOrchestratorInput::StageSpawn).expect("stage");
        let result = auth.apply(MobOrchestratorInput::MarkCompleted);
        assert!(result.is_err());
    }

    #[test]
    fn destroy_from_stopped() {
        let mut auth = make_running_authority();
        auth.apply(MobOrchestratorInput::StopOrchestrator)
            .expect("stop");
        let t = auth
            .apply(MobOrchestratorInput::DestroyOrchestrator)
            .expect("destroy");
        assert_eq!(t.next_phase, MobState::Destroyed);
        assert!(!t.snapshot.supervisor_active);
        assert!(!t.snapshot.coordinator_bound);
        assert!(
            t.effects
                .contains(&MobOrchestratorEffect::DeactivateSupervisor)
        );
        assert!(
            t.effects
                .contains(&MobOrchestratorEffect::EmitOrchestratorNotice)
        );
    }

    #[test]
    fn destroy_from_completed() {
        let mut auth = make_running_authority();
        auth.apply(MobOrchestratorInput::MarkCompleted)
            .expect("complete");
        let t = auth
            .apply(MobOrchestratorInput::DestroyOrchestrator)
            .expect("destroy");
        assert_eq!(t.next_phase, MobState::Destroyed);
    }

    #[test]
    fn destroy_rejects_from_running() {
        let mut auth = make_running_authority();
        let result = auth.apply(MobOrchestratorInput::DestroyOrchestrator);
        assert!(result.is_err(), "destroy from Running should fail");
    }

    #[test]
    fn destroy_rejects_from_creating() {
        let mut auth = make_authority();
        let result = auth.apply(MobOrchestratorInput::DestroyOrchestrator);
        assert!(result.is_err(), "destroy from Creating should fail");
    }

    #[test]
    fn force_cancel_member_emits_effect() {
        let mut auth = make_running_authority();
        auth.apply(MobOrchestratorInput::BindCoordinator)
            .expect("bind");
        let t = auth
            .apply(MobOrchestratorInput::ForceCancelMember)
            .expect("force cancel");
        assert_eq!(t.next_phase, MobState::Running);
        assert!(
            t.effects
                .contains(&MobOrchestratorEffect::MemberForceCancelled)
        );
    }

    #[test]
    fn respawn_member_increments_revision() {
        let mut auth = make_running_authority();
        auth.apply(MobOrchestratorInput::BindCoordinator)
            .expect("bind");
        let t = auth
            .apply(MobOrchestratorInput::RespawnMember)
            .expect("respawn");
        assert_eq!(t.next_phase, MobState::Running);
        assert_eq!(t.snapshot.topology_revision, 2);
        assert!(
            t.effects
                .contains(&MobOrchestratorEffect::MemberRespawnInitiated)
        );
    }

    #[test]
    fn observable_cache_updated_on_transition() {
        let observable = Arc::new(AtomicU8::new(0));
        let mut auth = MobOrchestratorAuthority::new(observable.clone());
        assert_eq!(observable.load(Ordering::Acquire), MobState::Creating as u8);
        auth.apply(MobOrchestratorInput::InitializeOrchestrator)
            .expect("init");
        assert_eq!(observable.load(Ordering::Acquire), MobState::Running as u8);
    }

    #[test]
    fn phase_unchanged_on_rejected_transition() {
        let mut auth = make_authority();
        let result = auth.apply(MobOrchestratorInput::StopOrchestrator);
        assert!(result.is_err());
        assert_eq!(auth.phase(), MobState::Creating);
    }

    #[test]
    fn can_accept_probes_without_mutation() {
        let auth = make_running_authority();
        assert!(auth.can_accept(MobOrchestratorInput::StopOrchestrator));
        assert!(!auth.can_accept(MobOrchestratorInput::InitializeOrchestrator));
        assert_eq!(auth.phase(), MobState::Running);
    }

    #[test]
    fn full_lifecycle_creating_to_destroyed() {
        let mut auth = make_authority();
        // Creating -> Running
        auth.apply(MobOrchestratorInput::InitializeOrchestrator)
            .expect("init");
        // Bind coordinator
        auth.apply(MobOrchestratorInput::BindCoordinator)
            .expect("bind");
        // Stage and complete a spawn
        auth.apply(MobOrchestratorInput::StageSpawn).expect("stage");
        auth.apply(MobOrchestratorInput::CompleteSpawn)
            .expect("complete spawn");
        // Start and complete a flow
        auth.apply(MobOrchestratorInput::StartFlow)
            .expect("start flow");
        auth.apply(MobOrchestratorInput::CompleteFlow)
            .expect("complete flow");
        // Stop
        auth.apply(MobOrchestratorInput::StopOrchestrator)
            .expect("stop");
        assert_eq!(auth.phase(), MobState::Stopped);
        // Destroy
        auth.apply(MobOrchestratorInput::DestroyOrchestrator)
            .expect("destroy");
        assert_eq!(auth.phase(), MobState::Destroyed);
    }
}
