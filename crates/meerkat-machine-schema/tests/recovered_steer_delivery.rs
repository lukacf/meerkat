//! Recovery and the delivery class of a steer.
//!
//! The machine tracks a delivery class for an input only while it is Queued
//! in the Steer lane. The class is written at admission and stored with the
//! input's row. Recovery hands the stored class back with the row's other
//! admission stamps, and the machine alone decides whether the recovered
//! input keeps it:
//!
//! * an input recovered Queued in the Steer lane that was never bound to a
//!   run and has no stage attempt never left that lane, so it keeps the
//!   class: it still waits for a boundary of the run that resumes;
//! * one with a stage attempt was staged and came back (a rollback, a
//!   replay, an unstageable deferral), and one that still names a run joined
//!   it and had its applied image discarded: each takes the queued path;
//! * an input in the Queue lane never holds a class;
//! * a governed input never joins a running run, so the binding that
//!   recovery restores after the lifecycle drops the class, and the
//!   controller hold, which refuses an input that holds one, stays
//!   available.
//!
//! The first group of rows hands the machine rows written by hand, one per
//! condition. The second group takes each row from the live transition that
//! produces it: the machine is driven to the state, the row it would store
//! is read from that state, and a machine that starts over recovers it with
//! the class the row still carries. Each condition has a row that only it
//! refuses. The condition `phase == Queued` has none: the lifecycle's own
//! guard gives every other phase no lane, so no row tells it apart from the
//! lane condition.
//!
//! One return to the lane leaves no mark on the row: a join returned as
//! never applied clears the run and counts no attempt. Such a row is
//! recovered like a steer that never left, and its row here says so.
//!
//! These are asserted against the generated kernel directly, so they hold
//! whatever shell drives it.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use meerkat_machine_schema::catalog::dsl::meerkat_machine::{
    AdmissionContinuationKind, AdmissionInputKind, AdmissionTurnAppendShape, ContentShape,
    InputLane, InputPhase, LiveBoundaryDelivery, LiveBoundaryJoinObservation,
    MeerkatMachineAuthority, MeerkatMachineInput, MeerkatMachineMutator, MeerkatMachineSignal,
    RecoveredInputKind, RecoveredRunApplyBoundary, RecoveredRuntimeExecutionKind, RunId,
    RuntimeEpochId, SessionId, TurnPrimitiveKind,
};

const SESSION: &str = "session-1";
const RUN: &str = "run-1";
const BATCH: &str = "input-batch";
const STEER: &str = "input-1";

fn run() -> RunId {
    RunId(RUN.to_string())
}

/// What one stored row hands back at recovery.
struct RecoveredRow {
    lane: InputLane,
    attempt_count: u64,
    run_id: Option<RunId>,
    admission_sequence: u64,
    stored_class: Option<LiveBoundaryDelivery>,
}

impl RecoveredRow {
    /// A steer that was admitted with the durable in-turn class and never
    /// left the Steer lane.
    fn waiting_steer() -> Self {
        Self {
            lane: InputLane::Steer,
            attempt_count: 0,
            run_id: None,
            admission_sequence: 7,
            stored_class: Some(LiveBoundaryDelivery::DurableAppend),
        }
    }
}

fn apply(authority: &mut MeerkatMachineAuthority, input: MeerkatMachineInput) {
    MeerkatMachineMutator::apply(authority, input).expect("the machine accepts the input");
}

/// A registered, idle machine.
fn registered() -> MeerkatMachineAuthority {
    let mut authority = MeerkatMachineAuthority::new();
    authority
        .apply_signal(MeerkatMachineSignal::Initialize)
        .expect("machine initializes");
    apply(
        &mut authority,
        MeerkatMachineInput::RegisterSession {
            session_id: SessionId(SESSION.to_string()),
            runtime_epoch_id: Some(RuntimeEpochId("epoch-1".to_string())),
            initial_run_start_holds: std::collections::BTreeSet::new(),
        },
    );
    authority
}

/// Recover `row` as a Queued peer message on `authority`, as recovery does:
/// the admission witness, then the lifecycle.
fn recover_on(authority: &mut MeerkatMachineAuthority, row: RecoveredRow) {
    apply(
        authority,
        MeerkatMachineInput::RecoverAdmittedInput {
            input_id: STEER.to_string(),
            input_kind: RecoveredInputKind::PeerMessage,
            runtime_boundary: RecoveredRunApplyBoundary::RunCheckpoint,
            runtime_execution_kind: RecoveredRuntimeExecutionKind::ContentTurn,
            runtime_peer_response_terminal_apply_intent: None,
            lane: row.lane,
        },
    );
    apply(
        authority,
        MeerkatMachineInput::RecoverInputLifecycle {
            input_id: STEER.to_string(),
            phase: InputPhase::Queued,
            terminal_kind: None,
            superseded_by: None,
            aggregate_id: None,
            abandon_reason: None,
            abandon_attempt_count: 0,
            attempt_count: row.attempt_count,
            run_id: row.run_id,
            boundary_sequence: None,
            admission_sequence: Some(row.admission_sequence),
            admission_sequence_recovery: None,
            recovery_lane: Some(row.lane),
            lane: Some(row.lane),
            runtime_boundary: Some(RecoveredRunApplyBoundary::RunCheckpoint),
            runtime_execution_kind: Some(RecoveredRuntimeExecutionKind::ContentTurn),
            runtime_peer_response_terminal_apply_intent: None,
            is_prompt: false,
            live_boundary_delivery: row.stored_class,
        },
    );
}

/// A machine that starts over and recovers `row`.
fn recovered(row: RecoveredRow) -> MeerkatMachineAuthority {
    let mut authority = registered();
    recover_on(&mut authority, row);
    authority
}

fn tracked_class(authority: &MeerkatMachineAuthority) -> Option<LiveBoundaryDelivery> {
    authority
        .state()
        .input_live_boundary_delivery
        .get(STEER)
        .copied()
}

// ---------------------------------------------------------------------------
// Rows written by hand, one per condition.
// ---------------------------------------------------------------------------

#[test]
fn a_recovered_steer_that_never_left_its_lane_keeps_its_delivery_class() {
    let authority = recovered(RecoveredRow::waiting_steer());
    assert_eq!(
        tracked_class(&authority),
        Some(LiveBoundaryDelivery::DurableAppend)
    );
    assert_eq!(
        authority.state().input_admission_seq.get(STEER).copied(),
        Some(7),
        "and its place in the admission order"
    );
}

#[test]
fn a_recovered_steer_whose_row_stores_no_class_tracks_none() {
    let authority = recovered(RecoveredRow {
        stored_class: None,
        ..RecoveredRow::waiting_steer()
    });
    assert_eq!(tracked_class(&authority), None);
}

#[test]
fn a_recovered_steer_with_a_stage_attempt_keeps_no_delivery_class() {
    let authority = recovered(RecoveredRow {
        attempt_count: 1,
        ..RecoveredRow::waiting_steer()
    });
    assert_eq!(tracked_class(&authority), None);
}

#[test]
fn a_recovered_steer_that_still_names_a_run_keeps_no_delivery_class() {
    let authority = recovered(RecoveredRow {
        run_id: Some(run()),
        ..RecoveredRow::waiting_steer()
    });
    assert_eq!(tracked_class(&authority), None);
}

#[test]
fn a_recovered_queue_lane_input_keeps_no_delivery_class() {
    let authority = recovered(RecoveredRow {
        lane: InputLane::Queue,
        ..RecoveredRow::waiting_steer()
    });
    assert_eq!(tracked_class(&authority), None);
}

#[test]
fn a_recovered_governed_steer_keeps_no_delivery_class_and_can_be_held() {
    let mut authority = recovered(RecoveredRow::waiting_steer());
    assert_eq!(
        tracked_class(&authority),
        Some(LiveBoundaryDelivery::DurableAppend),
        "the lifecycle is recovered before the row's binding is"
    );
    apply(
        &mut authority,
        MeerkatMachineInput::BindInputAuthority {
            input_id: STEER.to_string(),
            authority_binding: "association-1".to_string(),
            authority_batch_key: "batch-1".to_string(),
        },
    );
    assert_eq!(
        tracked_class(&authority),
        None,
        "a governed input never joins a running run"
    );
    apply(
        &mut authority,
        MeerkatMachineInput::HoldInputController {
            input_id: STEER.to_string(),
        },
    );
    assert!(authority.state().input_controller_holds.contains(STEER));
}

// ---------------------------------------------------------------------------
// Rows taken from the live transition that produces them.
// ---------------------------------------------------------------------------

/// Admit the steer as a peer message into the Steer lane with the durable
/// in-turn class, while a run is running (`running`) or while the machine is
/// idle.
fn admit_steer(authority: &mut MeerkatMachineAuthority, running: bool) {
    apply(
        authority,
        MeerkatMachineInput::ResolveAdmissionPlan {
            input_id: STEER.to_string(),
            authority_binding: None,
            authority_batch_key: None,
            input_kind: AdmissionInputKind::PeerMessage,
            requested_lane: Some(InputLane::Steer),
            continuation_kind: AdmissionContinuationKind::Ordinary,
            turn_append_shape: AdmissionTurnAppendShape::InTurnEligible,
            silent_intent_match: false,
            existing_superseded_input_id: None,
            runtime_running: running,
            active_turn_boundary_available: running,
            without_wake: false,
        },
    );
    apply(
        authority,
        MeerkatMachineInput::SteerAccepted {
            input_id: STEER.to_string(),
        },
    );
    assert_eq!(
        tracked_class(authority),
        Some(LiveBoundaryDelivery::DurableAppend),
        "admission gives the steer the durable in-turn class"
    );
}

/// A machine whose runtime-loop run holds one staged batch input and stands
/// at a model boundary, with the steer admitted during the run.
fn steer_waiting_at_a_model_boundary() -> MeerkatMachineAuthority {
    let mut authority = registered();
    apply(
        &mut authority,
        MeerkatMachineInput::ResolveAdmissionPlan {
            input_id: BATCH.to_string(),
            authority_binding: None,
            authority_batch_key: None,
            input_kind: AdmissionInputKind::Prompt,
            requested_lane: None,
            continuation_kind: AdmissionContinuationKind::Ordinary,
            turn_append_shape: AdmissionTurnAppendShape::InTurnEligible,
            silent_intent_match: false,
            existing_superseded_input_id: None,
            runtime_running: false,
            active_turn_boundary_available: false,
            without_wake: false,
        },
    );
    apply(
        &mut authority,
        MeerkatMachineInput::QueueAccepted {
            input_id: BATCH.to_string(),
        },
    );
    apply(
        &mut authority,
        MeerkatMachineInput::Prepare {
            session_id: SessionId(SESSION.to_string()),
            run_id: run(),
        },
    );
    apply(
        &mut authority,
        MeerkatMachineInput::StageForRun {
            input_id: BATCH.to_string(),
            run_id: run(),
        },
    );
    apply(
        &mut authority,
        MeerkatMachineInput::StartConversationRun {
            run_id: run(),
            primitive_kind: TurnPrimitiveKind::ConversationTurn,
            admitted_content_shape: ContentShape::Conversation,
            vision_enabled: false,
            image_tool_results_enabled: false,
            max_extraction_retries: 0,
        },
    );
    apply(
        &mut authority,
        MeerkatMachineInput::PrimitiveApplied { run_id: run() },
    );
    admit_steer(&mut authority, true);
    authority
}

/// The steer joins the run at its boundary and the run's terminal resolves
/// the join with `observation`.
fn joined_and_resolved(observation: LiveBoundaryJoinObservation) -> MeerkatMachineAuthority {
    let mut authority = steer_waiting_at_a_model_boundary();
    apply(
        &mut authority,
        MeerkatMachineInput::JoinLiveBoundaryDurableAppend {
            run_id: run(),
            input_id: STEER.to_string(),
        },
    );
    assert_eq!(
        authority.state().input_phases.get(STEER).copied(),
        Some(InputPhase::Staged)
    );
    apply(
        &mut authority,
        MeerkatMachineInput::ResolveLiveBoundaryDurableAppendJoin {
            run_id: run(),
            input_id: STEER.to_string(),
            lane: InputLane::Steer,
            observation,
        },
    );
    authority
}

/// The row the live machine holds for the steer, as its store would keep it,
/// with the class the stored row still carries: the runtime's copy of the
/// class is written at admission and is not rewritten when the machine
/// removes its own.
fn stored_row(live: &MeerkatMachineAuthority) -> RecoveredRow {
    let state = live.state();
    assert_eq!(
        state.input_phases.get(STEER).copied(),
        Some(InputPhase::Queued),
        "the steer is back in a lane"
    );
    let lane = state
        .input_lane
        .get(STEER)
        .copied()
        .expect("a queued input has a lane");
    assert_eq!(
        state.input_recovery_lanes.get(STEER).copied(),
        Some(lane),
        "and that lane is its recovery lane"
    );
    assert_eq!(
        tracked_class(live),
        None,
        "the live machine removed the class when the steer left the lane"
    );
    RecoveredRow {
        lane,
        attempt_count: state.input_attempt_counts.get(STEER).copied().unwrap_or(0),
        run_id: state.input_run_associations.get(STEER).cloned(),
        admission_sequence: state
            .input_admission_seq
            .get(STEER)
            .copied()
            .expect("a queued input has an admission sequence"),
        stored_class: Some(LiveBoundaryDelivery::DurableAppend),
    }
}

/// The condition that carries exactly once. A join whose append the runner
/// applied and whose image was then discarded returns to the Steer lane and
/// keeps its run, with no stage attempt: the run is the only mark that the
/// append may sit in a tail a later start adopts. Such a row never gets its
/// class back, so it can never join a run again.
#[test]
fn a_join_whose_applied_image_was_discarded_gets_no_class_back() {
    let live = joined_and_resolved(LiveBoundaryJoinObservation::AppliedDiscarded);
    let row = stored_row(&live);
    assert_eq!(row.lane, InputLane::Steer);
    assert_eq!(row.attempt_count, 0, "a join counts no stage attempt");
    assert_eq!(row.run_id, Some(run()), "the discarded join keeps its run");
    assert_eq!(tracked_class(&recovered(row)), None);
}

/// A join returned as never applied clears its run and counts no attempt, so
/// its row is the row of a steer that never left the lane, and it is
/// recovered as one: it gets its class back. Its append was never written,
/// so a second join delivers it once.
#[test]
fn a_join_returned_as_never_applied_gets_its_class_back() {
    let live = joined_and_resolved(LiveBoundaryJoinObservation::NotApplied);
    let row = stored_row(&live);
    assert_eq!(row.lane, InputLane::Steer);
    assert_eq!(row.attempt_count, 0);
    assert_eq!(row.run_id, None, "the unapplied join clears its run");
    assert_eq!(
        tracked_class(&recovered(row)),
        Some(LiveBoundaryDelivery::DurableAppend)
    );
}

/// A steer whose boundary was unavailable is normalized to the Queue lane.
/// The runtime clears its stored class in the same step; a row that still
/// carried one would get none back.
#[test]
fn a_steer_normalized_to_the_queue_lane_gets_no_class_back() {
    let mut live = steer_waiting_at_a_model_boundary();
    apply(
        &mut live,
        MeerkatMachineInput::LiveBoundaryUnavailable {
            input_id: STEER.to_string(),
        },
    );
    let row = stored_row(&live);
    assert_eq!(row.lane, InputLane::Queue);
    assert_eq!(row.attempt_count, 0);
    assert_eq!(row.run_id, None);
    assert_eq!(tracked_class(&recovered(row)), None);
}

/// A steer that started a run and was rolled back to its lane counts the
/// stage attempt: it takes the queued path after a restart, as it does
/// before one.
#[test]
fn a_steer_staged_and_rolled_back_gets_no_class_back() {
    let mut live = registered();
    admit_steer(&mut live, false);
    apply(
        &mut live,
        MeerkatMachineInput::Prepare {
            session_id: SessionId(SESSION.to_string()),
            run_id: run(),
        },
    );
    apply(
        &mut live,
        MeerkatMachineInput::StageForRun {
            input_id: STEER.to_string(),
            run_id: run(),
        },
    );
    apply(
        &mut live,
        MeerkatMachineInput::ResolveStagedRollback {
            input_id: STEER.to_string(),
            lane: InputLane::Steer,
        },
    );
    let row = stored_row(&live);
    assert_eq!(row.lane, InputLane::Steer);
    assert_eq!(row.attempt_count, 1, "staging counted the attempt");
    assert_eq!(row.run_id, None, "the rollback cleared the run");
    assert_eq!(tracked_class(&recovered(row)), None);
}

// ---------------------------------------------------------------------------
// A recycle recovers the machine's live inputs again, on the same machine.
// ---------------------------------------------------------------------------

/// A recycle is taken only while no run is current. It recovers a queued
/// steer again from its row, and the steer keeps its class; the next run
/// then stages it as its own input, which removes the class.
#[test]
fn a_recycle_keeps_the_class_of_a_queued_steer_until_a_run_stages_it() {
    let mut authority = registered();
    admit_steer(&mut authority, false);
    let admission_sequence = authority
        .state()
        .input_admission_seq
        .get(STEER)
        .copied()
        .expect("admitted");
    apply(&mut authority, MeerkatMachineInput::Recycle);
    recover_on(
        &mut authority,
        RecoveredRow {
            admission_sequence,
            ..RecoveredRow::waiting_steer()
        },
    );
    assert_eq!(
        tracked_class(&authority),
        Some(LiveBoundaryDelivery::DurableAppend),
        "the recycled steer keeps the class it was admitted with"
    );
    apply(
        &mut authority,
        MeerkatMachineInput::Prepare {
            session_id: SessionId(SESSION.to_string()),
            run_id: run(),
        },
    );
    apply(
        &mut authority,
        MeerkatMachineInput::StageForRun {
            input_id: STEER.to_string(),
            run_id: run(),
        },
    );
    assert_eq!(tracked_class(&authority), None, "staging removes the class");
}

#[test]
fn a_recycle_is_refused_while_a_run_is_current() {
    let mut authority = steer_waiting_at_a_model_boundary();
    assert!(
        MeerkatMachineMutator::apply(&mut authority, MeerkatMachineInput::Recycle).is_err(),
        "the machine takes no recycle beside a running run"
    );
    assert_eq!(
        tracked_class(&authority),
        Some(LiveBoundaryDelivery::DurableAppend),
        "and the waiting steer is untouched"
    );
}
