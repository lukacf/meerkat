//! Level 1 — SessionDocumentMachine runtime-snapshot read-source arbitration.
//!
//! Drives the generated authority (`SessionDocumentMachineAuthority`)
//! directly and pins the full `ResolveRuntimeSnapshotReadSource` disposition
//! table, including the legacy-adoption routing arm:
//!
//! - a COMMITTED strict descendant is ordinary authority
//!   (`UseCommittedStoreHead`), whatever the stamp era;
//! - a cold intra-turn strict descendant with run-id-bound execution routes
//!   to recovery (`RecoveryRequired`);
//! - a cold INTRA-TURN strict descendant whose tail records execution with
//!   NO run identity routes to recovery ONLY under pre-witness-v3 stamp
//!   evidence (the legacy lost-boundary shape — run-identity bookkeeping did
//!   not exist when the tail was written); with modern stamp evidence, or
//!   without a verified stamp at all (Unstamped provenance), it keeps the
//!   fail-closed `Quarantine`;
//! - live sessions, exact/behind rows, and execution-free tails keep the
//!   runtime snapshot; non-intra-turn forks and unverifiable evidence keep
//!   the quarantine.
//!
//! The transitions are total and disjoint: for every input combination
//! exactly one transition matches (the generated authority errors on zero or
//! multiple matches), which the exhaustive sweep pins.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use meerkat_core::session_document::{
    CheckpointProvenanceClass, DurableHeadRelation, DurableHeadStampEra,
    DurableTailExecutionEvidence, RuntimeSnapshotReadDisposition, SessionDocumentEffect,
    SessionDocumentKey, SessionDocumentMachineAuthority,
};

fn resolve(
    relation: DurableHeadRelation,
    store_provenance: CheckpointProvenanceClass,
    session_is_live: bool,
    tail_execution: DurableTailExecutionEvidence,
    head_stamp_era: DurableHeadStampEra,
) -> RuntimeSnapshotReadDisposition {
    let mut authority = SessionDocumentMachineAuthority::new();
    let effects = authority
        .resolve_runtime_snapshot_read_source(
            SessionDocumentKey::new("session-read-source"),
            relation,
            store_provenance,
            session_is_live,
            tail_execution,
            head_stamp_era,
        )
        .expect("ResolveRuntimeSnapshotReadSource must be total over the input domain");
    let mut resolved = effects.into_iter().filter_map(|effect| match effect {
        SessionDocumentEffect::RuntimeSnapshotReadSourceResolved { disposition } => {
            Some(disposition)
        }
        _ => None,
    });
    let first = resolved
        .next()
        .expect("resolution must emit a RuntimeSnapshotReadSourceResolved effect");
    assert!(
        resolved.next().is_none(),
        "resolution must emit exactly one RuntimeSnapshotReadSourceResolved effect"
    );
    first
}

/// The legacy lost-boundary shape routes to recovery: cold, intra-turn,
/// identity-less execution, pre-witness-v3 stamp evidence.
#[test]
fn cold_intra_turn_unbound_execution_with_legacy_stamp_routes_to_recovery() {
    assert_eq!(
        resolve(
            DurableHeadRelation::VerifiedStrictDescendant,
            CheckpointProvenanceClass::IntraTurn,
            false,
            DurableTailExecutionEvidence::UnboundExecution,
            DurableHeadStampEra::PreWitnessV3,
        ),
        RuntimeSnapshotReadDisposition::RecoveryRequired
    );
}

/// The same shape under MODERN stamp evidence keeps today's quarantine: a
/// modern in-run writer persists run identity inside the same message bytes,
/// so an identity-less modern tail is contradictory evidence.
#[test]
fn cold_intra_turn_unbound_execution_with_modern_stamp_keeps_quarantine() {
    assert_eq!(
        resolve(
            DurableHeadRelation::VerifiedStrictDescendant,
            CheckpointProvenanceClass::IntraTurn,
            false,
            DurableTailExecutionEvidence::UnboundExecution,
            DurableHeadStampEra::WitnessV3OrNewer,
        ),
        RuntimeSnapshotReadDisposition::Quarantine
    );
}

/// An UNSTAMPED row can never present legacy stamp evidence (the era is only
/// observable from a VERIFIED stamp), so the legacy arm requires intra-turn
/// provenance explicitly and an unstamped identity-less descendant stays
/// quarantined even if a shell mislabeled the era.
#[test]
fn unstamped_rows_never_take_the_legacy_arm() {
    for era in [
        DurableHeadStampEra::WitnessV3OrNewer,
        DurableHeadStampEra::PreWitnessV3,
    ] {
        assert_eq!(
            resolve(
                DurableHeadRelation::VerifiedStrictDescendant,
                CheckpointProvenanceClass::Unstamped,
                false,
                DurableTailExecutionEvidence::UnboundExecution,
                era,
            ),
            RuntimeSnapshotReadDisposition::Quarantine,
            "unstamped provenance with era {era:?} must stay quarantined"
        );
    }
}

/// The stamp era changes NOTHING outside the legacy arm: committed heads
/// serve, live sessions keep the snapshot, bound execution keeps the
/// existing recovery routing.
#[test]
fn stamp_era_is_inert_outside_the_legacy_arm() {
    for era in [
        DurableHeadStampEra::WitnessV3OrNewer,
        DurableHeadStampEra::PreWitnessV3,
    ] {
        assert_eq!(
            resolve(
                DurableHeadRelation::VerifiedStrictDescendant,
                CheckpointProvenanceClass::Committed,
                false,
                DurableTailExecutionEvidence::UnboundExecution,
                era,
            ),
            RuntimeSnapshotReadDisposition::UseCommittedStoreHead
        );
        assert_eq!(
            resolve(
                DurableHeadRelation::VerifiedStrictDescendant,
                CheckpointProvenanceClass::IntraTurn,
                true,
                DurableTailExecutionEvidence::UnboundExecution,
                era,
            ),
            RuntimeSnapshotReadDisposition::UseRuntimeSnapshot,
            "a live session's intra-turn residue keeps the snapshot"
        );
        assert_eq!(
            resolve(
                DurableHeadRelation::VerifiedStrictDescendant,
                CheckpointProvenanceClass::IntraTurn,
                false,
                DurableTailExecutionEvidence::BoundExecution,
                era,
            ),
            RuntimeSnapshotReadDisposition::RecoveryRequired
        );
    }
}

/// Every combination in the input domain resolves successfully with exactly
/// one verdict, and the verdict agrees with the independently-stated
/// disposition-table oracle.
#[test]
fn read_source_resolution_is_total_disjoint_and_matches_the_disposition_table() {
    let relations = [
        DurableHeadRelation::AbsentOrExact,
        DurableHeadRelation::RuntimeSnapshotAhead,
        DurableHeadRelation::VerifiedStrictDescendant,
        DurableHeadRelation::Diverged,
        DurableHeadRelation::Unverifiable,
    ];
    let provenances = [
        CheckpointProvenanceClass::Unstamped,
        CheckpointProvenanceClass::Committed,
        CheckpointProvenanceClass::IntraTurn,
    ];
    let executions = [
        DurableTailExecutionEvidence::NoExecutionContent,
        DurableTailExecutionEvidence::BoundExecution,
        DurableTailExecutionEvidence::UnboundExecution,
    ];
    let eras = [
        DurableHeadStampEra::WitnessV3OrNewer,
        DurableHeadStampEra::PreWitnessV3,
    ];

    for relation in relations {
        for provenance in provenances {
            for live in [false, true] {
                for execution in executions {
                    for era in eras {
                        let got = resolve(relation, provenance, live, execution, era);
                        let expected = match relation {
                            DurableHeadRelation::Unverifiable => {
                                RuntimeSnapshotReadDisposition::Quarantine
                            }
                            DurableHeadRelation::Diverged => {
                                if provenance == CheckpointProvenanceClass::IntraTurn {
                                    RuntimeSnapshotReadDisposition::UseRuntimeSnapshot
                                } else {
                                    RuntimeSnapshotReadDisposition::Quarantine
                                }
                            }
                            DurableHeadRelation::VerifiedStrictDescendant => {
                                if provenance == CheckpointProvenanceClass::Committed {
                                    RuntimeSnapshotReadDisposition::UseCommittedStoreHead
                                } else if live {
                                    RuntimeSnapshotReadDisposition::UseRuntimeSnapshot
                                } else {
                                    match execution {
                                        DurableTailExecutionEvidence::NoExecutionContent => {
                                            RuntimeSnapshotReadDisposition::UseRuntimeSnapshot
                                        }
                                        DurableTailExecutionEvidence::BoundExecution => {
                                            RuntimeSnapshotReadDisposition::RecoveryRequired
                                        }
                                        DurableTailExecutionEvidence::UnboundExecution => {
                                            if provenance == CheckpointProvenanceClass::IntraTurn
                                                && era == DurableHeadStampEra::PreWitnessV3
                                            {
                                                RuntimeSnapshotReadDisposition::RecoveryRequired
                                            } else {
                                                RuntimeSnapshotReadDisposition::Quarantine
                                            }
                                        }
                                    }
                                }
                            }
                            DurableHeadRelation::AbsentOrExact
                            | DurableHeadRelation::RuntimeSnapshotAhead => {
                                RuntimeSnapshotReadDisposition::UseRuntimeSnapshot
                            }
                        };
                        assert_eq!(
                            got, expected,
                            "disposition-table mismatch at relation={relation:?} \
                             provenance={provenance:?} live={live} execution={execution:?} \
                             era={era:?}"
                        );
                    }
                }
            }
        }
    }
}
