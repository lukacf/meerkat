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
//! - Legacy: descendant + NoRunId + EndTurn + 0 dangling + 0 orphans +
//!   nothing-after-terminal + PRE-WITNESS-V3 stamp era ->
//!   `LegacyCompletedCandidate` (a pre-run-identity writer wrote the tail;
//!   no run id can ever appear, so the clean completed shape is adopted).
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
    DurableHeadRelation, DurableHeadStampEra, DurableTailRecoveryClass, DurableTailStopReason,
    RunIdCardinality, SessionDocumentEffect, SessionDocumentKey, SessionDocumentMachineAuthority,
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
    head_stamp_era: DurableHeadStampEra,
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
            head_stamp_era,
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

/// Classify under the MODERN stamp era — the fail-closed default every
/// pre-existing truth-table row was stated under.
fn classify(
    relation: DurableHeadRelation,
    run_id_cardinality: RunIdCardinality,
    terminal_stop_reason: DurableTailStopReason,
    dangling_tool_use_count: u64,
    orphan_tool_result_count: u64,
    messages_after_terminal: bool,
) -> DurableTailRecoveryClass {
    classify_with_era(
        relation,
        run_id_cardinality,
        terminal_stop_reason,
        dangling_tool_use_count,
        orphan_tool_result_count,
        messages_after_terminal,
        DurableHeadStampEra::WitnessV3OrNewer,
    )
}

fn classify_with_era(
    relation: DurableHeadRelation,
    run_id_cardinality: RunIdCardinality,
    terminal_stop_reason: DurableTailStopReason,
    dangling_tool_use_count: u64,
    orphan_tool_result_count: u64,
    messages_after_terminal: bool,
    head_stamp_era: DurableHeadStampEra,
) -> DurableTailRecoveryClass {
    classify_with_candidate(
        "candidate-under-test",
        relation,
        run_id_cardinality,
        terminal_stop_reason,
        dangling_tool_use_count,
        orphan_tool_result_count,
        messages_after_terminal,
        head_stamp_era,
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

/// A tail with no run identity on a MODERN-era row cannot bind to machine
/// run facts: Ambiguous. (Modern writers persist run identity inside the
/// same message bytes as in-run assistant content, so an identity-less
/// modern tail is contradictory evidence, not a legacy shape.)
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

// ---------------------------------------------------------------------------
// Legacy adoption row
// ---------------------------------------------------------------------------

/// Legacy adoption: an identity-less clean completed tail on a
/// PRE-WITNESS-V3 row classifies `LegacyCompletedCandidate` — the writer
/// predates run-identity bookkeeping, so no run id can ever appear and
/// holding for one is a permanent availability loss.
#[test]
fn legacy_era_no_run_id_clean_end_turn_is_legacy_completed_candidate() {
    assert_eq!(
        classify_with_era(
            DurableHeadRelation::VerifiedStrictDescendant,
            RunIdCardinality::NoRunId,
            DurableTailStopReason::EndTurn,
            0,
            0,
            false,
            DurableHeadStampEra::PreWitnessV3,
        ),
        DurableTailRecoveryClass::LegacyCompletedCandidate
    );
}

/// The legacy arm requires EVERY conjunct: flipping any single axis off the
/// clean completed legacy shape falls back to the fail-closed class that
/// axis previously produced.
#[test]
fn legacy_adoption_requires_every_conjunct() {
    // A run id present -> the EXISTING completed path, never the legacy arm.
    assert_eq!(
        classify_with_era(
            DurableHeadRelation::VerifiedStrictDescendant,
            RunIdCardinality::SingleRunId,
            DurableTailStopReason::EndTurn,
            0,
            0,
            false,
            DurableHeadStampEra::PreWitnessV3,
        ),
        DurableTailRecoveryClass::CompletedCandidate,
        "a run-id-bearing tail on a legacy-era row takes the modern completed path"
    );
    // Modern stamp era -> held exactly as before the legacy arm existed.
    assert_eq!(
        classify_with_era(
            DurableHeadRelation::VerifiedStrictDescendant,
            RunIdCardinality::NoRunId,
            DurableTailStopReason::EndTurn,
            0,
            0,
            false,
            DurableHeadStampEra::WitnessV3OrNewer,
        ),
        DurableTailRecoveryClass::Ambiguous,
        "an identity-less tail without legacy stamp evidence stays held"
    );
    // Interrupted legacy shapes stay held: no repairable arm exists for a
    // tail that cannot bind a run identity.
    for stop in [
        DurableTailStopReason::ToolUse,
        DurableTailStopReason::Absent,
        DurableTailStopReason::Other,
    ] {
        assert_eq!(
            classify_with_era(
                DurableHeadRelation::VerifiedStrictDescendant,
                RunIdCardinality::NoRunId,
                stop,
                0,
                0,
                false,
                DurableHeadStampEra::PreWitnessV3,
            ),
            DurableTailRecoveryClass::Ambiguous,
            "an interrupted legacy tail (stop {stop:?}) must stay held"
        );
    }
    // Tool-racing or trailing shapes stay held.
    for (dangling, orphans, after) in [(1u64, 0u64, false), (0, 1, false), (0, 0, true)] {
        assert_eq!(
            classify_with_era(
                DurableHeadRelation::VerifiedStrictDescendant,
                RunIdCardinality::NoRunId,
                DurableTailStopReason::EndTurn,
                dangling,
                orphans,
                after,
                DurableHeadStampEra::PreWitnessV3,
            ),
            DurableTailRecoveryClass::Ambiguous,
            "unclean legacy shape (dangling={dangling} orphans={orphans} after={after}) \
             must stay held"
        );
    }
    // Multiple runs never adopt, whatever the era.
    assert_eq!(
        classify_with_era(
            DurableHeadRelation::VerifiedStrictDescendant,
            RunIdCardinality::MultipleRunIds,
            DurableTailStopReason::EndTurn,
            0,
            0,
            false,
            DurableHeadStampEra::PreWitnessV3,
        ),
        DurableTailRecoveryClass::Ambiguous
    );
    // Non-descendant relations never adopt, whatever the era.
    assert_eq!(
        classify_with_era(
            DurableHeadRelation::Diverged,
            RunIdCardinality::NoRunId,
            DurableTailStopReason::EndTurn,
            0,
            0,
            false,
            DurableHeadStampEra::PreWitnessV3,
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
        DurableHeadStampEra::WitnessV3OrNewer,
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
        DurableHeadStampEra::WitnessV3OrNewer,
    );
    assert_eq!(echoed, candidate);
    assert_eq!(class, DurableTailRecoveryClass::Ambiguous);

    // And on the legacy adoption arm.
    let (echoed, class) = classify_with_candidate(
        candidate,
        DurableHeadRelation::VerifiedStrictDescendant,
        RunIdCardinality::NoRunId,
        DurableTailStopReason::EndTurn,
        0,
        0,
        false,
        DurableHeadStampEra::PreWitnessV3,
    );
    assert_eq!(echoed, candidate);
    assert_eq!(class, DurableTailRecoveryClass::LegacyCompletedCandidate);
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

    let eras = [
        DurableHeadStampEra::WitnessV3OrNewer,
        DurableHeadStampEra::PreWitnessV3,
    ];

    for relation in relations {
        for cardinality in cardinalities {
            for stop in stops {
                for dangling in [0u64, 1, 3] {
                    for orphans in [0u64, 1] {
                        for after_terminal in [false, true] {
                            for era in eras {
                                let got = classify_with_era(
                                    relation,
                                    cardinality,
                                    stop,
                                    dangling,
                                    orphans,
                                    after_terminal,
                                    era,
                                );
                                let coherent = relation
                                    == DurableHeadRelation::VerifiedStrictDescendant
                                    && cardinality == RunIdCardinality::SingleRunId
                                    && orphans == 0
                                    && !after_terminal;
                                // The legacy adoption arm: identity-less
                                // clean COMPLETED shape with legacy stamp
                                // evidence. Stated independently of the
                                // modern-coherence predicate above.
                                let legacy = relation
                                    == DurableHeadRelation::VerifiedStrictDescendant
                                    && cardinality == RunIdCardinality::NoRunId
                                    && stop == DurableTailStopReason::EndTurn
                                    && dangling == 0
                                    && orphans == 0
                                    && !after_terminal
                                    && era == DurableHeadStampEra::PreWitnessV3;
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
                                } else if legacy {
                                    DurableTailRecoveryClass::LegacyCompletedCandidate
                                } else {
                                    DurableTailRecoveryClass::Ambiguous
                                };
                                assert_eq!(
                                    got, expected,
                                    "truth-table mismatch at relation={relation:?} \
                                     cardinality={cardinality:?} stop={stop:?} \
                                     dangling={dangling} orphans={orphans} \
                                     after_terminal={after_terminal} era={era:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
