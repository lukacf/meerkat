//! Level 1 — SessionDocumentMachine durable-tail classification truth table.
//!
//! Drives the generated authority (`SessionDocumentMachineAuthority`)
//! directly: the shell mechanically encodes transcript structure, the MACHINE
//! assigns the recovery class. These tests pin the full `ClassifyDurableTail`
//! truth table:
//!
//! - Completed: VerifiedStrictDescendant + SingleRunId + EndTurn + 0 dangling
//!   + 0 orphans + nothing-after-terminal -> `CompletedCandidate`.
//! - Repairable: descendant + SingleRunId + coherent + 0 dangling +
//!   (ToolUse | Absent) -> `InterruptedRepairableCandidate`.
//! - Everything else — INCLUDING any tail carrying a dangling tool_use ->
//!   `Ambiguous` (fail-closed: hold, never discard, never auto-close).
//!
//! A dangling tool_use proves INTENT, not execution: the call may have fired
//! its external side effect (a payment, an email, a file write) before the
//! crash, and nothing in the transcript can distinguish that from
//! never-dispatched. Auto-closing such a tail with synthetic results and
//! resuming normal autonomy would let the agent repeat an executed action, so
//! the machine holds it for reconciliation instead.
//!
//! The classification transitions are guaranteed total and disjoint: for every
//! input combination exactly one transition matches (the generated authority
//! errors on zero or multiple matches), which the exhaustive sweep pins.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use meerkat_core::session_document::{
    DurableHeadRelation, DurableTailRecoveryClass, DurableTailStopReason, RunIdCardinality,
    SessionDocumentEffect, SessionDocumentKey, SessionDocumentMachineAuthority,
};

/// Drive one `ClassifyDurableTail` input through a fresh generated authority
/// and return the single emitted `DurableTailClassified` effect payload.
#[allow(clippy::too_many_arguments)]
fn classify_with_candidate(
    candidate_id: &str,
    relation: DurableHeadRelation,
    run_id_cardinality: RunIdCardinality,
    terminal_stop_reason: DurableTailStopReason,
    dangling_tool_use_count: u64,
    orphan_tool_result_count: u64,
    messages_after_terminal: bool,
) -> (String, DurableTailRecoveryClass) {
    let mut authority = SessionDocumentMachineAuthority::new();
    let effects = authority
        .classify_durable_tail(
            SessionDocumentKey::new("session-recovery"),
            candidate_id.to_string(),
            relation,
            run_id_cardinality,
            terminal_stop_reason,
            dangling_tool_use_count,
            orphan_tool_result_count,
            messages_after_terminal,
        )
        .expect("ClassifyDurableTail must be total over the input domain");
    let mut classified = effects.into_iter().filter_map(|effect| match effect {
        SessionDocumentEffect::DurableTailClassified {
            candidate_id,
            class,
        } => Some((candidate_id, class)),
        _ => None,
    });
    let first = classified
        .next()
        .expect("classification must emit a DurableTailClassified effect");
    assert!(
        classified.next().is_none(),
        "classification must emit exactly one DurableTailClassified effect"
    );
    first
}

fn classify(
    relation: DurableHeadRelation,
    run_id_cardinality: RunIdCardinality,
    terminal_stop_reason: DurableTailStopReason,
    dangling_tool_use_count: u64,
    orphan_tool_result_count: u64,
    messages_after_terminal: bool,
) -> DurableTailRecoveryClass {
    classify_with_candidate(
        "candidate-under-test",
        relation,
        run_id_cardinality,
        terminal_stop_reason,
        dangling_tool_use_count,
        orphan_tool_result_count,
        messages_after_terminal,
    )
    .1
}

// ---------------------------------------------------------------------------
// Completed row
// ---------------------------------------------------------------------------

/// Completed: verified strict descendant, one run id, EndTurn terminal, no
/// dangling tool_use, no orphan tool_result, nothing after the terminal.
#[test]
fn completed_row_classifies_completed_candidate() {
    assert_eq!(
        classify(
            DurableHeadRelation::VerifiedStrictDescendant,
            RunIdCardinality::SingleRunId,
            DurableTailStopReason::EndTurn,
            0,
            0,
            false,
        ),
        DurableTailRecoveryClass::CompletedCandidate
    );
}

// ---------------------------------------------------------------------------
// Repairable rows
// ---------------------------------------------------------------------------

/// Repairable ONLY with zero dangling calls: a ToolUse terminal whose issued
/// calls all carry durable results is a turn that lost its follow-up, and
/// closing it invents no execution truth. With ANY dangling call the tail
/// holds instead — the call's external effect may already have happened.
#[test]
fn tool_use_terminal_is_repairable_only_without_dangling_calls() {
    assert_eq!(
        classify(
            DurableHeadRelation::VerifiedStrictDescendant,
            RunIdCardinality::SingleRunId,
            DurableTailStopReason::ToolUse,
            0,
            0,
            false,
        ),
        DurableTailRecoveryClass::InterruptedRepairableCandidate,
        "ToolUse terminal with every result landed must be repairable"
    );
    for dangling in [1, 3] {
        assert_eq!(
            classify(
                DurableHeadRelation::VerifiedStrictDescendant,
                RunIdCardinality::SingleRunId,
                DurableTailStopReason::ToolUse,
                dangling,
                0,
                false,
            ),
            DurableTailRecoveryClass::Ambiguous,
            "ToolUse terminal with {dangling} dangling tool_use must hold for \
             reconciliation, never auto-close"
        );
    }
}

/// Same rule at an Absent terminal (turn cut mid-stream before any stop
/// reason): repairable with zero dangling calls, held with any.
#[test]
fn absent_terminal_is_repairable_only_without_dangling_calls() {
    assert_eq!(
        classify(
            DurableHeadRelation::VerifiedStrictDescendant,
            RunIdCardinality::SingleRunId,
            DurableTailStopReason::Absent,
            0,
            0,
            false,
        ),
        DurableTailRecoveryClass::InterruptedRepairableCandidate,
        "Absent terminal with no dangling calls must be repairable"
    );
    for dangling in [1, 3] {
        assert_eq!(
            classify(
                DurableHeadRelation::VerifiedStrictDescendant,
                RunIdCardinality::SingleRunId,
                DurableTailStopReason::Absent,
                dangling,
                0,
                false,
            ),
            DurableTailRecoveryClass::Ambiguous,
            "Absent terminal with {dangling} dangling tool_use must hold for \
             reconciliation, never auto-close"
        );
    }
}

/// The unknown-external-effect rule stated directly: for EVERY otherwise
/// coherent shape, one dangling tool_use is enough to hold the tail.
#[test]
fn any_dangling_tool_use_holds_the_tail_for_reconciliation() {
    for stop in [
        DurableTailStopReason::EndTurn,
        DurableTailStopReason::ToolUse,
        DurableTailStopReason::Absent,
        DurableTailStopReason::Other,
    ] {
        assert_eq!(
            classify(
                DurableHeadRelation::VerifiedStrictDescendant,
                RunIdCardinality::SingleRunId,
                stop,
                1,
                0,
                false,
            ),
            DurableTailRecoveryClass::Ambiguous,
            "a dangling tool_use at stop {stop:?} proves intent, not execution: hold"
        );
    }
}

// ---------------------------------------------------------------------------
// Ambiguous rows — one test each
// ---------------------------------------------------------------------------

/// Any non-descendant relation is Ambiguous, even with an otherwise perfect
/// completed shape: digest continuity is the precondition for everything.
#[test]
fn non_descendant_relation_is_ambiguous() {
    for relation in [
        DurableHeadRelation::AbsentOrExact,
        DurableHeadRelation::RuntimeSnapshotAhead,
        DurableHeadRelation::Diverged,
        DurableHeadRelation::Unverifiable,
    ] {
        assert_eq!(
            classify(
                relation,
                RunIdCardinality::SingleRunId,
                DurableTailStopReason::EndTurn,
                0,
                0,
                false,
            ),
            DurableTailRecoveryClass::Ambiguous,
            "relation {relation:?} must be ambiguous"
        );
    }
}

/// A tail with no run identity cannot bind to machine run facts: Ambiguous.
#[test]
fn no_run_id_is_ambiguous() {
    assert_eq!(
        classify(
            DurableHeadRelation::VerifiedStrictDescendant,
            RunIdCardinality::NoRunId,
            DurableTailStopReason::EndTurn,
            0,
            0,
            false,
        ),
        DurableTailRecoveryClass::Ambiguous
    );
}

/// A tail spanning multiple runs is not a single recoverable boundary:
/// Ambiguous.
#[test]
fn multiple_run_ids_is_ambiguous() {
    assert_eq!(
        classify(
            DurableHeadRelation::VerifiedStrictDescendant,
            RunIdCardinality::MultipleRunIds,
            DurableTailStopReason::EndTurn,
            0,
            0,
            false,
        ),
        DurableTailRecoveryClass::Ambiguous
    );
}

/// An orphan tool_result (result without its call) is structurally
/// contradictory evidence: Ambiguous, never auto-repaired.
#[test]
fn orphan_tool_result_is_ambiguous() {
    for stop in [
        DurableTailStopReason::EndTurn,
        DurableTailStopReason::ToolUse,
    ] {
        assert_eq!(
            classify(
                DurableHeadRelation::VerifiedStrictDescendant,
                RunIdCardinality::SingleRunId,
                stop,
                0,
                1,
                false,
            ),
            DurableTailRecoveryClass::Ambiguous,
            "one orphan tool_result must force ambiguity under {stop:?}"
        );
    }
}

/// Messages recorded after the terminal message contradict the terminal:
/// Ambiguous.
#[test]
fn messages_after_terminal_is_ambiguous() {
    assert_eq!(
        classify(
            DurableHeadRelation::VerifiedStrictDescendant,
            RunIdCardinality::SingleRunId,
            DurableTailStopReason::EndTurn,
            0,
            0,
            true,
        ),
        DurableTailRecoveryClass::Ambiguous
    );
}

/// An unrecognized stop reason class carries no completion meaning: Ambiguous.
#[test]
fn other_stop_reason_is_ambiguous() {
    assert_eq!(
        classify(
            DurableHeadRelation::VerifiedStrictDescendant,
            RunIdCardinality::SingleRunId,
            DurableTailStopReason::Other,
            0,
            0,
            false,
        ),
        DurableTailRecoveryClass::Ambiguous
    );
}

/// EndTurn claiming completion while a tool_use dangles is contradictory
/// evidence: Ambiguous, NOT completed and NOT repairable.
#[test]
fn end_turn_with_dangling_tool_use_is_ambiguous() {
    assert_eq!(
        classify(
            DurableHeadRelation::VerifiedStrictDescendant,
            RunIdCardinality::SingleRunId,
            DurableTailStopReason::EndTurn,
            1,
            0,
            false,
        ),
        DurableTailRecoveryClass::Ambiguous
    );
}

// ---------------------------------------------------------------------------
// Candidate binding
// ---------------------------------------------------------------------------

/// The emitted effect must carry the EXACT candidate id passed in — the
/// candidate binds the classification to one observed head so it can never
/// authorize mutation of a later head.
#[test]
fn effect_carries_exact_candidate_id() {
    let candidate = "candidate:session-recovery:rev385:cas42:run-7f";
    let (echoed, class) = classify_with_candidate(
        candidate,
        DurableHeadRelation::VerifiedStrictDescendant,
        RunIdCardinality::SingleRunId,
        DurableTailStopReason::EndTurn,
        0,
        0,
        false,
    );
    assert_eq!(echoed, candidate);
    assert_eq!(class, DurableTailRecoveryClass::CompletedCandidate);

    // The binding holds on the ambiguous arm too.
    let (echoed, class) = classify_with_candidate(
        candidate,
        DurableHeadRelation::Diverged,
        RunIdCardinality::SingleRunId,
        DurableTailStopReason::EndTurn,
        0,
        0,
        false,
    );
    assert_eq!(echoed, candidate);
    assert_eq!(class, DurableTailRecoveryClass::Ambiguous);
}

// ---------------------------------------------------------------------------
// Totality + disjointness sweep
// ---------------------------------------------------------------------------

/// Every combination in the input domain classifies successfully with exactly
/// one verdict (the generated authority errors on zero or multiple matching
/// transitions), and the verdict agrees with the independently-stated truth
/// table oracle.
#[test]
fn classification_is_total_disjoint_and_matches_the_truth_table() {
    let relations = [
        DurableHeadRelation::AbsentOrExact,
        DurableHeadRelation::RuntimeSnapshotAhead,
        DurableHeadRelation::VerifiedStrictDescendant,
        DurableHeadRelation::Diverged,
        DurableHeadRelation::Unverifiable,
    ];
    let cardinalities = [
        RunIdCardinality::NoRunId,
        RunIdCardinality::SingleRunId,
        RunIdCardinality::MultipleRunIds,
    ];
    let stops = [
        DurableTailStopReason::Absent,
        DurableTailStopReason::EndTurn,
        DurableTailStopReason::ToolUse,
        DurableTailStopReason::Other,
    ];

    for relation in relations {
        for cardinality in cardinalities {
            for stop in stops {
                for dangling in [0u64, 1, 3] {
                    for orphans in [0u64, 1] {
                        for after_terminal in [false, true] {
                            let got = classify(
                                relation,
                                cardinality,
                                stop,
                                dangling,
                                orphans,
                                after_terminal,
                            );
                            let coherent = relation
                                == DurableHeadRelation::VerifiedStrictDescendant
                                && cardinality == RunIdCardinality::SingleRunId
                                && orphans == 0
                                && !after_terminal;
                            // A dangling tool_use disqualifies BOTH commit
                            // classes: unknown external effects are held.
                            let expected = if coherent && dangling == 0 {
                                match stop {
                                    DurableTailStopReason::EndTurn => {
                                        DurableTailRecoveryClass::CompletedCandidate
                                    }
                                    DurableTailStopReason::ToolUse
                                    | DurableTailStopReason::Absent => {
                                        DurableTailRecoveryClass::InterruptedRepairableCandidate
                                    }
                                    DurableTailStopReason::Other => {
                                        DurableTailRecoveryClass::Ambiguous
                                    }
                                }
                            } else {
                                DurableTailRecoveryClass::Ambiguous
                            };
                            assert_eq!(
                                got, expected,
                                "truth-table mismatch at relation={relation:?} \
                                 cardinality={cardinality:?} stop={stop:?} \
                                 dangling={dangling} orphans={orphans} \
                                 after_terminal={after_terminal}"
                            );
                        }
                    }
                }
            }
        }
    }
}
