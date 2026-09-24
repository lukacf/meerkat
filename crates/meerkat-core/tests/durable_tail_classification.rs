//! Level 1 — SessionDocumentMachine durable-tail classification truth table.
//!
//! The shell encodes transcript structure; the generated machine assigns one
//! modern recovery class:
//!
//! - Completed: verified strict descendant, exactly one run id, EndTurn, no
//!   dangling calls or orphan results, and nothing after the terminal.
//! - Repairable: the same coherent single-run shape, no dangling calls, and a
//!   ToolUse or Absent stop.
//! - Everything else is Ambiguous and held intact.
//!
//! A dangling tool_use proves intent, not execution: its external effect may
//! already have fired, so the machine never auto-closes such a tail.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use meerkat_core::session_document::{
    DurableHeadRelation, DurableTailRecoveryClass, DurableTailStopReason, RunIdCardinality,
    SessionDocumentEffect, SessionDocumentKey, SessionDocumentMachineAuthority,
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
