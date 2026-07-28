//! Level 2 — MeerkatMachine durable-tail recovery authorization.
//!
//! SessionDocumentMachine CLASSIFIES the durable tail; the MeerkatMachine —
//! owner of run identity, input lifecycle, and terminal outcome — AUTHORIZES
//! what recovery may do with the classified candidate. These tests drive the
//! generated `AuthorizeDurableTailRecovery` transitions directly and pin the
//! disposition table:
//!
//! - quiescent in-process facts AND quiescent persisted observations
//!   (Missing/Idle/Retired row, no persisted current run), candidate run not
//!   already terminalized, durable receipts that do not already cover the
//!   candidate, and fully attributable input rows: Completed ->
//!   `CommitCompleted`, Repairable -> `RepairAndCommitInterrupted`, Legacy ->
//!   `CommitLegacyCompleted` (each with a machine-minted boundary sequence
//!   one past the last durably committed receipt), Ambiguous -> `HoldIntact`;
//! - durable receipts that already carry the candidate's content, or content
//!   the candidate neither extends nor equals: `RefuseRecovery` (the
//!   cross-process duplicate-recovery phantom);
//! - input rows durable identity cannot attribute, or blocking rows the store
//!   cannot fence: `HoldIntact` — minted HERE, never by the shell — EXCEPT a
//!   clean COMPLETED candidate with an unbound content input, which commits
//!   `CommitLegacyCompletedRetainInputs` (terminalize only proven-bound
//!   rows; the unbound input redelivers). That commit is deliberately NOT
//!   era-gated: a pre-0.8.9 writer's lost boundary routinely leaves the
//!   executed turn's own input unbound, so redelivery costs at most one
//!   duplicate turn (the legacy fleet's own restart semantics), while a
//!   0.8.9+ writer fences staged bindings durable before execution, so an
//!   unbound content row never started and redelivery is simply correct.
//!   Never a dropped input, never fabricated consumption; interrupted
//!   shapes and unfenceable rows hold in every era;
//! - conflicting in-process run facts (live run, or the candidate's run
//!   already produced the retained turn terminal witness): `RefuseRecovery`;
//! - conflicting PERSISTED facts (non-quiescent or undecodable lifecycle
//!   row, or ANY persisted current-run fact — even one naming the
//!   candidate): `RefuseRecovery`;
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

/// Quiescent persisted observations: the shape the shell reports for a cold
/// session whose lifecycle row is a clean Idle with no current run.
fn quiescent_observation() -> (
    mm_dsl::DurableRecoveryObservedLifecycle,
    mm_dsl::DurableRecoveryObservedRun,
) {
    (
        mm_dsl::DurableRecoveryObservedLifecycle::Idle,
        mm_dsl::DurableRecoveryObservedRun::NoRun,
    )
}

/// The durable evidence that admits a commit: no receipt covers the candidate
/// and every non-terminal input row is attributable to the candidate run.
fn admitting_evidence() -> (
    mm_dsl::DurableRecoveryPriorCommit,
    mm_dsl::DurableRecoveryInputEvidence,
) {
    (
        mm_dsl::DurableRecoveryPriorCommit::NoPriorCommit,
        mm_dsl::DurableRecoveryInputEvidence::AllBoundOrInert,
    )
}

#[allow(clippy::too_many_arguments)]
fn recovery_input(
    candidate_id: &str,
    candidate_run_id: &str,
    class: mm_dsl::DurableTailRecoveryClass,
    observed_lifecycle: mm_dsl::DurableRecoveryObservedLifecycle,
    observed_current_run: mm_dsl::DurableRecoveryObservedRun,
    last_committed_sequence: u64,
    prior_commit: mm_dsl::DurableRecoveryPriorCommit,
    input_evidence: mm_dsl::DurableRecoveryInputEvidence,
) -> mm_dsl::MeerkatMachineInput {
    mm_dsl::MeerkatMachineInput::AuthorizeDurableTailRecovery {
        session_id: mm_dsl::SessionId("session-recovery".to_string()),
        candidate_id: candidate_id.to_string(),
        candidate_run_id: mm_dsl::RunId(candidate_run_id.to_string()),
        class,
        observed_lifecycle,
        observed_current_run,
        last_committed_sequence,
        prior_commit,
        input_evidence,
    }
}

/// One authorization verdict: either a commit authorization (with the
/// machine-minted boundary sequence) or a non-commit disposition.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Commit(mm_dsl::DurableTailRecoveryDisposition, u64),
    NonCommit(mm_dsl::DurableTailRecoveryDisposition),
}

#[allow(clippy::too_many_arguments)]
fn authorize(
    authority: &mut mm_dsl::MeerkatMachineAuthority,
    candidate_id: &str,
    candidate_run_id: &str,
    class: mm_dsl::DurableTailRecoveryClass,
    observed_lifecycle: mm_dsl::DurableRecoveryObservedLifecycle,
    observed_current_run: mm_dsl::DurableRecoveryObservedRun,
    last_committed_sequence: u64,
    prior_commit: mm_dsl::DurableRecoveryPriorCommit,
    input_evidence: mm_dsl::DurableRecoveryInputEvidence,
) -> (String, Verdict) {
    let transition = mm_dsl::MeerkatMachineMutator::apply(
        authority,
        recovery_input(
            candidate_id,
            candidate_run_id,
            class,
            observed_lifecycle,
            observed_current_run,
            last_committed_sequence,
            prior_commit,
            input_evidence,
        ),
    )
    .expect("AuthorizeDurableTailRecovery must be total over machine phases");
    let mut verdicts = transition
        .into_effects()
        .into_iter()
        .filter_map(|effect| match effect {
            mm_dsl::MeerkatMachineEffect::DurableTailRecoveryCommitAuthorized {
                candidate_id,
                disposition,
                boundary_sequence,
            } => Some((
                candidate_id,
                Verdict::Commit(disposition, boundary_sequence),
            )),
            mm_dsl::MeerkatMachineEffect::DurableTailRecoveryAuthorized {
                candidate_id,
                disposition,
            } => Some((candidate_id, Verdict::NonCommit(disposition))),
            _ => None,
        });
    let first = verdicts
        .next()
        .expect("authorization must emit exactly one recovery verdict effect");
    assert!(
        verdicts.next().is_none(),
        "authorization must emit exactly one recovery verdict effect"
    );
    first
}

fn authorize_quiescent(
    authority: &mut mm_dsl::MeerkatMachineAuthority,
    candidate_id: &str,
    candidate_run_id: &str,
    class: mm_dsl::DurableTailRecoveryClass,
) -> (String, Verdict) {
    let (lifecycle, run) = quiescent_observation();
    let (prior_commit, input_evidence) = admitting_evidence();
    authorize(
        authority,
        candidate_id,
        candidate_run_id,
        class,
        lifecycle,
        run,
        0,
        prior_commit,
        input_evidence,
    )
}

#[test]
fn idle_quiescent_completed_candidate_commits_completed_with_minted_sequence() {
    let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(quiescent_state(
        mm_dsl::MeerkatPhase::Idle,
    ))
    .expect("idle quiescent state must be recoverable");
    let (candidate_id, verdict) = authorize_quiescent(
        &mut authority,
        "candidate-1",
        "run-1",
        mm_dsl::DurableTailRecoveryClass::CompletedCandidate,
    );
    assert_eq!(candidate_id, "candidate-1");
    assert_eq!(
        verdict,
        Verdict::Commit(mm_dsl::DurableTailRecoveryDisposition::CommitCompleted, 1),
        "no committed receipts (last=0) mints boundary sequence 1"
    );
    assert_eq!(
        authority.state().lifecycle_phase,
        mm_dsl::MeerkatPhase::Idle,
        "authorization must not move the lifecycle"
    );
    assert_eq!(
        authority.state().turn_terminal_run_id,
        Some(mm_dsl::RunId("run-1".to_string())),
        "a commit authorization records the candidate run as the turn terminal"
    );
}

/// An interrupted tool loop can already have committed BoundaryContinue
/// receipts before losing only its final boundary: the machine mints the
/// recovery boundary one past the last durably committed receipt, never a
/// colliding sequence 1.
#[test]
fn committed_receipts_advance_the_minted_sequence() {
    let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(quiescent_state(
        mm_dsl::MeerkatPhase::Idle,
    ))
    .expect("idle quiescent state must be recoverable");
    let (lifecycle, run) = quiescent_observation();
    let (_, verdict) = authorize(
        &mut authority,
        "candidate-seq",
        "run-seq",
        mm_dsl::DurableTailRecoveryClass::CompletedCandidate,
        lifecycle,
        run,
        3,
        // The mid-run boundary records strictly less content than the
        // recovered tail: an ancestor, not a prior recovery.
        mm_dsl::DurableRecoveryPriorCommit::PrecedesCandidate,
        mm_dsl::DurableRecoveryInputEvidence::AllBoundOrInert,
    );
    assert_eq!(
        verdict,
        Verdict::Commit(mm_dsl::DurableTailRecoveryDisposition::CommitCompleted, 4),
        "last committed sequence 3 must mint boundary sequence 4"
    );
}

#[test]
fn retired_quiescent_completed_candidate_commits_completed() {
    let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(quiescent_state(
        mm_dsl::MeerkatPhase::Retired,
    ))
    .expect("retired quiescent state must be recoverable");
    let (candidate_id, verdict) = authorize_quiescent(
        &mut authority,
        "candidate-2",
        "run-2",
        mm_dsl::DurableTailRecoveryClass::CompletedCandidate,
    );
    assert_eq!(candidate_id, "candidate-2");
    assert_eq!(
        verdict,
        Verdict::Commit(mm_dsl::DurableTailRecoveryDisposition::CommitCompleted, 1)
    );
    assert_eq!(
        authority.state().lifecycle_phase,
        mm_dsl::MeerkatPhase::Retired,
        "per_phase decision arms must keep Retired at Retired"
    );
}

#[test]
fn idle_repairable_candidate_repairs_and_commits_interrupted() {
    let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(quiescent_state(
        mm_dsl::MeerkatPhase::Idle,
    ))
    .expect("idle quiescent state must be recoverable");
    let (candidate_id, verdict) = authorize_quiescent(
        &mut authority,
        "candidate-3",
        "run-3",
        mm_dsl::DurableTailRecoveryClass::InterruptedRepairableCandidate,
    );
    assert_eq!(candidate_id, "candidate-3");
    assert_eq!(
        verdict,
        Verdict::Commit(
            mm_dsl::DurableTailRecoveryDisposition::RepairAndCommitInterrupted,
            1
        )
    );
}

/// Legacy adoption: the classifier's `LegacyCompletedCandidate` commits under
/// the same admissible core and durable-evidence guards as the completed
/// class, with the distinct `CommitLegacyCompleted` disposition and the
/// domain-separated legacy run identity recorded as the turn terminal.
#[test]
fn idle_quiescent_legacy_candidate_commits_legacy_with_minted_sequence() {
    let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(quiescent_state(
        mm_dsl::MeerkatPhase::Idle,
    ))
    .expect("idle quiescent state must be recoverable");
    let (candidate_id, verdict) = authorize_quiescent(
        &mut authority,
        "candidate-legacy",
        "run-legacy-deterministic",
        mm_dsl::DurableTailRecoveryClass::LegacyCompletedCandidate,
    );
    assert_eq!(candidate_id, "candidate-legacy");
    assert_eq!(
        verdict,
        Verdict::Commit(
            mm_dsl::DurableTailRecoveryDisposition::CommitLegacyCompleted,
            1
        ),
        "no committed receipts (last=0) mints boundary sequence 1"
    );
    assert_eq!(
        authority.state().turn_terminal_run_id,
        Some(mm_dsl::RunId("run-legacy-deterministic".to_string())),
        "a legacy commit records the minted legacy run identity as the turn terminal"
    );
}

#[test]
fn idle_ambiguous_candidate_holds_intact() {
    let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(quiescent_state(
        mm_dsl::MeerkatPhase::Idle,
    ))
    .expect("idle quiescent state must be recoverable");
    let (candidate_id, verdict) = authorize_quiescent(
        &mut authority,
        "candidate-4",
        "run-4",
        mm_dsl::DurableTailRecoveryClass::Ambiguous,
    );
    assert_eq!(candidate_id, "candidate-4");
    assert_eq!(
        verdict,
        Verdict::NonCommit(mm_dsl::DurableTailRecoveryDisposition::HoldIntact)
    );
}

/// Cross-process duplicate recovery: process A's recovery already landed as a
/// committed boundary carrying this exact candidate's content. Process B —
/// vacuously quiescent in-process, quiescent persisted row, holding a stale
/// snapshot — must NOT mint a second recovered boundary at sequence N+1. The
/// receipt key `(runtime_id, run_id, sequence)` cannot fence that: B observes
/// A's receipt and mints past it. Only the prior-commit observation can.
#[test]
fn prior_commit_covering_the_candidate_refuses_duplicate_recovery() {
    for class in [
        mm_dsl::DurableTailRecoveryClass::CompletedCandidate,
        mm_dsl::DurableTailRecoveryClass::InterruptedRepairableCandidate,
        mm_dsl::DurableTailRecoveryClass::LegacyCompletedCandidate,
    ] {
        let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(quiescent_state(
            mm_dsl::MeerkatPhase::Idle,
        ))
        .expect("idle quiescent state must be recoverable");
        let (lifecycle, run) = quiescent_observation();
        let (_, verdict) = authorize(
            &mut authority,
            "candidate-duplicate",
            "run-duplicate",
            class,
            lifecycle,
            run,
            7,
            mm_dsl::DurableRecoveryPriorCommit::MatchesCandidate,
            mm_dsl::DurableRecoveryInputEvidence::AllBoundOrInert,
        );
        assert_eq!(
            verdict,
            Verdict::NonCommit(mm_dsl::DurableTailRecoveryDisposition::RefuseRecovery),
            "class {class:?}: a recovery that already landed must not re-commit"
        );
        assert_eq!(
            authority.state().turn_terminal_run_id,
            None,
            "a refusal must not record any terminal fact"
        );
        assert_eq!(
            authority.state().recovered_boundary_sequence,
            0,
            "a refusal must not mint a recovery boundary sequence"
        );
    }
}

/// A committed boundary the candidate neither extends nor equals: the run's
/// durable history and the candidate disagree and no observation here can say
/// which continuation is truthful. Fail closed.
#[test]
fn diverging_prior_commit_refuses_recovery() {
    let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(quiescent_state(
        mm_dsl::MeerkatPhase::Idle,
    ))
    .expect("idle quiescent state must be recoverable");
    let (lifecycle, run) = quiescent_observation();
    let (_, verdict) = authorize(
        &mut authority,
        "candidate-diverged",
        "run-diverged",
        mm_dsl::DurableTailRecoveryClass::CompletedCandidate,
        lifecycle,
        run,
        2,
        mm_dsl::DurableRecoveryPriorCommit::DivergesFromCandidate,
        mm_dsl::DurableRecoveryInputEvidence::AllBoundOrInert,
    );
    assert_eq!(
        verdict,
        Verdict::NonCommit(mm_dsl::DurableTailRecoveryDisposition::RefuseRecovery)
    );
}

/// Input evidence the machine — not the shell — turns into a hold: blocking
/// rows the store cannot fence hold for every commit-seeking class, and an
/// unattributable redeliverable input holds for the INTERRUPTED shape. No
/// commit authorization is emitted on any hold, so no shell can be handed a
/// commit verdict it then downgrades. (The COMPLETED shapes under
/// `UnboundContentInput` belong to the retain-inputs commit, pinned below.)
#[test]
fn unattributable_input_evidence_holds_intact() {
    let cases = [
        (
            mm_dsl::DurableRecoveryInputEvidence::Unfenceable,
            mm_dsl::DurableTailRecoveryClass::CompletedCandidate,
        ),
        (
            mm_dsl::DurableRecoveryInputEvidence::Unfenceable,
            mm_dsl::DurableTailRecoveryClass::InterruptedRepairableCandidate,
        ),
        (
            mm_dsl::DurableRecoveryInputEvidence::Unfenceable,
            mm_dsl::DurableTailRecoveryClass::LegacyCompletedCandidate,
        ),
        (
            mm_dsl::DurableRecoveryInputEvidence::UnboundContentInput,
            mm_dsl::DurableTailRecoveryClass::InterruptedRepairableCandidate,
        ),
    ];
    for (evidence, class) in cases {
        let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(quiescent_state(
            mm_dsl::MeerkatPhase::Idle,
        ))
        .expect("idle quiescent state must be recoverable");
        let (lifecycle, run) = quiescent_observation();
        let (candidate_id, verdict) = authorize(
            &mut authority,
            "candidate-inputs",
            "run-inputs",
            class,
            lifecycle,
            run,
            0,
            mm_dsl::DurableRecoveryPriorCommit::NoPriorCommit,
            evidence,
        );
        assert_eq!(candidate_id, "candidate-inputs");
        assert_eq!(
            verdict,
            Verdict::NonCommit(mm_dsl::DurableTailRecoveryDisposition::HoldIntact),
            "{evidence:?} with class {class:?} must hold intact"
        );
        assert_eq!(
            authority.state().turn_terminal_run_id,
            None,
            "a hold must not record the candidate run as terminalized"
        );
    }
}

/// The retain-inputs commit: a clean COMPLETED candidate with an unbound
/// content input commits WITHOUT terminalizing the unbound row — it is
/// retained for ordinary redelivery instead of wedging the session forever.
/// The arm carries no writer-era guard: a pre-0.8.9 writer's lost boundary
/// routinely leaves the executed turn's own input unbound (redelivery costs
/// at most one duplicate turn — the legacy fleet's own restart semantics),
/// and a 0.8.9+ writer fences staged bindings durable before execution, so
/// an unbound content row never started and redelivery is simply correct.
#[test]
fn unbound_content_input_with_completed_shape_commits_retaining_inputs() {
    for class in [
        mm_dsl::DurableTailRecoveryClass::CompletedCandidate,
        mm_dsl::DurableTailRecoveryClass::LegacyCompletedCandidate,
    ] {
        let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(quiescent_state(
            mm_dsl::MeerkatPhase::Idle,
        ))
        .expect("idle quiescent state must be recoverable");
        let (lifecycle, run) = quiescent_observation();
        let (candidate_id, verdict) = authorize(
            &mut authority,
            "candidate-retain-inputs",
            "run-retain-bound",
            class,
            lifecycle,
            run,
            0,
            mm_dsl::DurableRecoveryPriorCommit::NoPriorCommit,
            mm_dsl::DurableRecoveryInputEvidence::UnboundContentInput,
        );
        assert_eq!(candidate_id, "candidate-retain-inputs");
        assert_eq!(
            verdict,
            Verdict::Commit(
                mm_dsl::DurableTailRecoveryDisposition::CommitLegacyCompletedRetainInputs,
                1
            ),
            "class {class:?}: an unbound content input on a completed shape must \
             commit retaining inputs"
        );
        assert_eq!(
            authority.state().turn_terminal_run_id,
            Some(mm_dsl::RunId("run-retain-bound".to_string())),
            "the retain-inputs commit still records the candidate run as the turn terminal"
        );
    }
}

/// The retain-inputs arm is exactly as narrow as the evidence allows: an
/// interrupted shape stays held, unfenceable rows stay held for every class,
/// and a prior-commit conflict refuses before the evidence cut is ever
/// consulted.
#[test]
fn retain_inputs_arm_stays_fail_closed_everywhere_else() {
    // Interrupted shape: held.
    let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(quiescent_state(
        mm_dsl::MeerkatPhase::Idle,
    ))
    .expect("idle quiescent state must be recoverable");
    let (lifecycle, run) = quiescent_observation();
    let (_, verdict) = authorize(
        &mut authority,
        "candidate-retain-interrupted",
        "run-retain-interrupted",
        mm_dsl::DurableTailRecoveryClass::InterruptedRepairableCandidate,
        lifecycle,
        run,
        0,
        mm_dsl::DurableRecoveryPriorCommit::NoPriorCommit,
        mm_dsl::DurableRecoveryInputEvidence::UnboundContentInput,
    );
    assert_eq!(
        verdict,
        Verdict::NonCommit(mm_dsl::DurableTailRecoveryDisposition::HoldIntact),
        "an interrupted tail with an unbound input must stay held"
    );

    // Unfenceable rows: held for every commit-seeking class.
    for class in [
        mm_dsl::DurableTailRecoveryClass::CompletedCandidate,
        mm_dsl::DurableTailRecoveryClass::InterruptedRepairableCandidate,
        mm_dsl::DurableTailRecoveryClass::LegacyCompletedCandidate,
    ] {
        let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(quiescent_state(
            mm_dsl::MeerkatPhase::Idle,
        ))
        .expect("idle quiescent state must be recoverable");
        let (lifecycle, run) = quiescent_observation();
        let (_, verdict) = authorize(
            &mut authority,
            "candidate-retain-unfenceable",
            "run-retain-unfenceable",
            class,
            lifecycle,
            run,
            0,
            mm_dsl::DurableRecoveryPriorCommit::NoPriorCommit,
            mm_dsl::DurableRecoveryInputEvidence::Unfenceable,
        );
        assert_eq!(
            verdict,
            Verdict::NonCommit(mm_dsl::DurableTailRecoveryDisposition::HoldIntact),
            "class {class:?}: unfenceable rows must hold"
        );
    }

    // Prior-commit conflict refuses before the evidence cut.
    let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(quiescent_state(
        mm_dsl::MeerkatPhase::Idle,
    ))
    .expect("idle quiescent state must be recoverable");
    let (lifecycle, run) = quiescent_observation();
    let (_, verdict) = authorize(
        &mut authority,
        "candidate-retain-duplicate",
        "run-retain-duplicate",
        mm_dsl::DurableTailRecoveryClass::CompletedCandidate,
        lifecycle,
        run,
        4,
        mm_dsl::DurableRecoveryPriorCommit::MatchesCandidate,
        mm_dsl::DurableRecoveryInputEvidence::UnboundContentInput,
    );
    assert_eq!(
        verdict,
        Verdict::NonCommit(mm_dsl::DurableTailRecoveryDisposition::RefuseRecovery),
        "an already-landed recovery must refuse before the input-evidence cut"
    );
}

/// An ambiguous classification holds whatever the receipts and input rows
/// say: the ambiguous arm absorbs the whole class slice, so the evidence cuts
/// never reach it.
#[test]
fn ambiguous_class_holds_regardless_of_durable_evidence() {
    for prior_commit in [
        mm_dsl::DurableRecoveryPriorCommit::NoPriorCommit,
        mm_dsl::DurableRecoveryPriorCommit::PrecedesCandidate,
        mm_dsl::DurableRecoveryPriorCommit::MatchesCandidate,
        mm_dsl::DurableRecoveryPriorCommit::DivergesFromCandidate,
    ] {
        for evidence in [
            mm_dsl::DurableRecoveryInputEvidence::AllBoundOrInert,
            mm_dsl::DurableRecoveryInputEvidence::UnboundContentInput,
            mm_dsl::DurableRecoveryInputEvidence::Unfenceable,
        ] {
            let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(
                quiescent_state(mm_dsl::MeerkatPhase::Idle),
            )
            .expect("idle quiescent state must be recoverable");
            let (lifecycle, run) = quiescent_observation();
            let (_, verdict) = authorize(
                &mut authority,
                "candidate-ambiguous",
                "run-ambiguous",
                mm_dsl::DurableTailRecoveryClass::Ambiguous,
                lifecycle,
                run,
                0,
                prior_commit,
                evidence,
            );
            assert_eq!(
                verdict,
                Verdict::NonCommit(mm_dsl::DurableTailRecoveryDisposition::HoldIntact),
                "ambiguous with ({prior_commit:?}, {evidence:?}) must hold intact"
            );
        }
    }
}

/// The reviewer's exact case: an in-process authority that looks vacuously
/// quiescent (freshly registered) must still refuse when the PERSISTED row
/// carries conflicting facts — a non-quiescent phase, an undecodable row, or
/// any current-run fact, even one naming the candidate.
#[test]
fn conflicting_persisted_facts_refuse_recovery() {
    let cases = [
        (
            mm_dsl::DurableRecoveryObservedLifecycle::NonQuiescent,
            mm_dsl::DurableRecoveryObservedRun::NoRun,
        ),
        (
            mm_dsl::DurableRecoveryObservedLifecycle::Undecodable,
            mm_dsl::DurableRecoveryObservedRun::NoRun,
        ),
        (
            mm_dsl::DurableRecoveryObservedLifecycle::Retired,
            mm_dsl::DurableRecoveryObservedRun::CandidateRun,
        ),
        (
            mm_dsl::DurableRecoveryObservedLifecycle::Idle,
            mm_dsl::DurableRecoveryObservedRun::OtherRun,
        ),
    ];
    for (lifecycle, run) in cases {
        let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(quiescent_state(
            mm_dsl::MeerkatPhase::Idle,
        ))
        .expect("idle quiescent state must be recoverable");
        let (prior_commit, input_evidence) = admitting_evidence();
        let (_, verdict) = authorize(
            &mut authority,
            "candidate-persisted",
            "run-persisted",
            mm_dsl::DurableTailRecoveryClass::CompletedCandidate,
            lifecycle,
            run,
            0,
            prior_commit,
            input_evidence,
        );
        assert_eq!(
            verdict,
            Verdict::NonCommit(mm_dsl::DurableTailRecoveryDisposition::RefuseRecovery),
            "persisted facts ({lifecycle:?}, {run:?}) must refuse recovery"
        );
        assert_eq!(
            authority.state().turn_terminal_run_id,
            None,
            "a refusal must not record any terminal fact"
        );
    }
}

/// Missing lifecycle rows are quiescent by absence and commit.
#[test]
fn missing_persisted_row_is_quiescent_by_absence() {
    let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(quiescent_state(
        mm_dsl::MeerkatPhase::Idle,
    ))
    .expect("idle quiescent state must be recoverable");
    let (prior_commit, input_evidence) = admitting_evidence();
    let (_, verdict) = authorize(
        &mut authority,
        "candidate-missing",
        "run-missing",
        mm_dsl::DurableTailRecoveryClass::CompletedCandidate,
        mm_dsl::DurableRecoveryObservedLifecycle::MissingRow,
        mm_dsl::DurableRecoveryObservedRun::NoRun,
        0,
        prior_commit,
        input_evidence,
    );
    assert_eq!(
        verdict,
        Verdict::Commit(mm_dsl::DurableTailRecoveryDisposition::CommitCompleted, 1)
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
    let (candidate_id, verdict) = authorize_quiescent(
        &mut authority,
        "candidate-5",
        "run-other",
        mm_dsl::DurableTailRecoveryClass::CompletedCandidate,
    );
    assert_eq!(candidate_id, "candidate-5");
    assert_eq!(
        verdict,
        Verdict::NonCommit(mm_dsl::DurableTailRecoveryDisposition::RefuseRecovery),
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
    let (candidate_id, verdict) = authorize_quiescent(
        &mut authority,
        "candidate-6",
        "run-terminal",
        mm_dsl::DurableTailRecoveryClass::CompletedCandidate,
    );
    assert_eq!(candidate_id, "candidate-6");
    assert_eq!(
        verdict,
        Verdict::NonCommit(mm_dsl::DurableTailRecoveryDisposition::RefuseRecovery),
        "an already-terminalized candidate run must refuse (idempotence guard)"
    );

    // The refusal is about the EXACT run: a different candidate run against
    // the same machine state commits.
    let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(seed())
        .expect("idle state with a terminal witness must be recoverable");
    let (_, verdict) = authorize_quiescent(
        &mut authority,
        "candidate-7",
        "run-different",
        mm_dsl::DurableTailRecoveryClass::CompletedCandidate,
    );
    assert_eq!(
        verdict,
        Verdict::Commit(mm_dsl::DurableTailRecoveryDisposition::CommitCompleted, 1)
    );
}

/// Every non-quiescent lifecycle phase refuses recovery regardless of the
/// classification and regardless of quiescent-looking persisted facts.
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
            mm_dsl::DurableTailRecoveryClass::LegacyCompletedCandidate,
            mm_dsl::DurableTailRecoveryClass::Ambiguous,
        ] {
            let mut authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(state.clone())
                .unwrap_or_else(|err| panic!("{phase:?} seed state must be recoverable: {err:?}"));
            let (candidate_id, verdict) =
                authorize_quiescent(&mut authority, "candidate-8", "run-8", class);
            assert_eq!(candidate_id, "candidate-8");
            assert_eq!(
                verdict,
                Verdict::NonCommit(mm_dsl::DurableTailRecoveryDisposition::RefuseRecovery),
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
        mm_dsl::DurableTailRecoveryDisposition::CommitLegacyCompleted,
        mm_dsl::DurableTailRecoveryDisposition::CommitLegacyCompletedRetainInputs,
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
            // The legacy tail IS the committed transcript, adopted under the
            // minted legacy run identity.
            mm_dsl::DurableTailRecoveryDisposition::CommitLegacyCompleted => true,
            // The tail IS the committed transcript; the unbound input row is
            // retained in its own lifecycle for redelivery — nothing is
            // terminalized, nothing discarded.
            mm_dsl::DurableTailRecoveryDisposition::CommitLegacyCompletedRetainInputs => true,
            // Held intact, readable, blocked from resume.
            mm_dsl::DurableTailRecoveryDisposition::HoldIntact => true,
        };
        assert!(
            retains_every_durable_message,
            "{disposition:?} must retain the durable tail"
        );
    }
}
