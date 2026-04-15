//! Generated-authority module for the MobOrchestrator machine.
//!
//! This module provides typed enums and a sealed mutator trait that enforces
//! all MobOrchestrator state mutations flow through the machine authority.
//! Handwritten shell code calls [`MobOrchestratorAuthority::apply_in_phase`] and
//! executes returned effects; it cannot mutate canonical state directly.
//!
//! The transition table encoded here is the single source of truth, matching
//! the machine schema in `meerkat-machine-schema/src/catalog/mob_orchestrator.rs`:
//!
//! - 5 states: Creating, Running, Stopped, Completed, Destroyed
//! - 11 inputs: InitializeOrchestrator, BindCoordinator, UnbindCoordinator,
//!   StageSpawn, CompleteSpawn, StartFlow, CompleteFlow, StopOrchestrator,
//!   ResumeOrchestrator, MarkCompleted, DestroyOrchestrator, ForceCancelMember
//! - 5 fields: coordinator_bound (bool), pending_spawn_count (u32),
//!   active_flow_count (u32), topology_revision (u32), supervisor_active (bool)
//! - 1 invariant: destroyed_is_terminal
//! - 13 transitions with guards
//! - 6 effects: ActivateSupervisor, DeactivateSupervisor, FlowActivated,
//!   FlowDeactivated, EmitOrchestratorNotice, MemberForceCancelled

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
    #[cfg(test)]
    InitializeOrchestrator,
    BindCoordinator,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "schema-aligned authority input retained even when current shell paths do not construct it"
        )
    )]
    #[cfg_attr(test, allow(dead_code))]
    UnbindCoordinator,
    StageSpawn,
    CompleteSpawn,
    StartFlow,
    CompleteFlow,
    StopOrchestrator,
    ResumeOrchestrator,
    MarkCompleted,
    DestroyOrchestrator,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "schema-aligned authority input retained even when current shell paths do not construct it"
        )
    )]
    #[cfg_attr(test, allow(dead_code))]
    ForceCancelMember,
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
}

// ---------------------------------------------------------------------------
// Transition result
// ---------------------------------------------------------------------------

/// Successful transition outcome from the MobOrchestrator authority.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "transition snapshot retained for schema-aligned authority surface and tests"
    )
)]
#[cfg_attr(test, allow(dead_code))]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobOrchestratorSnapshot {
    pub phase: MobState,
    pub coordinator_bound: bool,
    pub pending_spawn_count: u32,
    pub active_flow_count: u32,
    pub topology_revision: u32,
    pub supervisor_active: bool,
}

impl Default for MobOrchestratorSnapshot {
    fn default() -> Self {
        Self {
            phase: MobState::Creating,
            coordinator_bound: false,
            pending_spawn_count: 0,
            active_flow_count: 0,
            topology_revision: 0,
            supervisor_active: false,
        }
    }
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
    fn to_snapshot(&self, phase: MobState, active_flow_count: u32) -> MobOrchestratorSnapshot {
        MobOrchestratorSnapshot {
            phase,
            coordinator_bound: self.coordinator_bound,
            pending_spawn_count: self.pending_spawn_count,
            active_flow_count,
            topology_revision: self.topology_revision,
            supervisor_active: self.supervisor_active,
        }
    }
}

// ---------------------------------------------------------------------------
// Sealed mutator trait — only the authority implements this
// ---------------------------------------------------------------------------

#[cfg(test)]
mod sealed {
    pub trait Sealed {}
}

/// Sealed trait for MobOrchestrator state mutation.
///
/// Only [`MobOrchestratorAuthority`] implements this. Tests keep a
/// phase-owning helper surface for direct table checks, but production code
/// routes legality through [`MobOrchestratorAuthority::apply_in_phase`].
#[cfg(test)]
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
/// the encoded transition table.
pub(crate) struct MobOrchestratorAuthority {
    /// Test-only phase owner retained for direct authority table tests.
    #[cfg(test)]
    phase: MobState,
    /// Canonical machine-owned fields.
    fields: MobOrchestratorFields,
}

#[cfg(test)]
impl sealed::Sealed for MobOrchestratorAuthority {}

impl MobOrchestratorAuthority {
    #[cfg(test)]
    /// Create a new authority in Creating phase with default fields.
    pub(crate) fn new() -> Self {
        Self {
            #[cfg(test)]
            phase: MobState::Creating,
            fields: MobOrchestratorFields::default(),
        }
    }

    /// Create a fresh authority directly in Running phase with the canonical
    /// running-default fields used by live mob bootstrap.
    pub(crate) fn new_running() -> Self {
        Self {
            #[cfg(test)]
            phase: MobState::Running,
            fields: MobOrchestratorFields {
                supervisor_active: true,
                ..MobOrchestratorFields::default()
            },
        }
    }

    /// Create an authority initialized to a specific phase.
    ///
    /// Used during resume to restore the authority to the persisted phase
    /// rather than always starting in Creating.
    pub(crate) fn with_phase(_phase: MobState) -> Self {
        Self {
            #[cfg(test)]
            phase: _phase,
            fields: MobOrchestratorFields::default(),
        }
    }

    /// Current phase (read from canonical state).
    #[cfg(test)]
    pub(crate) fn phase(&self) -> MobState {
        self.phase
    }

    /// Current snapshot of all fields.
    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> MobOrchestratorSnapshot {
        self.fields
            .to_snapshot(self.phase, self.fields.active_flow_count)
    }

    /// Current snapshot projected onto an external phase owner.
    pub(crate) fn snapshot_in_phase(
        &self,
        phase: MobState,
        active_flow_count: u32,
    ) -> MobOrchestratorSnapshot {
        self.fields.to_snapshot(phase, active_flow_count)
    }

    /// Check if a transition is legal without applying it.
    #[cfg(test)]
    pub(crate) fn can_accept(&self, input: MobOrchestratorInput) -> bool {
        self.evaluate(input).is_ok()
    }

    /// Check legality using an externally owned top-level phase.
    pub(crate) fn can_accept_in_phase(
        &self,
        phase: MobState,
        active_flow_count: u32,
        input: MobOrchestratorInput,
    ) -> bool {
        self.evaluate_in_phase(phase, active_flow_count, input)
            .is_ok()
    }

    /// Evaluate a transition without committing it.
    #[cfg(test)]
    fn evaluate(
        &self,
        input: MobOrchestratorInput,
    ) -> Result<
        (
            MobState,
            MobOrchestratorFields,
            u32,
            Vec<MobOrchestratorEffect>,
        ),
        MobError,
    > {
        self.evaluate_in_phase(self.phase, self.fields.active_flow_count, input)
    }

    fn evaluate_in_phase(
        &self,
        phase: MobState,
        active_flow_count: u32,
        input: MobOrchestratorInput,
    ) -> Result<
        (
            MobState,
            MobOrchestratorFields,
            u32,
            Vec<MobOrchestratorEffect>,
        ),
        MobError,
    > {
        use MobOrchestratorInput::{
            BindCoordinator, CompleteFlow, CompleteSpawn, DestroyOrchestrator, ForceCancelMember,
            MarkCompleted, ResumeOrchestrator, StageSpawn, StartFlow, StopOrchestrator,
            UnbindCoordinator,
        };
        use MobState::{Completed, Destroyed, Running, Stopped};

        let mut fields = self.fields.clone();
        let mut next_active_flow_count = active_flow_count;
        let mut effects = Vec::new();

        let next_phase = match (phase, input) {
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
                next_active_flow_count = active_flow_count.saturating_add(1);
                effects.push(MobOrchestratorEffect::FlowActivated);
                effects.push(MobOrchestratorEffect::EmitOrchestratorNotice);
                Running
            }

            // CompleteFlow: Running|Completed -> Running
            // Guards: has_active_flows
            // Updates: active_flow_count -= 1
            // Emits: FlowDeactivated, EmitOrchestratorNotice
            (Running | Completed, CompleteFlow) => {
                if active_flow_count == 0 {
                    return Err(MobError::Internal(
                        "guard failed: no active flows to complete".into(),
                    ));
                }
                next_active_flow_count = active_flow_count - 1;
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
                if active_flow_count != 0 {
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
                if active_flow_count != 0 {
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
                if active_flow_count != 0 {
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

            // All other combinations are illegal.
            _ => {
                let target = match input {
                    #[cfg(test)]
                    MobOrchestratorInput::InitializeOrchestrator => Running,
                    BindCoordinator | ResumeOrchestrator | StageSpawn | CompleteSpawn
                    | StartFlow | CompleteFlow | ForceCancelMember => Running,
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

        Ok((next_phase, fields, next_active_flow_count, effects))
    }

    pub(crate) fn apply_in_phase(
        &mut self,
        phase: MobState,
        active_flow_count: u32,
        input: MobOrchestratorInput,
    ) -> Result<MobOrchestratorTransition, MobError> {
        let (next_phase, next_fields, next_active_flow_count, effects) =
            self.evaluate_in_phase(phase, active_flow_count, input)?;

        self.fields = next_fields;
        self.fields.active_flow_count = next_active_flow_count;

        Ok(MobOrchestratorTransition {
            next_phase,
            snapshot: self.fields.to_snapshot(next_phase, next_active_flow_count),
            effects,
        })
    }
}

#[cfg(test)]
impl MobOrchestratorMutator for MobOrchestratorAuthority {
    fn apply(
        &mut self,
        input: MobOrchestratorInput,
    ) -> Result<MobOrchestratorTransition, MobError> {
        let (next_phase, next_fields, next_active_flow_count, effects) = self.evaluate(input)?;

        // Commit: update canonical state.
        self.phase = next_phase;
        self.fields = next_fields;
        self.fields.active_flow_count = next_active_flow_count;

        Ok(MobOrchestratorTransition {
            next_phase,
            snapshot: self.fields.to_snapshot(next_phase, next_active_flow_count),
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
        MobOrchestratorAuthority::new()
    }

    fn make_running_authority() -> MobOrchestratorAuthority {
        MobOrchestratorAuthority::new_running()
    }

    #[test]
    fn initialize_from_creating_is_rejected() {
        let mut auth = make_authority();
        let result = auth.apply(MobOrchestratorInput::InitializeOrchestrator);
        assert!(
            result.is_err(),
            "initialize should be rejected from Creating"
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
    fn internal_phase_updated_on_transition() {
        let mut auth = MobOrchestratorAuthority::new_running();
        auth.apply(MobOrchestratorInput::StopOrchestrator)
            .expect("stop");
        assert_eq!(auth.phase(), MobState::Stopped);
    }

    #[test]
    fn running_constructor_initializes_running_phase() {
        let auth = MobOrchestratorAuthority::new_running();
        assert_eq!(auth.phase(), MobState::Running);
        assert!(auth.snapshot().supervisor_active);
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
    fn full_lifecycle_running_to_destroyed() {
        let mut auth = make_running_authority();
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

    #[test]
    fn with_phase_initializes_to_given_phase() {
        let auth = MobOrchestratorAuthority::with_phase(MobState::Completed);
        assert_eq!(auth.phase(), MobState::Completed);
    }

    #[test]
    fn with_phase_stopped_accepts_resume() {
        let mut auth = MobOrchestratorAuthority::with_phase(MobState::Stopped);
        assert_eq!(auth.phase(), MobState::Stopped);
        let t = auth
            .apply(MobOrchestratorInput::ResumeOrchestrator)
            .expect("resume from stopped should succeed");
        assert_eq!(t.next_phase, MobState::Running);
    }
}
