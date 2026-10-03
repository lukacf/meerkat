#![allow(clippy::expect_used, clippy::panic)]

//! A transition with several source phases and one target phase moves every
//! other source phase to that target. When the transition is really
//! phase-agnostic bookkeeping, that is a silent phase change: MobMachine's flow
//! and loop bookkeeping used to share one arm guarded on
//! `Running || Stopped || Completed` with `to Running`, so a late flow record
//! moved a Stopped mob back to Running, undoing the Stop and stranding the
//! member run-start holds (#1500). Phase-agnostic bookkeeping must be written
//! per phase, each arm staying in its own phase.
//!
//! This ratchet refuses any multi-source transition whose target differs from
//! one of its source phases unless it is allowlisted below with the reason the
//! phase change is intended.

use meerkat_machine_schema::canonical_machine_schemas;

/// Why a multi-source transition may change the phase of its other sources.
#[derive(Debug, Clone, Copy)]
enum Reason {
    /// Leaves for a terminal or lifecycle-exit phase from any live phase.
    LifecycleExit,
    /// Restores the phase recorded in a durable snapshot.
    SnapshotRestore,
    /// Restarts the lifecycle from any settled phase.
    Restart,
    /// Converges several in-flight launch or delivery phases onto one outcome.
    OutcomeConvergence,
    /// An idempotent toggle between the two phases it names.
    IdempotentToggle,
}

const ALLOWED: &[(&str, &str, Reason)] = &[
    // MobMachine, reviewed with the #1500 Stopped-phase fix.
    ("MobMachine", "DestroyFromAny", Reason::LifecycleExit),
    ("MobMachine", "DestroyMob", Reason::LifecycleExit),
    ("MobMachine", "MarkCompleted", Reason::LifecycleExit),
    (
        "MobMachine",
        "ObserveRuntimeDestroyed",
        Reason::LifecycleExit,
    ),
    // Reset restarts the mob from any settled phase; from Stopped it also
    // emits ReleaseMemberRunStarts.
    ("MobMachine", "ResetToRunning", Reason::Restart),
    ("MeerkatMachine", "Destroy", Reason::LifecycleExit),
    (
        "MeerkatMachine",
        "RetireRequestedFromIdle",
        Reason::LifecycleExit,
    ),
    (
        "MeerkatMachine",
        "RetireRequestedFromIdleUnbound",
        Reason::LifecycleExit,
    ),
    (
        "MeerkatMachine",
        "RecycleFromIdleOrRetired",
        Reason::Restart,
    ),
    ("MeerkatMachine", "Reset", Reason::Restart),
    ("AuthMachine", "Acquire", Reason::Restart),
    ("AuthMachine", "Release", Reason::LifecycleExit),
    (
        "AuthMachine",
        "ClearCredentialLifecycle",
        Reason::LifecycleExit,
    ),
    (
        "AuthMachine",
        "ReleaseCredentialLifecycleWithOAuth",
        Reason::LifecycleExit,
    ),
    (
        "AuthMachine",
        "ReleaseCredentialLifecycleWithoutOAuth",
        Reason::LifecycleExit,
    ),
    (
        "AuthMachine",
        "RestoreAuthoritySnapshotExpired",
        Reason::SnapshotRestore,
    ),
    (
        "AuthMachine",
        "RestoreAuthoritySnapshotExpiring",
        Reason::SnapshotRestore,
    ),
    (
        "AuthMachine",
        "RestoreAuthoritySnapshotReauthRequired",
        Reason::SnapshotRestore,
    ),
    (
        "AuthMachine",
        "RestoreAuthoritySnapshotRefreshing",
        Reason::SnapshotRestore,
    ),
    (
        "AuthMachine",
        "RestoreAuthoritySnapshotReleased",
        Reason::SnapshotRestore,
    ),
    (
        "AuthMachine",
        "RestoreAuthoritySnapshotValid",
        Reason::SnapshotRestore,
    ),
    (
        "AuthMachine",
        "RestoreCredentialLifecycleSnapshotExpired",
        Reason::SnapshotRestore,
    ),
    (
        "AuthMachine",
        "RestoreCredentialLifecycleSnapshotExpiring",
        Reason::SnapshotRestore,
    ),
    (
        "AuthMachine",
        "RestoreCredentialLifecycleSnapshotNoCredentialWithOAuth",
        Reason::SnapshotRestore,
    ),
    (
        "AuthMachine",
        "RestoreCredentialLifecycleSnapshotNoCredentialWithoutOAuth",
        Reason::SnapshotRestore,
    ),
    (
        "AuthMachine",
        "RestoreCredentialLifecycleSnapshotReauthRequired",
        Reason::SnapshotRestore,
    ),
    (
        "AuthMachine",
        "RestoreCredentialLifecycleSnapshotRefreshing",
        Reason::SnapshotRestore,
    ),
    (
        "AuthMachine",
        "RestoreCredentialLifecycleSnapshotValid",
        Reason::SnapshotRestore,
    ),
    (
        "OccurrenceLifecycleMachine",
        "CompleteFromDispatchingOrAwaiting",
        Reason::OutcomeConvergence,
    ),
    (
        "OccurrenceLifecycleMachine",
        "DeliveryCompletionFailureInternalError",
        Reason::OutcomeConvergence,
    ),
    (
        "OccurrenceLifecycleMachine",
        "DeliveryCompletionFailureTransportError",
        Reason::OutcomeConvergence,
    ),
    (
        "OccurrenceLifecycleMachine",
        "DeliveryFailureInternalError",
        Reason::OutcomeConvergence,
    ),
    (
        "OccurrenceLifecycleMachine",
        "DeliveryFailureMobRejected",
        Reason::OutcomeConvergence,
    ),
    (
        "OccurrenceLifecycleMachine",
        "DeliveryFailureRuntimeRejected",
        Reason::OutcomeConvergence,
    ),
    (
        "OccurrenceLifecycleMachine",
        "DeliveryFailureTargetBusy",
        Reason::OutcomeConvergence,
    ),
    (
        "OccurrenceLifecycleMachine",
        "DeliveryFailureTargetMaterializationFailed",
        Reason::OutcomeConvergence,
    ),
    (
        "OccurrenceLifecycleMachine",
        "DeliveryFailureTargetMissing",
        Reason::OutcomeConvergence,
    ),
    (
        "OccurrenceLifecycleMachine",
        "DeliveryFailureTransportError",
        Reason::OutcomeConvergence,
    ),
    (
        "OccurrenceLifecycleMachine",
        "RuntimeCompletionCompleted",
        Reason::OutcomeConvergence,
    ),
    (
        "OccurrenceLifecycleMachine",
        "RuntimeCompletionInternalError",
        Reason::OutcomeConvergence,
    ),
    (
        "OccurrenceLifecycleMachine",
        "RuntimeCompletionRuntimeRejected",
        Reason::OutcomeConvergence,
    ),
    (
        "OccurrenceLifecycleMachine",
        "RuntimeCompletionTransportError",
        Reason::OutcomeConvergence,
    ),
    (
        "OccurrenceLifecycleMachine",
        "SupersedePendingOrLive",
        Reason::LifecycleExit,
    ),
    (
        "ScheduleLifecycleMachine",
        "PauseActiveOrPaused",
        Reason::IdempotentToggle,
    ),
    (
        "ScheduleLifecycleMachine",
        "ResumeActiveOrPaused",
        Reason::IdempotentToggle,
    ),
    (
        "WorkExecutionLifecycleMachine",
        "AcceptFlowLaunch",
        Reason::OutcomeConvergence,
    ),
    (
        "WorkExecutionLifecycleMachine",
        "FailLaunch",
        Reason::OutcomeConvergence,
    ),
    (
        "WorkExecutionLifecycleMachine",
        "QuarantineLaunch",
        Reason::OutcomeConvergence,
    ),
    (
        "WorkExecutionLifecycleMachine",
        "ObserveCanceledFlow",
        Reason::OutcomeConvergence,
    ),
    (
        "WorkExecutionLifecycleMachine",
        "ObserveCompletedFlow",
        Reason::OutcomeConvergence,
    ),
    (
        "WorkExecutionLifecycleMachine",
        "ObserveFailedFlow",
        Reason::OutcomeConvergence,
    ),
    (
        "WorkExecutionLifecycleMachine",
        "ObserveRunningFlow",
        Reason::OutcomeConvergence,
    ),
];

/// Multi-source transitions of one machine whose target differs from one of
/// their source phases.
fn foreign_target_transitions(machine: &meerkat_machine_schema::MachineSchema) -> Vec<String> {
    machine
        .transitions
        .iter()
        .filter(|transition| {
            transition.from.len() > 1 && transition.from.iter().any(|from| *from != transition.to)
        })
        .map(|transition| transition.name.as_str().to_owned())
        .collect()
}

fn allowed(machine: &str, transition: &str) -> Option<Reason> {
    ALLOWED
        .iter()
        .find(|(m, t, _)| *m == machine && *t == transition)
        .map(|(_, _, reason)| *reason)
}

#[test]
fn multi_source_transitions_change_phase_only_where_allowlisted() {
    let mut unexplained = Vec::new();
    let mut seen = Vec::new();
    for machine in canonical_machine_schemas() {
        for transition in foreign_target_transitions(&machine) {
            match allowed(machine.machine.as_str(), &transition) {
                Some(_reason) => seen.push((machine.machine.as_str().to_owned(), transition)),
                None => unexplained.push(format!("{}::{transition}", machine.machine)),
            }
        }
    }
    assert!(
        unexplained.is_empty(),
        "multi-source transitions that move other source phases to their target \
         without an allowlisted reason (write phase-agnostic arms per phase): {unexplained:?}"
    );
    // A stale allowlist entry is a deleted or fixed transition: drop it.
    let stale = ALLOWED
        .iter()
        .filter(|(m, t, _)| !seen.iter().any(|(sm, st)| sm == m && st == t))
        .map(|(m, t, reason)| format!("{m}::{t} ({reason:?})"))
        .collect::<Vec<_>>();
    assert!(stale.is_empty(), "stale allowlist entries: {stale:?}");
}

/// The MobMachine bookkeeping fix: nothing but ResumeStopped and Reset leaves
/// Stopped for Running.
#[test]
fn only_resume_and_reset_take_a_stopped_mob_to_running() {
    let mob = canonical_machine_schemas()
        .into_iter()
        .find(|machine| machine.machine.as_str() == "MobMachine")
        .expect("MobMachine");
    let mut leaving = mob
        .transitions
        .iter()
        .filter(|transition| {
            transition.to.as_str() == "Running"
                && transition
                    .from
                    .iter()
                    .any(|from| from.as_str() == "Stopped")
        })
        .map(|transition| transition.name.as_str())
        .collect::<Vec<_>>();
    leaving.sort_unstable();
    assert_eq!(leaving, ["ResetToRunning", "ResumeStopped"]);
}

/// Mutant: re-merging one bookkeeping arm across phases with a single
/// Running target is refused by the ratchet.
#[test]
fn a_merged_bookkeeping_arm_is_refused() {
    let mut mob = canonical_machine_schemas()
        .into_iter()
        .find(|machine| machine.machine.as_str() == "MobMachine")
        .expect("MobMachine");
    let running = mob
        .transitions
        .iter()
        .position(|transition| transition.name.as_str() == "AuthorizeFlowRunReducerCommandStartRun")
        .expect("Running arm");
    let stopped_phase = mob
        .transitions
        .iter()
        .find(|transition| {
            transition.name.as_str() == "AuthorizeFlowRunReducerCommandStartRunStopped"
        })
        .expect("Stopped arm")
        .from[0]
        .clone();
    mob.transitions[running].from.push(stopped_phase);
    let refused = foreign_target_transitions(&mob)
        .into_iter()
        .filter(|transition| allowed("MobMachine", transition).is_none())
        .collect::<Vec<_>>();
    assert_eq!(refused, ["AuthorizeFlowRunReducerCommandStartRun"]);
}
