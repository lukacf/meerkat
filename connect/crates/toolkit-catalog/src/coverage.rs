//! Explicit semantic attribution. These declarations are not execution proof.

use meerkat_machine_schema::catalog::{
    CoverageAnchor, CoverageClaims, CoverageSchemaTarget, MachineCoverageManifest,
    ScenarioCoverage, SemanticCoverageEntry, SymbolRef,
};

fn anchor(id: &str, note: &str, claims: CoverageClaims) -> CoverageAnchor {
    CoverageAnchor {
        id: id.into(),
        symbol: SymbolRef::parse("crates/toolkit-catalog/src/dsl/acquisition.rs")
            .expect("canonical portable source path"),
        target: CoverageSchemaTarget::Machine(crate::acquisition_schema().machine),
        note: note.into(),
        claims,
    }
}

fn scenario(id: &str, summary: &str, claims: CoverageClaims) -> ScenarioCoverage {
    ScenarioCoverage {
        id: id.into(),
        summary: summary.into(),
        claims,
    }
}

/// Strict coverage declarations for the canonical lifecycle. Scenario IDs name
/// tests in `tests/acquisition.rs`; a test run is recorded separately. The
/// anchors target the actual generated authority, not a future runtime bridge.
pub fn acquisition_coverage() -> MachineCoverageManifest {
    let admission = CoverageClaims::none()
        .transitions(&["AdmitRequest"])
        .effects(&["WorkAdmissionRequested"])
        .invariants(&[
            "absent_is_empty",
            "admitted_identity_is_bound",
            "investigating_has_no_selection",
        ]);
    let selection = CoverageClaims::none()
        .transitions(&["SelectImplementation"])
        .effects(&["SelectionRecorded"])
        .invariants(&[
            "selection_has_exact_evidence",
            "preparing_has_selection",
            "active_precedes_deadline",
        ]);
    let resolution = CoverageClaims::none()
        .transitions(&[
            "ResolveCurrentReadiness",
            "ResolveUnavailable",
            "ResolveFailure",
            "CancelRequest",
            "ExpireRequest",
        ])
        .effects(&["ContinuationRequested"])
        .invariants(&[
            "ready_has_current_observation",
            "other_outcomes_have_no_readiness",
            "terminal_has_one_continuation",
            "nonterminal_has_no_continuation",
            "pending_continuation_retains_terminal_revision",
            "expiry_observes_deadline",
            "resolution_detail_matches_outcome",
            "non_expiry_resolution_precedes_deadline",
        ]);
    let acknowledgment = CoverageClaims::none()
        .transitions(&[
            "AcknowledgeContinuationReady",
            "AcknowledgeContinuationUnavailable",
            "AcknowledgeContinuationFailed",
            "AcknowledgeContinuationCancelled",
            "AcknowledgeContinuationExpired",
            "ObserveSameContinuationAcknowledgmentReady",
            "ObserveSameContinuationAcknowledgmentUnavailable",
            "ObserveSameContinuationAcknowledgmentFailed",
            "ObserveSameContinuationAcknowledgmentCancelled",
            "ObserveSameContinuationAcknowledgmentExpired",
        ])
        .effects(&["ContinuationAcknowledged"])
        .invariants(&[
            "acknowledged_continuation_advances_once",
            "revision_tracks_selection_and_resolution",
        ]);
    let anchors = [
        anchor(
            "admission-owner",
            "Generated admission binds immutable request identity and authorizes its one work handoff",
            admission.clone(),
        ),
        anchor(
            "selection-owner",
            "Generated selection supersession records agent decision bindings and advances the generation",
            selection.clone(),
        ),
        anchor(
            "resolution-owner",
            "Generated terminal resolution consumes current owner observation bindings and creates one continuation",
            resolution.clone(),
        ),
        anchor(
            "continuation-owner",
            "Generated identity-bound continuation acknowledgment and idempotent observation",
            acknowledgment.clone(),
        ),
    ];
    let scenarios = [
        scenario(
            "admission_binds_identity_and_launches_work_once",
            "Admit once, retain immutable identity and reject duplicate/invalid admission",
            admission,
        ),
        scenario(
            "replaced_selection_requires_fresh_generation_even_after_aba",
            "Changed selection increments generation and old receipts fail even if the selected digest returns",
            selection,
        ),
        scenario(
            "terminal_outcomes_are_distinct_and_acknowledgment_is_bound_and_idempotent",
            "All five terminal classes produce exactly one continuation, with exact receipt replay and wrong identity rejection",
            resolution
                .clone()
                .transitions(&[
                    "ResolveCurrentReadiness",
                    "ResolveUnavailable",
                    "ResolveFailure",
                    "CancelRequest",
                    "ExpireRequest",
                    "AcknowledgeContinuationReady",
                    "AcknowledgeContinuationUnavailable",
                    "AcknowledgeContinuationFailed",
                    "AcknowledgeContinuationCancelled",
                    "AcknowledgeContinuationExpired",
                    "ObserveSameContinuationAcknowledgmentReady",
                    "ObserveSameContinuationAcknowledgmentUnavailable",
                    "ObserveSameContinuationAcknowledgmentFailed",
                    "ObserveSameContinuationAcknowledgmentCancelled",
                    "ObserveSameContinuationAcknowledgmentExpired",
                ])
                .effects(&["ContinuationRequested", "ContinuationAcknowledged"])
                .invariants(&[
                    "ready_has_current_observation",
                    "other_outcomes_have_no_readiness",
                    "terminal_has_one_continuation",
                    "nonterminal_has_no_continuation",
                    "pending_continuation_retains_terminal_revision",
                    "expiry_observes_deadline",
                    "resolution_detail_matches_outcome",
                    "non_expiry_resolution_precedes_deadline",
                    "acknowledged_continuation_advances_once",
                ]),
        ),
        scenario(
            "revision_capacity_reserves_terminal_resolution_and_acknowledgment",
            "Selection stops before overflow so cancellation and acknowledgment can still complete",
            CoverageClaims::none().invariants(&["revision_tracks_selection_and_resolution"]),
        ),
        scenario(
            "release_recovery_rejects_invalid_states_without_debug_assertions",
            "Recovery rejects invalid bindings, readiness, outcome and continuation records in debug and release",
            CoverageClaims::none().invariants(&[
                "absent_is_empty",
                "admitted_identity_is_bound",
                "selection_has_exact_evidence",
                "preparing_has_selection",
                "active_precedes_deadline",
                "ready_has_current_observation",
                "other_outcomes_have_no_readiness",
                "terminal_has_one_continuation",
                "nonterminal_has_no_continuation",
                "pending_continuation_retains_terminal_revision",
                "expiry_observes_deadline",
                "resolution_detail_matches_outcome",
                "revision_tracks_selection_and_resolution",
                "non_expiry_resolution_precedes_deadline",
            ]),
        ),
    ];
    let schema = crate::acquisition_schema();
    // Toolkit owns these declarations. Project only exact claims onto the
    // generated schema; an unclaimed element keeps empty attribution lists.
    // Native strict validation remains responsible for checking the manifest.
    let entry = |name: &str, declares: fn(&CoverageClaims, &str) -> bool| SemanticCoverageEntry {
        name: name.to_owned(),
        anchor_ids: anchors
            .iter()
            .filter(|anchor| declares(&anchor.claims, name))
            .map(|anchor| anchor.id.clone())
            .collect(),
        scenario_ids: scenarios
            .iter()
            .filter(|scenario| declares(&scenario.claims, name))
            .map(|scenario| scenario.id.clone())
            .collect(),
    };
    MachineCoverageManifest {
        machine: schema.machine.clone(),
        transition_coverage: schema
            .transitions
            .iter()
            .map(|transition| {
                entry(transition.name.as_str(), |claims, name| {
                    claims.transitions.iter().any(|id| id.as_str() == name)
                })
            })
            .collect(),
        effect_coverage: schema
            .effects
            .variants
            .iter()
            .map(|effect| {
                entry(effect.name.as_str(), |claims, name| {
                    claims.effects.iter().any(|id| id.as_str() == name)
                })
            })
            .collect(),
        invariant_coverage: schema
            .invariants
            .iter()
            .map(|invariant| {
                entry(&invariant.name, |claims, name| {
                    claims.invariants.iter().any(|id| id == name)
                })
            })
            .collect(),
        code_anchors: anchors.to_vec(),
        scenarios: scenarios.to_vec(),
    }
}
