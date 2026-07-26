//! Level 2 — MeerkatMachine durable-tail recovery authorization.
//!
//! SessionDocumentMachine CLASSIFIES the durable tail; the MeerkatMachine —
//! owner of run identity, input lifecycle, and terminal outcome — AUTHORIZES
//! what recovery may do with the classified candidate. These tests drive the
//! generated `AuthorizeDurableTailRecovery` decision self-loops directly and
//! pin the disposition table:
//!
//! - quiescent (Idle/Retired), no current run, candidate run not already
//!   terminalized: Completed -> `CommitCompleted`, Repairable ->
//!   `RepairAndCommitInterrupted`, Ambiguous -> `HoldIntact`;
//! - conflicting run facts (live run, or the candidate's run already produced
//!   the retained turn terminal witness): `RefuseRecovery`;
//! - every non-quiescent phase: `RefuseRecovery`.
//!
//! No disposition discards the tail; refusal and hold both retain it intact.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use meerkat_runtime::meerkat_machine::dsl as mm_dsl;

fn quiescent_state(phase: mm_dsl::MeerkatPhase) -> mm_dsl::MeerkatMachineState {
    mm_dsl::MeerkatMachineState {
        lifecycle_phase: phase,
        session_id: Some(mm_dsl::SessionId("session-recovery".to_string())),
        ..Default::default()
    }
}

fn recovery_input(
    candidate_id: &str,
    candidate_run_id: &str,
    class: mm_dsl::DurableTailRecoveryClass,
) -> mm_dsl::MeerkatMachineInput {
    mm_dsl::MeerkatMachineInput::AuthorizeDurableTailRecovery {
        session_id: mm_dsl::SessionId("session-recovery".to_string()),
        candidate_id: candidate_id.to_string(),
        candidate_run_id: mm_dsl::RunId(candidate_run_id.to_string()),
        class,
    }
}

/// Apply one recovery-authorization input and return the single emitted
/// `DurableTailRecoveryAuthorized` payload.
fn authorize(
    authority: &mut mm_dsl::MeerkatMachineAuthority,
    candidate_id: &str,
    candidate_run_id: &str,
    class: mm_dsl::DurableTailRecoveryClass,
) -> (String, mm_dsl::DurableTailRecoveryDisposition) {
    let transition = mm_dsl::MeerkatMachineMutator::apply(
        authority,
        recovery_input(candidate_id, candidate_run_id, class),
    )
    .expect("AuthorizeDurableTailRecovery must be total over machine phases");
    let mut authorized = transition
        .into_effects()
        .into_iter()
        .filter_map(|effect| match effect {
            mm_dsl::MeerkatMachineEffect::DurableTailRecoveryAuthorized {
                candidate_id,
                disposition,
            } => Some((candidate_id, disposition)),
            _ => None,
        });
    let first = authorized
        .next()
        .expect("authorization must emit a DurableTailRecoveryAuthorized effect");
    assert!(
        authorized.next().is_none(),
        "authorization must emit exactly one DurableTailRecoveryAuthorized effect"
    );
    first
}

#[test]
fn idle_quiescent_completed_candidate_commits_completed() {
    let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(quiescent_state(
        mm_dsl::MeerkatPhase::Idle,
    ))
    .expect("idle quiescent state must be recoverable");
    let (candidate_id, disposition) = authorize(
        &mut authority,
        "candidate-1",
        "run-1",
        mm_dsl::DurableTailRecoveryClass::CompletedCandidate,
    );
    assert_eq!(candidate_id, "candidate-1");
    assert_eq!(
        disposition,
        mm_dsl::DurableTailRecoveryDisposition::CommitCompleted
    );
    assert_eq!(
        authority.state().lifecycle_phase,
        mm_dsl::MeerkatPhase::Idle,
        "the decision is a self-loop; authorization must not move the lifecycle"
    );
}

#[test]
fn retired_quiescent_completed_candidate_commits_completed() {
    let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(quiescent_state(
        mm_dsl::MeerkatPhase::Retired,
    ))
    .expect("retired quiescent state must be recoverable");
    let (candidate_id, disposition) = authorize(
        &mut authority,
        "candidate-2",
        "run-2",
        mm_dsl::DurableTailRecoveryClass::CompletedCandidate,
    );
    assert_eq!(candidate_id, "candidate-2");
    assert_eq!(
        disposition,
        mm_dsl::DurableTailRecoveryDisposition::CommitCompleted
    );
    assert_eq!(
        authority.state().lifecycle_phase,
        mm_dsl::MeerkatPhase::Retired,
        "per_phase decision self-loops must keep Retired at Retired"
    );
}

#[test]
fn idle_repairable_candidate_repairs_and_commits_interrupted() {
    let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(quiescent_state(
        mm_dsl::MeerkatPhase::Idle,
    ))
    .expect("idle quiescent state must be recoverable");
    let (candidate_id, disposition) = authorize(
        &mut authority,
        "candidate-3",
        "run-3",
        mm_dsl::DurableTailRecoveryClass::InterruptedRepairableCandidate,
    );
    assert_eq!(candidate_id, "candidate-3");
    assert_eq!(
        disposition,
        mm_dsl::DurableTailRecoveryDisposition::RepairAndCommitInterrupted
    );
}

#[test]
fn idle_ambiguous_candidate_holds_intact() {
    let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(quiescent_state(
        mm_dsl::MeerkatPhase::Idle,
    ))
    .expect("idle quiescent state must be recoverable");
    let (candidate_id, disposition) = authorize(
        &mut authority,
        "candidate-4",
        "run-4",
        mm_dsl::DurableTailRecoveryClass::Ambiguous,
    );
    assert_eq!(candidate_id, "candidate-4");
    assert_eq!(
        disposition,
        mm_dsl::DurableTailRecoveryDisposition::HoldIntact
    );
}

/// A live current run refuses recovery. Idle with a current run is
/// machine-unrepresentable (`current_run_only_while_running_or_retired`
/// recovered-state invariant) — the machine forbids the state instead of
/// guarding it — so the live-run refusal is pinned at Retired, the one
/// quiescent phase that can carry a current run.
#[test]
fn live_current_run_refuses_recovery_and_is_unrepresentable_at_idle() {
    // (1) Idle + current run: recover_from_state itself must reject.
    let mut idle_with_run = quiescent_state(mm_dsl::MeerkatPhase::Idle);
    idle_with_run.current_run_id = Some(mm_dsl::RunId("run-live".to_string()));
    idle_with_run.pre_run_phase = Some(mm_dsl::PreRunPhase::Idle);
    let Err(rejection) = mm_dsl::MeerkatMachineAuthority::recover_from_state(idle_with_run) else {
        panic!("Idle with a live current run must be machine-unrepresentable");
    };
    assert!(
        matches!(
            rejection,
            mm_dsl::MeerkatMachineTransitionError::RecoveredStateInvariantRejected { .. }
        ),
        "expected a recovered-state invariant rejection, got {rejection:?}"
    );

    // (2) Retired + current run: RefuseRecovery even for a perfect completed
    // candidate.
    let mut retired_with_run = quiescent_state(mm_dsl::MeerkatPhase::Retired);
    retired_with_run.current_run_id = Some(mm_dsl::RunId("run-live".to_string()));
    retired_with_run.pre_run_phase = Some(mm_dsl::PreRunPhase::Retired);
    let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(retired_with_run)
        .expect("retired state with a current run must be recoverable");
    let (candidate_id, disposition) = authorize(
        &mut authority,
        "candidate-5",
        "run-other",
        mm_dsl::DurableTailRecoveryClass::CompletedCandidate,
    );
    assert_eq!(candidate_id, "candidate-5");
    assert_eq!(
        disposition,
        mm_dsl::DurableTailRecoveryDisposition::RefuseRecovery,
        "a live run means the machine's run facts conflict with recovery"
    );
}

/// A candidate whose run already produced the retained turn terminal witness
/// refuses recovery (the run is already terminalized); a different candidate
/// run still commits.
#[test]
fn terminalized_candidate_run_refuses_recovery() {
    let seed = || {
        let mut state = quiescent_state(mm_dsl::MeerkatPhase::Idle);
        state.turn_terminal_run_id = Some(mm_dsl::RunId("run-terminal".to_string()));
        state
    };

    let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(seed())
        .expect("idle state with a terminal witness must be recoverable");
    let (candidate_id, disposition) = authorize(
        &mut authority,
        "candidate-6",
        "run-terminal",
        mm_dsl::DurableTailRecoveryClass::CompletedCandidate,
    );
    assert_eq!(candidate_id, "candidate-6");
    assert_eq!(
        disposition,
        mm_dsl::DurableTailRecoveryDisposition::RefuseRecovery,
        "an already-terminalized candidate run must refuse (idempotence guard)"
    );

    // The refusal is about the EXACT run: a different candidate run against
    // the same machine state commits.
    let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(seed())
        .expect("idle state with a terminal witness must be recoverable");
    let (_, disposition) = authorize(
        &mut authority,
        "candidate-7",
        "run-different",
        mm_dsl::DurableTailRecoveryClass::CompletedCandidate,
    );
    assert_eq!(
        disposition,
        mm_dsl::DurableTailRecoveryDisposition::CommitCompleted
    );
}

/// Every non-quiescent lifecycle phase refuses recovery regardless of the
/// classification.
#[test]
fn non_quiescent_phases_refuse_recovery() {
    let states: Vec<(mm_dsl::MeerkatPhase, mm_dsl::MeerkatMachineState)> = vec![
        (
            mm_dsl::MeerkatPhase::Initializing,
            mm_dsl::MeerkatMachineState {
                lifecycle_phase: mm_dsl::MeerkatPhase::Initializing,
                ..Default::default()
            },
        ),
        (
            mm_dsl::MeerkatPhase::Attached,
            quiescent_state(mm_dsl::MeerkatPhase::Attached),
        ),
        (mm_dsl::MeerkatPhase::Running, {
            let mut state = quiescent_state(mm_dsl::MeerkatPhase::Running);
            state.current_run_id = Some(mm_dsl::RunId("run-live".to_string()));
            state.pre_run_phase = Some(mm_dsl::PreRunPhase::Idle);
            state
        }),
        (
            mm_dsl::MeerkatPhase::Stopped,
            quiescent_state(mm_dsl::MeerkatPhase::Stopped),
        ),
        (
            mm_dsl::MeerkatPhase::Destroyed,
            quiescent_state(mm_dsl::MeerkatPhase::Destroyed),
        ),
    ];
    for (phase, state) in states {
        for class in [
            mm_dsl::DurableTailRecoveryClass::CompletedCandidate,
            mm_dsl::DurableTailRecoveryClass::InterruptedRepairableCandidate,
            mm_dsl::DurableTailRecoveryClass::Ambiguous,
        ] {
            let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(state.clone())
                .unwrap_or_else(|err| panic!("{phase:?} seed state must be recoverable: {err:?}"));
            let (candidate_id, disposition) =
                authorize(&mut authority, "candidate-8", "run-8", class);
            assert_eq!(candidate_id, "candidate-8");
            assert_eq!(
                disposition,
                mm_dsl::DurableTailRecoveryDisposition::RefuseRecovery,
                "phase {phase:?} with class {class:?} must refuse recovery"
            );
            assert_eq!(
                authority.state().lifecycle_phase,
                phase,
                "the refusal self-loop must not move the lifecycle from {phase:?}"
            );
        }
    }
}

/// The disposition vocabulary itself: no value exists that discards the
/// durable tail. The match is exhaustive WITHOUT a wildcard arm, so adding
/// any new disposition variant fails compilation here and forces a human to
/// classify whether it retains the tail.
#[test]
fn no_disposition_value_discards_the_durable_tail() {
    let all = [
        mm_dsl::DurableTailRecoveryDisposition::RefuseRecovery,
        mm_dsl::DurableTailRecoveryDisposition::CommitCompleted,
        mm_dsl::DurableTailRecoveryDisposition::RepairAndCommitInterrupted,
        mm_dsl::DurableTailRecoveryDisposition::HoldIntact,
    ];
    for disposition in all {
        let retains_every_durable_message = match disposition {
            // Not admissible now; the tail is retained for a later retry.
            mm_dsl::DurableTailRecoveryDisposition::RefuseRecovery => true,
            // The tail IS the committed transcript.
            mm_dsl::DurableTailRecoveryDisposition::CommitCompleted => true,
            // Repair appends synthetic interrupted results; it never removes
            // durable evidence.
            mm_dsl::DurableTailRecoveryDisposition::RepairAndCommitInterrupted => true,
            // Held intact, readable, blocked from resume.
            mm_dsl::DurableTailRecoveryDisposition::HoldIntact => true,
        };
        assert!(
            retains_every_durable_message,
            "{disposition:?} must retain the durable tail"
        );
    }
}
