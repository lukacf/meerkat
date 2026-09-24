//! Generated model-routing handoff lifecycle: convergence and conflict.
//!
//! The handoff lifecycle exists because a permanent routing change survives the
//! run that asked for it, which means it must survive crashes, retries, and
//! competing claims. That safety rests on exactly two properties, and neither
//! is expressible as "the shell checks first":
//!
//! * a retry of the SAME request for the SAME target from the SAME originating
//!   run converges — it does not rotate identity a second time;
//! * the same request id naming a DIFFERENT target or a DIFFERENT originating
//!   run is refused by the machine, not reinterpreted.
//!
//! These are asserted against the generated kernel directly, so they hold
//! regardless of which shell drives it.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use meerkat_machine_schema::catalog::dsl::meerkat_machine::{
    MeerkatMachineAuthority, MeerkatMachineInput, MeerkatMachineMutator, MeerkatMachineSignal,
    ModelRoutingHandoffRecord, ModelRoutingHandoffRecordError, RoutingAppliedModel,
    RoutingDenialReason, RoutingHandoffPhase, SessionId,
};

const REQUEST: &str = "req-1";
const RUN: &str = "run-1";
const TARGET: &str = "model-b";

/// A registered, idle machine — the state the pre-dequeue seam actually
/// observes, because it runs between turns.
fn registered_authority() -> MeerkatMachineAuthority {
    let mut authority = MeerkatMachineAuthority::new();
    authority
        .apply_signal(MeerkatMachineSignal::Initialize)
        .expect("machine initializes");
    MeerkatMachineMutator::apply(
        &mut authority,
        MeerkatMachineInput::RegisterSession {
            session_id: SessionId("session-1".to_string()),
            runtime_epoch_id: None,
        },
    )
    .expect("session registers");
    authority
}

/// Propose a record in `phase` for `run`/`target`, carrying no outcome fact.
///
/// Only the three outcome-free phases are constructible this way, which is the
/// point: `Realized` and `Denied` cannot exist without the fact that produced
/// them, so they have their own constructors.
/// A shorthand for the constructors' `Result` in tests that pass valid input.
///
/// Kept as an explicit expect rather than an `unwrap` so a future change that
/// tightens validation names the record it refused.
fn valid(
    record: Result<ModelRoutingHandoffRecord, ModelRoutingHandoffRecordError>,
) -> ModelRoutingHandoffRecord {
    record.expect("test builds a valid handoff record")
}

fn pending_record(
    phase: RoutingHandoffPhase,
    run: &str,
    target: &str,
) -> ModelRoutingHandoffRecord {
    match phase {
        RoutingHandoffPhase::Imported => valid(ModelRoutingHandoffRecord::imported(run, target)),
        RoutingHandoffPhase::Claimed => valid(ModelRoutingHandoffRecord::claimed(run, target)),
        RoutingHandoffPhase::Archived => valid(ModelRoutingHandoffRecord::archived(run, target)),
        other => unreachable!("{other:?} is not an outcome-free phase"),
    }
}

fn import(
    authority: &mut MeerkatMachineAuthority,
    request_id: &str,
    run: &str,
    target: &str,
) -> Result<(), String> {
    MeerkatMachineMutator::apply(
        authority,
        MeerkatMachineInput::ImportCommittedModelRoutingHandoff {
            request_id: request_id.to_string(),
            record: pending_record(RoutingHandoffPhase::Imported, run, target),
        },
    )
    .map(|_| ())
    .map_err(|error| format!("{error:?}"))
}

/// The record generated authority currently holds, as the shell would read it.
///
/// Every non-import transition names the state it observed, so tests read it
/// the same way production does. A request that is absent yields the
/// `Imported` record for its identity, which is deliberately WRONG for a
/// missing key — that keeps "claiming an unimported request" a real refusal
/// rather than an accident of the harness.
fn observed(
    authority: &MeerkatMachineAuthority,
    request_id: &str,
    run: &str,
    target: &str,
) -> ModelRoutingHandoffRecord {
    authority
        .state()
        .model_routing_handoff
        .get(request_id)
        .cloned()
        .unwrap_or_else(|| valid(ModelRoutingHandoffRecord::imported(run, target)))
}

fn claim(
    authority: &mut MeerkatMachineAuthority,
    request_id: &str,
    run: &str,
    target: &str,
) -> Result<(), String> {
    let observed = observed(authority, request_id, run, target);
    MeerkatMachineMutator::apply(
        authority,
        MeerkatMachineInput::ClaimModelRoutingHandoff {
            request_id: request_id.to_string(),
            observed,
            record: pending_record(RoutingHandoffPhase::Claimed, run, target),
        },
    )
    .map(|_| ())
    .map_err(|error| format!("{error:?}"))
}

fn realize(
    authority: &mut MeerkatMachineAuthority,
    request_id: &str,
    run: &str,
    target: &str,
    applied: &str,
) -> Result<(), String> {
    let observed = observed(authority, request_id, run, target);
    MeerkatMachineMutator::apply(
        authority,
        MeerkatMachineInput::RealizeModelRoutingHandoff {
            request_id: request_id.to_string(),
            observed,
            record: valid(ModelRoutingHandoffRecord::realized(run, target, applied)),
        },
    )
    .map(|_| ())
    .map_err(|error| format!("{error:?}"))
}

fn archive(
    authority: &mut MeerkatMachineAuthority,
    request_id: &str,
    run: &str,
    target: &str,
) -> Result<(), String> {
    let observed = observed(authority, request_id, run, target);
    MeerkatMachineMutator::apply(
        authority,
        MeerkatMachineInput::ArchiveUnresolvedModelRoutingHandoff {
            request_id: request_id.to_string(),
            observed,
            record: valid(ModelRoutingHandoffRecord::archived(run, target)),
        },
    )
    .map(|_| ())
    .map_err(|error| format!("{error:?}"))
}

fn phase(authority: &MeerkatMachineAuthority, request_id: &str) -> Option<RoutingHandoffPhase> {
    authority
        .state()
        .model_routing_handoff
        .get(request_id)
        .map(ModelRoutingHandoffRecord::phase)
}

#[test]
fn import_then_claim_then_realize_walks_the_lifecycle() {
    let mut authority = registered_authority();
    import(&mut authority, REQUEST, RUN, TARGET).expect("import");
    assert_eq!(
        phase(&authority, REQUEST),
        Some(RoutingHandoffPhase::Imported)
    );
    claim(&mut authority, REQUEST, RUN, TARGET).expect("claim");
    assert_eq!(
        phase(&authority, REQUEST),
        Some(RoutingHandoffPhase::Claimed)
    );
    realize(&mut authority, REQUEST, RUN, TARGET, TARGET).expect("realize");
    assert_eq!(
        phase(&authority, REQUEST),
        Some(RoutingHandoffPhase::Realized)
    );
    assert_eq!(
        authority
            .state()
            .model_routing_handoff
            .get(REQUEST)
            .and_then(ModelRoutingHandoffRecord::applied_model)
            .map(RoutingAppliedModel::as_str),
        Some(TARGET),
        "the resolved identity that was actually installed must be recorded"
    );
}

/// The steady state: the durable log is append-only, so every later lap
/// re-observes the same committed record until it terminalizes. Re-importing
/// must be a no-op, not a second request.
#[test]
fn re_importing_the_identical_committed_fact_converges() {
    let mut authority = registered_authority();
    import(&mut authority, REQUEST, RUN, TARGET).expect("first import");
    import(&mut authority, REQUEST, RUN, TARGET).expect("identical re-import converges");
    assert_eq!(
        phase(&authority, REQUEST),
        Some(RoutingHandoffPhase::Imported),
        "a repeat observation must not move the lifecycle"
    );
}

/// A claim interrupted before realization is retried on the next lap. The
/// retry must converge rather than attempt a second rotation.
#[test]
fn re_claiming_the_same_request_converges_without_rotating_twice() {
    let mut authority = registered_authority();
    import(&mut authority, REQUEST, RUN, TARGET).expect("import");
    claim(&mut authority, REQUEST, RUN, TARGET).expect("claim");
    claim(&mut authority, REQUEST, RUN, TARGET).expect("re-claim converges");
    assert_eq!(
        phase(&authority, REQUEST),
        Some(RoutingHandoffPhase::Claimed)
    );
}

/// After realization, a replayed claim must report already-exact instead of
/// re-entering the applying state.
#[test]
fn claiming_an_already_realized_request_converges_already_exact() {
    let mut authority = registered_authority();
    import(&mut authority, REQUEST, RUN, TARGET).expect("import");
    claim(&mut authority, REQUEST, RUN, TARGET).expect("claim");
    realize(&mut authority, REQUEST, RUN, TARGET, TARGET).expect("realize");
    claim(&mut authority, REQUEST, RUN, TARGET).expect("claim after realize converges");
    assert_eq!(
        phase(&authority, REQUEST),
        Some(RoutingHandoffPhase::Realized),
        "convergence must not walk a realized request back to claimed"
    );
}

/// Replaying the exact realization is idempotent — the crash window between
/// the durable record and the generated terminal is expected.
#[test]
fn re_realizing_the_same_identity_is_idempotent() {
    let mut authority = registered_authority();
    import(&mut authority, REQUEST, RUN, TARGET).expect("import");
    claim(&mut authority, REQUEST, RUN, TARGET).expect("claim");
    realize(&mut authority, REQUEST, RUN, TARGET, TARGET).expect("realize");
    realize(&mut authority, REQUEST, RUN, TARGET, TARGET).expect("replayed realize converges");
    assert_eq!(
        phase(&authority, REQUEST),
        Some(RoutingHandoffPhase::Realized)
    );
}

/// The conflict case. A matching request id alone must never be enough to move
/// a handoff that belongs to a different target.
#[test]
fn the_same_request_id_with_a_different_target_is_refused() {
    let mut authority = registered_authority();
    import(&mut authority, REQUEST, RUN, TARGET).expect("import");
    let error = import(&mut authority, REQUEST, RUN, "model-c")
        .expect_err("a contradictory target must be refused");
    assert!(
        !error.is_empty(),
        "the kernel must reject rather than absorb the contradiction"
    );
    assert_eq!(
        authority
            .state()
            .model_routing_handoff
            .get(REQUEST)
            .map(ModelRoutingHandoffRecord::target_model),
        Some(TARGET),
        "the refused import must not have mutated the committed target"
    );
}

/// Same identity, different originating run: also a conflict. A request is
/// only actionable because a specific run committed it.
#[test]
fn the_same_request_id_from_a_different_run_is_refused() {
    let mut authority = registered_authority();
    import(&mut authority, REQUEST, RUN, TARGET).expect("import");
    import(&mut authority, REQUEST, "run-2", TARGET)
        .expect_err("a different originating run must be refused");
    assert_eq!(
        authority
            .state()
            .model_routing_handoff
            .get(REQUEST)
            .map(ModelRoutingHandoffRecord::originating_run),
        Some(RUN)
    );
}

/// Claiming without importing is refused: a claim is a statement about a
/// committed record, and there is none.
#[test]
fn claiming_an_unimported_request_is_refused() {
    let mut authority = registered_authority();
    claim(&mut authority, REQUEST, RUN, TARGET).expect_err("nothing committed to claim");
    assert_eq!(phase(&authority, REQUEST), None);
}

/// Realizing without claiming is refused: realization marks a fact the claim
/// path is responsible for producing.
#[test]
fn realizing_an_unclaimed_request_is_refused() {
    let mut authority = registered_authority();
    import(&mut authority, REQUEST, RUN, TARGET).expect("import");
    realize(&mut authority, REQUEST, RUN, TARGET, TARGET)
        .expect_err("an unclaimed request cannot be realized");
    assert_eq!(
        phase(&authority, REQUEST),
        Some(RoutingHandoffPhase::Imported)
    );
}

/// Archive terminality is generated status whose durable mechanical mirror is
/// written by the archive chokepoint. It is admitted only after the session
/// itself reached generated Retired and is idempotent there.
#[test]
fn archiving_a_pending_handoff_requires_retired_and_is_idempotent() {
    let mut authority = registered_authority();
    import(&mut authority, REQUEST, RUN, TARGET).expect("import");
    archive(&mut authority, REQUEST, RUN, TARGET)
        .expect_err("an active session cannot mint an archived handoff");
    assert_eq!(
        phase(&authority, REQUEST),
        Some(RoutingHandoffPhase::Imported)
    );
    MeerkatMachineMutator::apply(
        &mut authority,
        MeerkatMachineInput::Retire {
            session_id: SessionId("session-1".to_string()),
        },
    )
    .expect("session retires before handoff archive");
    for _ in 0..2 {
        archive(&mut authority, REQUEST, RUN, TARGET).expect("archive converges");
    }
    assert_eq!(
        phase(&authority, REQUEST),
        Some(RoutingHandoffPhase::Archived)
    );
}

/// A realized handoff is terminal for archive too: archiving must not walk it
/// back, because the routing change already happened.
#[test]
fn archiving_a_realized_handoff_is_refused() {
    let mut authority = registered_authority();
    import(&mut authority, REQUEST, RUN, TARGET).expect("import");
    claim(&mut authority, REQUEST, RUN, TARGET).expect("claim");
    realize(&mut authority, REQUEST, RUN, TARGET, TARGET).expect("realize");
    archive(&mut authority, REQUEST, RUN, TARGET)
        .expect_err("a realized handoff is not unresolved");
    assert_eq!(
        phase(&authority, REQUEST),
        Some(RoutingHandoffPhase::Realized)
    );
}

/// An unregistered machine holds no session, so it can carry no handoff.
#[test]
fn an_unregistered_machine_refuses_to_import() {
    let mut authority = MeerkatMachineAuthority::new();
    authority
        .apply_signal(MeerkatMachineSignal::Initialize)
        .expect("machine initializes");
    import(&mut authority, REQUEST, RUN, TARGET)
        .expect_err("an unregistered session cannot own a handoff");
}

// ---------------------------------------------------------------------------
// Malformed replay.
//
// The shell proposes a whole record, so every arm — including the arms that do
// nothing — has to constrain the proposal. Two different mechanisms do that
// now, and both are pinned here:
//
//   * shapes that are meaningless are UNREPRESENTABLE — `ModelRoutingHandoffRecord`
//     has private fields and five phase-specific constructors, so there is no
//     way to build a `Realized` record with no installed identity or a
//     `Claimed` record carrying a denial. Those cases are exercised against the
//     decoder below, because deserialization is the only remaining way such a
//     value could enter the process.
//
//   * shapes that are well-formed but WRONG — a valid `Realized` record handed
//     to the import arm, or a valid `Denied` record naming a different run —
//     are refused by machine guards. Those are exercised here, and each test
//     re-reads state afterwards so a silently absorbed mutation fails too.
// ---------------------------------------------------------------------------

fn apply_raw(
    authority: &mut MeerkatMachineAuthority,
    input: MeerkatMachineInput,
) -> Result<(), String> {
    MeerkatMachineMutator::apply(authority, input)
        .map(|_| ())
        .map_err(|error| format!("{error:?}"))
}

/// Re-import is the hot path, so it is the easiest arm to smuggle a terminal
/// through: the record already exists and binds correctly.
#[test]
fn re_import_proposing_a_terminal_phase_is_refused() {
    let mut authority = registered_authority();
    import(&mut authority, REQUEST, RUN, TARGET).expect("import");
    apply_raw(
        &mut authority,
        MeerkatMachineInput::ImportCommittedModelRoutingHandoff {
            request_id: REQUEST.to_string(),
            record: valid(ModelRoutingHandoffRecord::realized(RUN, TARGET, TARGET)),
        },
    )
    .expect_err("an import may not propose a realized terminal");
    apply_raw(
        &mut authority,
        MeerkatMachineInput::ImportCommittedModelRoutingHandoff {
            request_id: REQUEST.to_string(),
            record: valid(ModelRoutingHandoffRecord::denied(
                RUN,
                TARGET,
                RoutingDenialReason::CapabilityPolicy,
            )),
        },
    )
    .expect_err("an import may not propose a denied terminal");
    assert_eq!(
        phase(&authority, REQUEST),
        Some(RoutingHandoffPhase::Imported),
        "a refused re-import must leave the lifecycle where it was"
    );
}

/// Re-claiming an already-claimed or already-realized request converges, but
/// only for a claim-shaped proposal.
#[test]
fn re_claim_proposing_a_terminal_phase_is_refused() {
    let mut authority = registered_authority();
    import(&mut authority, REQUEST, RUN, TARGET).expect("import");
    claim(&mut authority, REQUEST, RUN, TARGET).expect("claim");
    apply_raw(
        &mut authority,
        MeerkatMachineInput::ClaimModelRoutingHandoff {
            request_id: REQUEST.to_string(),
            observed: valid(ModelRoutingHandoffRecord::claimed(RUN, TARGET)),
            record: valid(ModelRoutingHandoffRecord::denied(
                RUN,
                TARGET,
                RoutingDenialReason::CapabilityPolicy,
            )),
        },
    )
    .expect_err("a claim may not propose a denial");
    assert_eq!(
        phase(&authority, REQUEST),
        Some(RoutingHandoffPhase::Claimed)
    );

    realize(&mut authority, REQUEST, RUN, TARGET, TARGET).expect("realize");
    apply_raw(
        &mut authority,
        MeerkatMachineInput::ClaimModelRoutingHandoff {
            request_id: REQUEST.to_string(),
            observed: valid(ModelRoutingHandoffRecord::realized(RUN, TARGET, TARGET)),
            record: valid(ModelRoutingHandoffRecord::realized(RUN, TARGET, TARGET)),
        },
    )
    .expect_err("converging on a realized request must still be claim-shaped");
    assert_eq!(
        phase(&authority, REQUEST),
        Some(RoutingHandoffPhase::Realized)
    );
}

/// A realization replay must still be a realization: same applied identity, and
/// bound to the same run and target.
#[test]
fn realization_replay_with_a_different_identity_is_refused() {
    let mut authority = registered_authority();
    import(&mut authority, REQUEST, RUN, TARGET).expect("import");
    claim(&mut authority, REQUEST, RUN, TARGET).expect("claim");
    realize(&mut authority, REQUEST, RUN, TARGET, TARGET).expect("realize");

    realize(&mut authority, REQUEST, RUN, TARGET, "model-z")
        .expect_err("a different applied identity is not the same realization");
    realize(&mut authority, REQUEST, "run-2", TARGET, TARGET)
        .expect_err("a realization from a different originating run is a conflict");
    realize(&mut authority, REQUEST, RUN, "model-z", TARGET)
        .expect_err("a realization naming a different target is a conflict");

    assert_eq!(
        authority
            .state()
            .model_routing_handoff
            .get(REQUEST)
            .and_then(ModelRoutingHandoffRecord::applied_model)
            .map(RoutingAppliedModel::as_str),
        Some(TARGET),
        "no refused replay may rewrite the installed identity"
    );
}

/// The denial replay arm previously compared only the reason, so a proposal
/// naming a different run or target could still report success.
#[test]
fn denial_replay_bound_to_a_different_request_is_refused() {
    let mut authority = registered_authority();
    import(&mut authority, REQUEST, RUN, TARGET).expect("import");
    apply_raw(
        &mut authority,
        MeerkatMachineInput::DenyModelRoutingHandoff {
            request_id: REQUEST.to_string(),
            observed: valid(ModelRoutingHandoffRecord::imported(RUN, TARGET)),
            record: valid(ModelRoutingHandoffRecord::denied(
                RUN,
                TARGET,
                RoutingDenialReason::CapabilityPolicy,
            )),
        },
    )
    .expect("deny");
    assert_eq!(
        phase(&authority, REQUEST),
        Some(RoutingHandoffPhase::Denied)
    );

    // A replay whose proposal names a different run. `observed` is the exact
    // stored record, so the ONLY thing wrong is the proposal's binding — which
    // is precisely the hole the old reason-only comparison left open.
    apply_raw(
        &mut authority,
        MeerkatMachineInput::DenyModelRoutingHandoff {
            request_id: REQUEST.to_string(),
            observed: valid(ModelRoutingHandoffRecord::denied(
                RUN,
                TARGET,
                RoutingDenialReason::CapabilityPolicy,
            )),
            record: valid(ModelRoutingHandoffRecord::denied(
                "run-2",
                TARGET,
                RoutingDenialReason::CapabilityPolicy,
            )),
        },
    )
    .expect_err("a denial replay from a different run must be refused");
    apply_raw(
        &mut authority,
        MeerkatMachineInput::DenyModelRoutingHandoff {
            request_id: REQUEST.to_string(),
            observed: valid(ModelRoutingHandoffRecord::denied(
                RUN,
                TARGET,
                RoutingDenialReason::CapabilityPolicy,
            )),
            record: valid(ModelRoutingHandoffRecord::denied(
                RUN,
                "model-z",
                RoutingDenialReason::CapabilityPolicy,
            )),
        },
    )
    .expect_err("a denial replay naming a different target must be refused");

    let state = authority.state();
    let stored = state
        .model_routing_handoff
        .get(REQUEST)
        .expect("record survives");
    assert_eq!(stored.originating_run(), RUN);
    assert_eq!(stored.target_model(), TARGET);
    assert_eq!(
        stored.denial_reason(),
        Some(RoutingDenialReason::CapabilityPolicy)
    );
}

/// The archive replay arm previously checked only the stored phase, so any
/// proposal at all could converge once a request was archived.
#[test]
fn archive_replay_bound_to_a_different_request_is_refused() {
    let mut authority = registered_authority();
    import(&mut authority, REQUEST, RUN, TARGET).expect("import");
    apply_raw(
        &mut authority,
        MeerkatMachineInput::Retire {
            session_id: SessionId("session-1".to_string()),
        },
    )
    .expect("session retires");
    apply_raw(
        &mut authority,
        MeerkatMachineInput::ArchiveUnresolvedModelRoutingHandoff {
            request_id: REQUEST.to_string(),
            observed: valid(ModelRoutingHandoffRecord::imported(RUN, TARGET)),
            record: valid(ModelRoutingHandoffRecord::archived(RUN, TARGET)),
        },
    )
    .expect("archive");

    // Both replays name the exact stored record as `observed`, so the only
    // defect is the proposal's binding — the hole the old stored-phase-only
    // comparison left open.
    apply_raw(
        &mut authority,
        MeerkatMachineInput::ArchiveUnresolvedModelRoutingHandoff {
            request_id: REQUEST.to_string(),
            observed: valid(ModelRoutingHandoffRecord::archived(RUN, TARGET)),
            record: valid(ModelRoutingHandoffRecord::archived("run-2", TARGET)),
        },
    )
    .expect_err("an archive replay from a different run must be refused");
    apply_raw(
        &mut authority,
        MeerkatMachineInput::ArchiveUnresolvedModelRoutingHandoff {
            request_id: REQUEST.to_string(),
            observed: valid(ModelRoutingHandoffRecord::archived(RUN, TARGET)),
            record: valid(ModelRoutingHandoffRecord::archived(RUN, "model-z")),
        },
    )
    .expect_err("an archive replay naming a different target must be refused");

    let state = authority.state();
    let stored = state
        .model_routing_handoff
        .get(REQUEST)
        .expect("record survives");
    assert_eq!(stored.phase(), RoutingHandoffPhase::Archived);
    assert_eq!(stored.originating_run(), RUN);
    assert_eq!(stored.target_model(), TARGET);
    assert_eq!(stored.applied_model(), None);
}

// ---------------------------------------------------------------------------
// Recovery decode.
//
// Machine state is serialized, so deserialization is a real ingress: a
// hand-edited, corrupted, or older-format state document can present a record
// that no constructor would ever produce. These decode directly from raw JSON
// rather than round-tripping a value, because a round-trip can only ever
// produce shapes the type already allows.
// ---------------------------------------------------------------------------

fn decode(json: &str) -> Result<ModelRoutingHandoffRecord, String> {
    serde_json::from_str::<ModelRoutingHandoffRecord>(json).map_err(|error| error.to_string())
}

#[test]
fn a_well_formed_record_decodes() {
    let record = decode(
        r#"{"phase":"Realized","originating_run":"run-1","target_model":"model-b","applied_model":"model-b"}"#,
    )
    .expect("a realized record naming its installed identity is valid");
    assert_eq!(record.phase(), RoutingHandoffPhase::Realized);
    assert_eq!(record.originating_run(), "run-1");
    assert_eq!(record.target_model(), "model-b");
    assert_eq!(
        record.applied_model().map(RoutingAppliedModel::as_str),
        Some("model-b")
    );
    assert_eq!(record.denial_reason(), None);
}

#[test]
fn a_realized_record_without_an_installed_identity_is_refused() {
    decode(r#"{"phase":"Realized","originating_run":"run-1","target_model":"model-b"}"#)
        .expect_err("a realization that names no installed identity is not a realization");
}

#[test]
fn a_denied_record_without_a_reason_is_refused() {
    decode(r#"{"phase":"Denied","originating_run":"run-1","target_model":"model-b"}"#)
        .expect_err("a denial that names no reason is not a denial");
}

#[test]
fn a_pending_record_carrying_an_outcome_is_refused() {
    decode(
        r#"{"phase":"Imported","originating_run":"run-1","target_model":"model-b","applied_model":"model-b"}"#,
    )
    .expect_err("an imported request has installed nothing");
    decode(
        r#"{"phase":"Claimed","originating_run":"run-1","target_model":"model-b","denial_reason":"CapabilityPolicy"}"#,
    )
    .expect_err("a claimed request has refused nothing");
    decode(
        r#"{"phase":"Archived","originating_run":"run-1","target_model":"model-b","applied_model":"model-b"}"#,
    )
    .expect_err("an archived request has installed nothing");
}

#[test]
fn a_record_carrying_both_outcomes_is_refused() {
    decode(
        r#"{"phase":"Realized","originating_run":"run-1","target_model":"model-b","applied_model":"model-b","denial_reason":"CapabilityPolicy"}"#,
    )
    .expect_err("a request cannot be both installed and refused");
    decode(
        r#"{"phase":"Denied","originating_run":"run-1","target_model":"model-b","applied_model":"model-b","denial_reason":"CapabilityPolicy"}"#,
    )
    .expect_err("a refused request installed nothing");
}

/// A record that binds to no run or no target can never be matched to a
/// committed request, so it is refused at the boundary rather than admitted as
/// an inert row that quietly accumulates.
#[test]
fn a_record_without_an_identity_is_refused() {
    decode(r#"{"phase":"Imported","originating_run":"","target_model":"model-b"}"#)
        .expect_err("a record with no originating run binds to nothing");
    decode(r#"{"phase":"Imported","originating_run":"run-1","target_model":""}"#)
        .expect_err("a record with no target model binds to nothing");
}

#[test]
fn unknown_and_missing_fields_are_refused() {
    decode(
        r#"{"phase":"Imported","originating_run":"run-1","target_model":"model-b","surprise":true}"#,
    )
    .expect_err("an unknown field means the document was written by something else");
    decode(r#"{"originating_run":"run-1","target_model":"model-b"}"#)
        .expect_err("a record with no phase is not a record");
}

/// The constructors enforce the same identity invariants as the decoder.
///
/// Without this they could mint values that fail their own round-trip: safe
/// in memory, refused the moment machine state is recovered. Every phase is
/// covered because the empty-identity check lives in one shared place and a
/// future constructor could bypass it.
#[test]
fn constructors_refuse_a_record_that_binds_to_nothing() {
    assert_eq!(
        ModelRoutingHandoffRecord::imported("", TARGET),
        Err(ModelRoutingHandoffRecordError::MissingOriginatingRun)
    );
    assert_eq!(
        ModelRoutingHandoffRecord::imported(RUN, ""),
        Err(ModelRoutingHandoffRecordError::MissingTargetModel)
    );
    assert_eq!(
        ModelRoutingHandoffRecord::claimed("", TARGET),
        Err(ModelRoutingHandoffRecordError::MissingOriginatingRun)
    );
    assert_eq!(
        ModelRoutingHandoffRecord::archived(RUN, ""),
        Err(ModelRoutingHandoffRecordError::MissingTargetModel)
    );
    assert_eq!(
        ModelRoutingHandoffRecord::denied("", TARGET, RoutingDenialReason::CapabilityPolicy),
        Err(ModelRoutingHandoffRecordError::MissingOriginatingRun)
    );
    assert_eq!(
        ModelRoutingHandoffRecord::realized("", TARGET, TARGET),
        Err(ModelRoutingHandoffRecordError::MissingOriginatingRun)
    );
}

/// A realization that names no installed identity is not a realization — the
/// decoder already says so, and now the constructor does too.
#[test]
fn realized_refuses_an_empty_applied_identity() {
    assert_eq!(
        ModelRoutingHandoffRecord::realized(RUN, TARGET, ""),
        Err(ModelRoutingHandoffRecordError::MissingAppliedModel)
    );
}

/// Everything a constructor accepts must survive its own round-trip. This is
/// the property the two suites above jointly guarantee, asserted directly.
#[test]
fn every_constructed_record_round_trips_through_the_decoder() {
    let records = [
        valid(ModelRoutingHandoffRecord::imported(RUN, TARGET)),
        valid(ModelRoutingHandoffRecord::claimed(RUN, TARGET)),
        valid(ModelRoutingHandoffRecord::archived(RUN, TARGET)),
        valid(ModelRoutingHandoffRecord::realized(RUN, TARGET, TARGET)),
        valid(ModelRoutingHandoffRecord::denied(
            RUN,
            TARGET,
            RoutingDenialReason::CapabilityPolicy,
        )),
    ];
    for record in records {
        let encoded = serde_json::to_string(&record).expect("record serializes");
        let decoded = decode(&encoded).expect("a constructed record must decode");
        assert_eq!(decoded, record, "round-trip must preserve the record");
    }
}
