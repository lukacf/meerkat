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

// ---------------------------------------------------------------------------
// A Stopped mob holds member run starts (#1500)
// ---------------------------------------------------------------------------
//
// The property is on state: MobMachine's `member_run_starts_held` records
// that a Stop, Shutdown or Completed cleanup held member run starts and no
// Resume or Reset has released them, and the invariant
// `stopped_mob_holds_member_run_starts` (checked by TLC) says a Stopped mob
// has it set. These checks run over every arm, not a list of names:
// - the field is written exactly by the arms that emit the hold (true) or
//   the release (false), so it cannot drift from the effects;
// - every arm entering Stopped from another phase either emits the hold or
//   is guarded on `member_run_starts_held == true`;
// - every arm triggered by the Shutdown input emits the hold, whichever
//   phase it leaves the mob in.

use meerkat_machine_schema::{Expr, TransitionSchema, TriggerMatch, Update};

const HOLD: &str = "HoldMemberRunStarts";
const RELEASE: &str = "ReleaseMemberRunStarts";
const HELD_FIELD: &str = "member_run_starts_held";
const STOPPED_HOLD_INVARIANT: &str = "stopped_mob_holds_member_run_starts";

fn mob_machine() -> meerkat_machine_schema::MachineSchema {
    canonical_machine_schemas()
        .into_iter()
        .find(|machine| machine.machine.as_str() == "MobMachine")
        .expect("MobMachine")
}

fn emits(transition: &TransitionSchema, effect: &str) -> bool {
    transition
        .emit
        .iter()
        .any(|emit| emit.variant.as_str() == effect)
}

fn is_held_field(expr: &Expr) -> bool {
    matches!(expr, Expr::Field(field) if field.as_str() == HELD_FIELD)
}

/// `member_run_starts_held == true`, either way round.
fn is_held_fact(expr: &Expr) -> bool {
    match expr {
        Expr::Eq(left, right) => {
            (is_held_field(left) && matches!(**right, Expr::Bool(true)))
                || (is_held_field(right) && matches!(**left, Expr::Bool(true)))
        }
        _ => false,
    }
}

fn guarded_held(transition: &TransitionSchema) -> bool {
    transition
        .guards
        .iter()
        .any(|guard| is_held_fact(&guard.expr))
}

/// What an arm writes to the held field.
#[derive(Debug, PartialEq, Eq)]
enum HeldWrite {
    Untouched,
    Set(bool),
    Computed,
}

fn held_write(transition: &TransitionSchema) -> HeldWrite {
    transition
        .updates
        .iter()
        .find_map(|update| match update {
            Update::Assign { field, expr } if field.as_str() == HELD_FIELD => Some(match expr {
                Expr::Bool(value) => HeldWrite::Set(*value),
                _ => HeldWrite::Computed,
            }),
            _ => None,
        })
        .unwrap_or(HeldWrite::Untouched)
}

fn enters_stopped(transition: &TransitionSchema) -> bool {
    transition.to.as_str() == "Stopped"
        && transition
            .from
            .iter()
            .any(|from| from.as_str() != "Stopped")
}

/// Arms whose write of the held field disagrees with their hold/release
/// effects.
fn held_field_drift(mob: &meerkat_machine_schema::MachineSchema) -> Vec<String> {
    mob.transitions
        .iter()
        .filter(|transition| {
            let expected = match (emits(transition, HOLD), emits(transition, RELEASE)) {
                (true, false) => HeldWrite::Set(true),
                (false, true) => HeldWrite::Set(false),
                (false, false) => HeldWrite::Untouched,
                (true, true) => return true,
            };
            held_write(transition) != expected
        })
        .map(|transition| transition.name.as_str().to_owned())
        .collect()
}

/// Arms that enter Stopped without holding or proving the hold.
fn unheld_stopped_entries(mob: &meerkat_machine_schema::MachineSchema) -> Vec<String> {
    mob.transitions
        .iter()
        .filter(|transition| enters_stopped(transition))
        .filter(|transition| !emits(transition, HOLD) && !guarded_held(transition))
        .map(|transition| transition.name.as_str().to_owned())
        .collect()
}

/// Shutdown arms that do not hold.
fn unheld_shutdowns(mob: &meerkat_machine_schema::MachineSchema) -> Vec<String> {
    mob.transitions
        .iter()
        .filter(|transition| {
            matches!(&transition.on, TriggerMatch::Input { variant, .. } if variant.as_str() == "Shutdown")
        })
        .filter(|transition| !emits(transition, HOLD))
        .map(|transition| transition.name.as_str().to_owned())
        .collect()
}

#[test]
fn the_held_field_is_written_exactly_by_the_hold_and_release_arms() {
    let mob = mob_machine();
    assert!(
        mob.transitions.iter().any(|t| emits(t, HOLD))
            && mob.transitions.iter().any(|t| emits(t, RELEASE)),
        "MobMachine no longer holds or releases member run starts"
    );
    assert_eq!(held_field_drift(&mob), Vec::<String>::new());
}

#[test]
fn every_entry_into_stopped_holds_member_run_starts() {
    let mob = mob_machine();
    assert!(
        mob.transitions.iter().any(enters_stopped),
        "MobMachine has no arm entering Stopped"
    );
    assert_eq!(unheld_stopped_entries(&mob), Vec::<String>::new());
    assert!(
        mob.invariants
            .iter()
            .any(|invariant| invariant.name == STOPPED_HOLD_INVARIANT),
        "MobMachine lost the {STOPPED_HOLD_INVARIANT} invariant TLC checks"
    );
}

#[test]
fn every_shutdown_holds_member_run_starts() {
    let mob = mob_machine();
    let shutdowns = mob
        .transitions
        .iter()
        .filter(|t| matches!(&t.on, TriggerMatch::Input { variant, .. } if variant.as_str() == "Shutdown"))
        .count();
    assert!(shutdowns > 0, "MobMachine has no Shutdown arm");
    assert_eq!(unheld_shutdowns(&mob), Vec::<String>::new());
}

fn arm<'a>(
    mob: &'a mut meerkat_machine_schema::MachineSchema,
    name: &str,
) -> &'a mut TransitionSchema {
    mob.transitions
        .iter_mut()
        .find(|transition| transition.name.as_str() == name)
        .unwrap_or_else(|| panic!("{name}"))
}

/// Mutant: a Shutdown of a Completed mob that does not hold is refused.
#[test]
fn a_shutdown_without_the_hold_is_refused() {
    let mut mob = mob_machine();
    arm(&mut mob, "ShutdownCompleted")
        .emit
        .retain(|emit| emit.variant.as_str() != HOLD);
    assert_eq!(unheld_shutdowns(&mob), ["ShutdownCompleted"]);
    assert_eq!(held_field_drift(&mob), ["ShutdownCompleted"]);
}

/// Mutant: the Stop commit without its proven-held guard is refused.
#[test]
fn a_stop_commit_without_the_held_guard_is_refused() {
    let mut mob = mob_machine();
    arm(&mut mob, "StopRunning")
        .guards
        .retain(|guard| !is_held_fact(&guard.expr));
    assert_eq!(unheld_stopped_entries(&mob), ["StopRunning"]);
}

/// Mutant: a Completed cleanup into Stopped that neither holds nor sets the
/// field is refused.
#[test]
fn a_cleanup_into_stopped_without_the_hold_is_refused() {
    let mut mob = mob_machine();
    let cleanup = arm(&mut mob, "BeginCleanupCompleted");
    cleanup.emit.retain(|emit| emit.variant.as_str() != HOLD);
    cleanup.updates.retain(
        |update| !matches!(update, Update::Assign { field, .. } if field.as_str() == HELD_FIELD),
    );
    assert_eq!(unheld_stopped_entries(&mob), ["BeginCleanupCompleted"]);
}
