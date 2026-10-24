//! Level 1 — SessionDocumentMachine durable-tail classification truth table.
//!
//! The shell encodes transcript structure; the generated machine assigns one
//! modern recovery class:
//!
//! - Completed: verified descendant, exactly one run id, EndTurn, no
//!   dangling calls or orphan results, and nothing after the terminal.
//! - Repairable: the same coherent single-run shape, no dangling calls, and a
//!   ToolUse or Absent stop.
//! - Everything else is Ambiguous and held intact.
//!
//! A verified descendant is a verified strict descendant (the row extends the
//! authority's exact message prefix) or a verified rewrite descendant (the
//! store proved that transcript rewrite occurrences bridge the authority to
//! the row). Both pass the same shape checks; neither stands in for the
//! other.
//!
//! A dangling tool_use proves intent, not execution: its external effect may
//! already have fired, so the machine never auto-closes such a tail.
//!
//! Rewrite rows carry no run identity. A rewrite descendant whose live tail
//! holds no run-stamped message observes no run id, so it is held as
//! Ambiguous rather than closed on the provisional's run identity alone.
//!
//! A durable session's run cursor adds one class and one attribution:
//!
//! - Continuable: the Repairable shape with a cursor that agrees with the
//!   transcript and whose bound inputs are all still bound to the run. A
//!   cursor never upgrades another shape, never sits on an EndTurn shape,
//!   and a disagreeing cursor holds.
//! - A tail with no run rows at all (no assistant message, no tool result)
//!   carries no run id. On a strict continuation the store-issued provisional
//!   binding attributes it; on a rewrite descendant only an agreeing cursor
//!   does. Attributed, it takes the Repairable or Continuable arm.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use meerkat_core::session_document::{
    DurableHeadRelation, DurableTailRecoveryClass, DurableTailRunCursor, DurableTailStopReason,
    RunIdCardinality, SessionDocumentEffect, SessionDocumentKey, SessionDocumentMachineAuthority,
};

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
    classify_with_cursor(
        candidate_id,
        relation,
        run_id_cardinality,
        terminal_stop_reason,
        dangling_tool_use_count,
        orphan_tool_result_count,
        messages_after_terminal,
        DurableTailRunCursor::Absent,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn classify_with_cursor(
    candidate_id: &str,
    relation: DurableHeadRelation,
    run_id_cardinality: RunIdCardinality,
    terminal_stop_reason: DurableTailStopReason,
    dangling_tool_use_count: u64,
    orphan_tool_result_count: u64,
    messages_after_terminal: bool,
    run_cursor: DurableTailRunCursor,
    cursor_inputs_open: bool,
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
            run_cursor,
            cursor_inputs_open,
        )
        .expect("ClassifyDurableTail must be total");
    let mut classified = effects.into_iter().filter_map(|effect| match effect {
        SessionDocumentEffect::DurableTailClassified {
            candidate_id,
            class,
        } => Some((candidate_id, class)),
        _ => None,
    });
    let first = classified
        .next()
        .expect("classification must emit a verdict");
    assert!(
        classified.next().is_none(),
        "classification must emit exactly one verdict"
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

/// Every head relation the truth table covers.
const RELATIONS: [DurableHeadRelation; 6] = [
    DurableHeadRelation::AbsentOrExact,
    DurableHeadRelation::RuntimeSnapshotAhead,
    DurableHeadRelation::VerifiedStrictDescendant,
    DurableHeadRelation::Diverged,
    DurableHeadRelation::Unverifiable,
    DurableHeadRelation::VerifiedRewriteDescendant,
];

/// The table's own oracle for the relation conjunct. The match is
/// exhaustive on purpose: a new relation does not compile here until the
/// table decides whether it is a verified descendant (and joins
/// [`RELATIONS`]).
fn is_verified_descendant(relation: DurableHeadRelation) -> bool {
    match relation {
        DurableHeadRelation::VerifiedStrictDescendant
        | DurableHeadRelation::VerifiedRewriteDescendant => true,
        DurableHeadRelation::AbsentOrExact
        | DurableHeadRelation::RuntimeSnapshotAhead
        | DurableHeadRelation::Diverged
        | DurableHeadRelation::Unverifiable => false,
    }
}

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

#[test]
fn tool_use_and_absent_are_repairable_only_without_dangling_calls() {
    for stop in [
        DurableTailStopReason::ToolUse,
        DurableTailStopReason::Absent,
    ] {
        assert_eq!(
            classify(
                DurableHeadRelation::VerifiedStrictDescendant,
                RunIdCardinality::SingleRunId,
                stop,
                0,
                0,
                false,
            ),
            DurableTailRecoveryClass::InterruptedRepairableCandidate
        );
        for dangling in [1, 3] {
            assert_eq!(
                classify(
                    DurableHeadRelation::VerifiedStrictDescendant,
                    RunIdCardinality::SingleRunId,
                    stop,
                    dangling,
                    0,
                    false,
                ),
                DurableTailRecoveryClass::Ambiguous,
                "dangling call at {stop:?} must hold"
            );
        }
    }
}

#[test]
fn every_stop_shape_with_a_dangling_call_is_ambiguous() {
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
            DurableTailRecoveryClass::Ambiguous
        );
    }
}

#[test]
fn identity_relation_and_shape_contradictions_are_ambiguous() {
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
            DurableTailRecoveryClass::Ambiguous
        );
    }

    for cardinality in [RunIdCardinality::NoRunId, RunIdCardinality::MultipleRunIds] {
        assert_eq!(
            classify(
                DurableHeadRelation::VerifiedStrictDescendant,
                cardinality,
                DurableTailStopReason::EndTurn,
                0,
                0,
                false,
            ),
            DurableTailRecoveryClass::Ambiguous
        );
    }

    for (orphans, after, stop) in [
        (1, false, DurableTailStopReason::EndTurn),
        (0, true, DurableTailStopReason::EndTurn),
        (0, false, DurableTailStopReason::Other),
    ] {
        assert_eq!(
            classify(
                DurableHeadRelation::VerifiedStrictDescendant,
                RunIdCardinality::SingleRunId,
                stop,
                0,
                orphans,
                after,
            ),
            DurableTailRecoveryClass::Ambiguous
        );
    }
}

#[test]
fn rewrite_descendant_completed_row_classifies_completed_candidate() {
    assert_eq!(
        classify(
            DurableHeadRelation::VerifiedRewriteDescendant,
            RunIdCardinality::SingleRunId,
            DurableTailStopReason::EndTurn,
            0,
            0,
            false,
        ),
        DurableTailRecoveryClass::CompletedCandidate
    );
}

#[test]
fn rewrite_descendant_tool_use_and_absent_are_repairable_only_without_dangling_calls() {
    for stop in [
        DurableTailStopReason::ToolUse,
        DurableTailStopReason::Absent,
    ] {
        assert_eq!(
            classify(
                DurableHeadRelation::VerifiedRewriteDescendant,
                RunIdCardinality::SingleRunId,
                stop,
                0,
                0,
                false,
            ),
            DurableTailRecoveryClass::InterruptedRepairableCandidate,
            "a coherent single-run rewrite tail stopped at {stop:?} must be repairable"
        );
        for dangling in [1, 3] {
            assert_eq!(
                classify(
                    DurableHeadRelation::VerifiedRewriteDescendant,
                    RunIdCardinality::SingleRunId,
                    stop,
                    dangling,
                    0,
                    false,
                ),
                DurableTailRecoveryClass::Ambiguous,
                "a rewrite tail with a dangling call at {stop:?} must hold"
            );
        }
    }
}

/// An empty live tail after a rewrite observes no run id: rewrite rows carry
/// no run identity, so the provisional's run id alone never closes it. More
/// than one run id is held the same way.
#[test]
fn rewrite_descendant_without_exactly_one_run_id_is_held_ambiguous() {
    for cardinality in [RunIdCardinality::NoRunId, RunIdCardinality::MultipleRunIds] {
        for stop in [
            DurableTailStopReason::Absent,
            DurableTailStopReason::EndTurn,
            DurableTailStopReason::ToolUse,
            DurableTailStopReason::Other,
        ] {
            assert_eq!(
                classify(
                    DurableHeadRelation::VerifiedRewriteDescendant,
                    cardinality,
                    stop,
                    0,
                    0,
                    false,
                ),
                DurableTailRecoveryClass::Ambiguous,
                "a rewrite tail with {cardinality:?} stopped at {stop:?} must hold"
            );
        }
    }
}

#[test]
fn rewrite_descendant_shape_contradictions_are_ambiguous() {
    for (dangling, orphans, after, stop) in [
        (0, 1, false, DurableTailStopReason::EndTurn),
        (0, 1, false, DurableTailStopReason::ToolUse),
        (0, 0, true, DurableTailStopReason::EndTurn),
        (0, 0, true, DurableTailStopReason::Absent),
        (0, 0, false, DurableTailStopReason::Other),
        (1, 0, false, DurableTailStopReason::EndTurn),
    ] {
        assert_eq!(
            classify(
                DurableHeadRelation::VerifiedRewriteDescendant,
                RunIdCardinality::SingleRunId,
                stop,
                dangling,
                orphans,
                after,
            ),
            DurableTailRecoveryClass::Ambiguous,
            "rewrite tail dangling={dangling} orphans={orphans} after_terminal={after} \
             stop={stop:?} must hold"
        );
    }
}

/// The rewrite relation is admitted on exactly the strict relation's shape
/// guards: over every observed shape the two relations get the same class.
#[test]
fn rewrite_and_strict_descendants_classify_identically_over_every_shape() {
    for cardinality in [
        RunIdCardinality::NoRunId,
        RunIdCardinality::SingleRunId,
        RunIdCardinality::MultipleRunIds,
    ] {
        for stop in [
            DurableTailStopReason::Absent,
            DurableTailStopReason::EndTurn,
            DurableTailStopReason::ToolUse,
            DurableTailStopReason::Other,
        ] {
            for dangling in [0u64, 1, 3] {
                for orphans in [0u64, 1] {
                    for after_terminal in [false, true] {
                        let strict = classify(
                            DurableHeadRelation::VerifiedStrictDescendant,
                            cardinality,
                            stop,
                            dangling,
                            orphans,
                            after_terminal,
                        );
                        let rewrite = classify(
                            DurableHeadRelation::VerifiedRewriteDescendant,
                            cardinality,
                            stop,
                            dangling,
                            orphans,
                            after_terminal,
                        );
                        assert_eq!(
                            rewrite, strict,
                            "rewrite and strict descendants diverge: \
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

#[test]
fn effect_carries_the_exact_candidate_id_on_every_class() {
    let candidate = "candidate:session-recovery:rev385:cas42:run-7f";
    for (relation, stop, expected) in [
        (
            DurableHeadRelation::VerifiedStrictDescendant,
            DurableTailStopReason::EndTurn,
            DurableTailRecoveryClass::CompletedCandidate,
        ),
        (
            DurableHeadRelation::VerifiedStrictDescendant,
            DurableTailStopReason::Absent,
            DurableTailRecoveryClass::InterruptedRepairableCandidate,
        ),
        (
            DurableHeadRelation::VerifiedRewriteDescendant,
            DurableTailStopReason::EndTurn,
            DurableTailRecoveryClass::CompletedCandidate,
        ),
        (
            DurableHeadRelation::VerifiedRewriteDescendant,
            DurableTailStopReason::ToolUse,
            DurableTailRecoveryClass::InterruptedRepairableCandidate,
        ),
        (
            DurableHeadRelation::Diverged,
            DurableTailStopReason::EndTurn,
            DurableTailRecoveryClass::Ambiguous,
        ),
    ] {
        let (echoed, class) = classify_with_candidate(
            candidate,
            relation,
            RunIdCardinality::SingleRunId,
            stop,
            0,
            0,
            false,
        );
        assert_eq!(echoed, candidate);
        assert_eq!(class, expected);
    }
}

#[test]
fn classification_is_total_disjoint_and_matches_the_truth_table() {
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

    for relation in RELATIONS {
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
                            let coherent = is_verified_descendant(relation)
                                && cardinality == RunIdCardinality::SingleRunId
                                && dangling == 0
                                && orphans == 0
                                && !after_terminal;
                            let expected = if coherent {
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
                                "truth-table mismatch: relation={relation:?} \
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

/// The run-cursor truth table, unrun until the candidate's validation (the
/// policy forbids Cargo while the batch is implemented). The oracle restates
/// the documented partition; the classifier must be total and disjoint over
/// every combination, including the shapes the shell never produces (a tail
/// with no run rows always has an Absent stop).
#[test]
fn run_cursor_classification_is_total_disjoint_and_matches_the_truth_table() {
    let cardinalities = [
        RunIdCardinality::NoRunId,
        RunIdCardinality::SingleRunId,
        RunIdCardinality::MultipleRunIds,
        RunIdCardinality::NoRunRows,
    ];
    let stops = [
        DurableTailStopReason::Absent,
        DurableTailStopReason::EndTurn,
        DurableTailStopReason::ToolUse,
        DurableTailStopReason::Other,
    ];
    let cursors = [
        DurableTailRunCursor::Disagrees,
        DurableTailRunCursor::Absent,
        DurableTailRunCursor::Agrees,
    ];
    for relation in RELATIONS {
        for cardinality in cardinalities {
            for stop in stops {
                for dangling in [0u64, 1] {
                    for orphans in [0u64, 1] {
                        for after_terminal in [false, true] {
                            for cursor in cursors {
                                for inputs_bound in [false, true] {
                                    let got = classify_with_cursor(
                                        "candidate-under-test",
                                        relation,
                                        cardinality,
                                        stop,
                                        dangling,
                                        orphans,
                                        after_terminal,
                                        cursor,
                                        inputs_bound,
                                    )
                                    .1;
                                    let attributed = cardinality == RunIdCardinality::SingleRunId
                                        || (cardinality == RunIdCardinality::NoRunRows
                                            && stop == DurableTailStopReason::Absent
                                            && (relation
                                                == DurableHeadRelation::VerifiedStrictDescendant
                                                || cursor == DurableTailRunCursor::Agrees));
                                    let coherent = is_verified_descendant(relation)
                                        && attributed
                                        && dangling == 0
                                        && orphans == 0
                                        && !after_terminal
                                        && cursor != DurableTailRunCursor::Disagrees;
                                    let expected = if coherent {
                                        match (stop, cursor) {
                                            (
                                                DurableTailStopReason::EndTurn,
                                                DurableTailRunCursor::Absent,
                                            ) => DurableTailRecoveryClass::CompletedCandidate,
                                            (
                                                DurableTailStopReason::ToolUse
                                                | DurableTailStopReason::Absent,
                                                DurableTailRunCursor::Agrees,
                                            ) if inputs_bound => {
                                                DurableTailRecoveryClass::ContinuableCandidate
                                            }
                                            (
                                                DurableTailStopReason::ToolUse
                                                | DurableTailStopReason::Absent,
                                                DurableTailRunCursor::Absent
                                                | DurableTailRunCursor::Agrees,
                                            ) => {
                                                DurableTailRecoveryClass::InterruptedRepairableCandidate
                                            }
                                            _ => DurableTailRecoveryClass::Ambiguous,
                                        }
                                    } else {
                                        DurableTailRecoveryClass::Ambiguous
                                    };
                                    assert_eq!(
                                        got, expected,
                                        "truth-table mismatch: relation={relation:?} \
                                         cardinality={cardinality:?} stop={stop:?} \
                                         dangling={dangling} orphans={orphans} \
                                         after_terminal={after_terminal} cursor={cursor:?} \
                                         inputs_bound={inputs_bound}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The run cursor classification rows, as explicit cases.
#[test]
fn run_cursor_named_rows() {
    let strict = DurableHeadRelation::VerifiedStrictDescendant;
    let rewrite = DurableHeadRelation::VerifiedRewriteDescendant;
    let absent = DurableTailStopReason::Absent;
    let case = |relation, cardinality, stop, cursor, bound| {
        classify_with_cursor(
            "candidate-under-test",
            relation,
            cardinality,
            stop,
            0,
            0,
            false,
            cursor,
            bound,
        )
        .1
    };
    // Round zero's first commit, and a metadata-only tail: no run rows, an
    // agreeing cursor, bound inputs.
    assert_eq!(
        case(
            strict,
            RunIdCardinality::NoRunRows,
            absent,
            DurableTailRunCursor::Agrees,
            true
        ),
        DurableTailRecoveryClass::ContinuableCandidate
    );
    // A run's closing save before any assistant row: no run rows, no cursor.
    assert_eq!(
        case(
            strict,
            RunIdCardinality::NoRunRows,
            absent,
            DurableTailRunCursor::Absent,
            false
        ),
        DurableTailRecoveryClass::InterruptedRepairableCandidate
    );
    // After a rewrite only an agreeing cursor attributes a tail with no run rows.
    assert_eq!(
        case(
            rewrite,
            RunIdCardinality::NoRunRows,
            absent,
            DurableTailRunCursor::Absent,
            false
        ),
        DurableTailRecoveryClass::Ambiguous
    );
    assert_eq!(
        case(
            rewrite,
            RunIdCardinality::NoRunRows,
            absent,
            DurableTailRunCursor::Agrees,
            true
        ),
        DurableTailRecoveryClass::ContinuableCandidate
    );
    // Settled or requeued bound inputs, or an empty bound set: repaired.
    assert_eq!(
        case(
            strict,
            RunIdCardinality::NoRunRows,
            absent,
            DurableTailRunCursor::Agrees,
            false
        ),
        DurableTailRecoveryClass::InterruptedRepairableCandidate
    );
    assert_eq!(
        case(
            strict,
            RunIdCardinality::SingleRunId,
            DurableTailStopReason::ToolUse,
            DurableTailRunCursor::Agrees,
            false
        ),
        DurableTailRecoveryClass::InterruptedRepairableCandidate
    );
    // An unstamped assistant row is no run rows' shape: held.
    assert_eq!(
        case(
            strict,
            RunIdCardinality::NoRunId,
            absent,
            DurableTailRunCursor::Agrees,
            true
        ),
        DurableTailRecoveryClass::Ambiguous
    );
    // A disagreeing cursor holds, and a cursor never sits on an EndTurn shape.
    assert_eq!(
        case(
            strict,
            RunIdCardinality::SingleRunId,
            DurableTailStopReason::ToolUse,
            DurableTailRunCursor::Disagrees,
            true
        ),
        DurableTailRecoveryClass::Ambiguous
    );
    assert_eq!(
        case(
            strict,
            RunIdCardinality::SingleRunId,
            DurableTailStopReason::EndTurn,
            DurableTailRunCursor::Agrees,
            true
        ),
        DurableTailRecoveryClass::Ambiguous
    );
}
