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
//! These are asserted against the generated kernel directly, so they hold
//! whatever shell drives it.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use meerkat_machine_schema::catalog::dsl::meerkat_machine::{
    InputLane, InputPhase, LiveBoundaryDelivery, MeerkatMachineAuthority, MeerkatMachineInput,
    MeerkatMachineMutator, MeerkatMachineSignal, RecoveredInputKind, RecoveredRunApplyBoundary,
    RecoveredRuntimeExecutionKind, RunId, SessionId,
};

const STEER: &str = "input-1";

/// What one stored row hands back at recovery.
struct RecoveredRow {
    lane: InputLane,
    attempt_count: u64,
    run_id: Option<RunId>,
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
            stored_class: Some(LiveBoundaryDelivery::DurableAppend),
        }
    }
}

/// A registered machine that recovered `row` as a Queued peer message.
fn recovered(row: RecoveredRow) -> MeerkatMachineAuthority {
    let mut authority = MeerkatMachineAuthority::new();
    authority
        .apply_signal(MeerkatMachineSignal::Initialize)
        .expect("machine initializes");
    MeerkatMachineMutator::apply(
        &mut authority,
        MeerkatMachineInput::RegisterSession {
            session_id: SessionId("session-1".to_string()),
            runtime_epoch_id: None,
            initial_run_start_holds: std::collections::BTreeSet::new(),
        },
    )
    .expect("session registers");
    MeerkatMachineMutator::apply(
        &mut authority,
        MeerkatMachineInput::RecoverAdmittedInput {
            input_id: STEER.to_string(),
            input_kind: RecoveredInputKind::PeerMessage,
            runtime_boundary: RecoveredRunApplyBoundary::RunCheckpoint,
            runtime_execution_kind: RecoveredRuntimeExecutionKind::ContentTurn,
            runtime_peer_response_terminal_apply_intent: None,
            lane: row.lane,
        },
    )
    .expect("the recovered admission witness is accepted");
    MeerkatMachineMutator::apply(
        &mut authority,
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
            admission_sequence: Some(7),
            admission_sequence_recovery: None,
            recovery_lane: Some(row.lane),
            lane: Some(row.lane),
            runtime_boundary: Some(RecoveredRunApplyBoundary::RunCheckpoint),
            runtime_execution_kind: Some(RecoveredRuntimeExecutionKind::ContentTurn),
            runtime_peer_response_terminal_apply_intent: None,
            is_prompt: false,
            live_boundary_delivery: row.stored_class,
        },
    )
    .expect("the recovered lifecycle is accepted");
    authority
}

fn tracked_class(authority: &MeerkatMachineAuthority) -> Option<LiveBoundaryDelivery> {
    authority
        .state()
        .input_live_boundary_delivery
        .get(STEER)
        .copied()
}

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
        run_id: Some(RunId("run-1".to_string())),
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
    MeerkatMachineMutator::apply(
        &mut authority,
        MeerkatMachineInput::BindInputAuthority {
            input_id: STEER.to_string(),
            authority_binding: "association-1".to_string(),
            authority_batch_key: "batch-1".to_string(),
        },
    )
    .expect("the recovered row's binding is restored");
    assert_eq!(
        tracked_class(&authority),
        None,
        "a governed input never joins a running run"
    );
    MeerkatMachineMutator::apply(
        &mut authority,
        MeerkatMachineInput::HoldInputController {
            input_id: STEER.to_string(),
        },
    )
    .expect("the kept governed steer can wait for its controller");
    assert!(authority.state().input_controller_holds.contains(STEER));
}
