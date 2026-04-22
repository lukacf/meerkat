//! Phase 4 recovery tests: REQ-10, REQ-11, REQ-12, REQ-13
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use indexmap::IndexMap;
use meerkat_mob::generated::{flow_frame, flow_run, loop_iteration};
use meerkat_mob::ids::{FrameId, LoopId, LoopInstanceId, RunId, StepId};
use meerkat_mob::run::{
    FlowContext, FrameSnapshot, LoopContextHistory, LoopSnapshot, MobRun, MobRunStatus,
};
use meerkat_mob::runtime::recovery::{RestoreIncompatible, reconcile_run_state};
use std::collections::BTreeMap;

// ─── Helpers ────────────────────────────────────────────────────────────────

fn minimal_run_with_schema_v2() -> MobRun {
    let flow_state = flow_run::initial_state();
    MobRun {
        run_id: RunId::new(),
        mob_id: meerkat_mob::MobId::from("test-mob"),
        flow_id: meerkat_mob::FlowId::from("test-flow"),
        status: MobRunStatus::Running,
        flow_state,
        activation_params: serde_json::json!({}),
        created_at: chrono::Utc::now(),
        completed_at: None,
        step_ledger: vec![],
        failure_ledger: vec![],
        frames: BTreeMap::new(),
        loops: BTreeMap::new(),
        loop_iteration_ledger: vec![],
        schema_version: 4,
        root_step_outputs: IndexMap::new(),
        loop_iteration_outputs: BTreeMap::new(),
    }
}

/// Build a FrameSnapshot with a specific ready_queue and matching node_status.
fn frame_snapshot_with_ready_queue(_frame_id: &str, ready_nodes: &[&str]) -> FrameSnapshot {
    use meerkat_mob::FlowNodeId;
    let mut state = flow_frame::initial_state();
    state.phase = flow_frame::Phase::Running;
    state.ready_queue = ready_nodes.iter().map(|s| FlowNodeId::from(*s)).collect();
    state.tracked_nodes = ready_nodes.iter().map(|s| FlowNodeId::from(*s)).collect();
    state.node_status = ready_nodes
        .iter()
        .map(|s| (FlowNodeId::from(*s), flow_frame::NodeRunStatus::Ready))
        .collect();
    FrameSnapshot {
        kernel_state: state,
    }
}

/// Insert a frame_id into the ready_frames Seq and ready_frame_membership Set of flow_state.
fn insert_frame_to_ready_queue(flow_state: &mut flow_run::State, frame_id: &str) {
    use meerkat_mob::FrameId;
    flow_state.ready_frames.push(FrameId::from(frame_id));
    flow_state
        .ready_frame_membership
        .insert(FrameId::from(frame_id));
}

fn get_ready_frames_from_run_state(flow_state: &flow_run::State) -> Vec<meerkat_mob::FrameId> {
    flow_state.ready_frames.clone()
}

// ─── REQ-10 / CHOKE-04: FlowContext path resolution ─────────────────────────

/// REQ-10 / CHOKE-04: FlowContext accesses loop iteration outputs by path.
#[test]
fn test_flow_context_accesses_loop_iteration_outputs() {
    use meerkat_mob::runtime::path::resolve_context_path;

    let mut iter_outputs: IndexMap<StepId, serde_json::Value> = IndexMap::new();
    iter_outputs.insert(
        StepId::from("impl"),
        serde_json::json!({"result": "draft-v1"}),
    );

    let mut loop_outputs: IndexMap<LoopId, LoopContextHistory> = IndexMap::new();
    loop_outputs.insert(
        LoopId::from("review-loop"),
        LoopContextHistory {
            iterations: vec![iter_outputs],
        },
    );

    let ctx = FlowContext {
        run_id: RunId::new(),
        activation_params: serde_json::json!({}),
        step_outputs: IndexMap::new(),
        loop_outputs,
    };

    // Path: loops.review-loop.iterations.0.steps.impl.result
    let val = resolve_context_path(&ctx, "loops.review-loop.iterations.0.steps.impl.result");
    assert_eq!(val, Some(&serde_json::json!("draft-v1")));

    // Non-existent iteration index → None.
    let val2 = resolve_context_path(&ctx, "loops.review-loop.iterations.1.steps.impl.result");
    assert_eq!(val2, None);

    // Non-existent loop → None.
    let val3 = resolve_context_path(&ctx, "loops.other-loop.iterations.0.steps.impl.result");
    assert_eq!(val3, None);
}

/// Populated iteration 1 is accessible at its index — not just the out-of-bounds None case.
#[test]
fn test_flow_context_accesses_iteration_one_with_real_data() {
    use meerkat_mob::runtime::path::resolve_context_path;

    let mut iter0: IndexMap<StepId, serde_json::Value> = IndexMap::new();
    iter0.insert(StepId::from("review"), serde_json::json!({"passes": false}));

    let mut iter1: IndexMap<StepId, serde_json::Value> = IndexMap::new();
    iter1.insert(StepId::from("review"), serde_json::json!({"passes": true}));

    let mut loop_outputs: IndexMap<LoopId, LoopContextHistory> = IndexMap::new();
    loop_outputs.insert(
        LoopId::from("review-loop"),
        LoopContextHistory {
            iterations: vec![iter0, iter1],
        },
    );

    let ctx = FlowContext {
        run_id: RunId::new(),
        activation_params: serde_json::json!({}),
        step_outputs: IndexMap::new(),
        loop_outputs,
    };

    // iteration 0 — first attempt failed
    let v0 = resolve_context_path(&ctx, "loops.review-loop.iterations.0.steps.review.passes");
    assert_eq!(v0, Some(&serde_json::json!(false)));

    // iteration 1 — second attempt passed
    let v1 = resolve_context_path(&ctx, "loops.review-loop.iterations.1.steps.review.passes");
    assert_eq!(v1, Some(&serde_json::json!(true)));

    // out of bounds → None
    let v2 = resolve_context_path(&ctx, "loops.review-loop.iterations.2.steps.review.passes");
    assert_eq!(v2, None);
}

// ─── REQ-11 / CHOKE-05: Recovery drops stale ready_frames entries ────────────

/// REQ-11 / CHOKE-05: Recovery drops stale ready_frames entries.
#[test]
fn test_recovery_drops_stale_ready_frames() {
    let mut run = minimal_run_with_schema_v2();

    // Frame-1 with empty ready_queue (stale — should be removed after reconcile).
    let stale = frame_snapshot_with_ready_queue("frame-1", &[]);
    run.frames.insert(FrameId::from("frame-1"), stale);

    // Manually add frame-1 to ready_frames as a stale entry.
    insert_frame_to_ready_queue(&mut run.flow_state, "frame-1");

    reconcile_run_state(&mut run).expect("reconcile");

    let ready_frames = get_ready_frames_from_run_state(&run.flow_state);
    assert!(
        !ready_frames.contains(&FrameId::from("frame-1")),
        "Stale frame-1 should be removed from ready_frames; got: {ready_frames:?}"
    );
}

// ─── REQ-11 / CHOKE-05: Recovery adds missing ready_frames entries ───────────

/// REQ-11 / CHOKE-05: Recovery adds missing ready_frames entries.
#[test]
fn test_recovery_adds_missing_ready_frames() {
    let mut run = minimal_run_with_schema_v2();

    // Frame-2 with non-empty ready_queue → should be added to ready_frames.
    let active = frame_snapshot_with_ready_queue("frame-2", &["node-a"]);
    run.frames.insert(FrameId::from("frame-2"), active);

    // Do NOT insert frame-2 into ready_frames (simulating missing entry).

    reconcile_run_state(&mut run).expect("reconcile");

    let ready_frames = get_ready_frames_from_run_state(&run.flow_state);
    assert!(
        ready_frames.contains(&FrameId::from("frame-2")),
        "Missing frame-2 should be added to ready_frames; got: {ready_frames:?}"
    );
}

// ─── REQ-12: Recovery detects frame invariant violation ─────────────────────

/// REQ-12: Recovery validates per-frame local invariant; mismatch → RestoreIncompatible.
#[test]
fn test_recovery_invalid_frame_invariant() {
    let mut run = minimal_run_with_schema_v2();

    // Frame with node-a status=Ready but NOT in ready_queue → invariant violation.
    let bad_frame = FrameSnapshot {
        kernel_state: flow_frame::State {
            phase: flow_frame::Phase::Running,
            tracked_nodes: [meerkat_mob::FlowNodeId::from("node-a")]
                .into_iter()
                .collect(),
            node_status: BTreeMap::from([(
                meerkat_mob::FlowNodeId::from("node-a"),
                flow_frame::NodeRunStatus::Ready,
            )]),
            ..flow_frame::initial_state()
        },
    };
    run.frames.insert(FrameId::from("frame-bad"), bad_frame);

    let result = reconcile_run_state(&mut run);
    assert!(
        matches!(
            result,
            Err(RestoreIncompatible::FrameInvariantViolation { .. })
        ),
        "Should return FrameInvariantViolation, got: {result:?}"
    );
}

// ─── REQ-13: Pre-v3 runs are rejected with typed hard-cut error ──────────────

/// REQ-13: Active pre-v3 frame runs are rejected with a typed hard-cut error.
#[test]
fn test_hard_cut_rejects_pre_v3_run() {
    let mut run = minimal_run_with_schema_v2();
    run.schema_version = 2; // Simulate a pre-descriptor frame run.
    run.status = MobRunStatus::Running; // Active pre-v3 run.

    let result = reconcile_run_state(&mut run);
    assert!(
        matches!(result, Err(RestoreIncompatible::PreV3Schema { .. })),
        "Pre-v3 run should be rejected, got: {result:?}"
    );
}

/// REQ-13: A pre-v3 run in Pending state is accepted (not yet active).
#[test]
fn test_pre_v3_pending_run_is_accepted() {
    let mut run = minimal_run_with_schema_v2();
    run.schema_version = 2;
    run.status = MobRunStatus::Pending;

    let result = reconcile_run_state(&mut run);
    assert!(result.is_ok(), "Pending pre-v3 run should be accepted");
}

// ─── Recovery: pending_body_frame_loops reconciliation ─────────────────────

fn get_pending_body_frame_loops_from_run_state(
    flow_state: &flow_run::State,
) -> Vec<meerkat_mob::LoopInstanceId> {
    flow_state.pending_body_frame_loops.clone()
}

/// Recovery adds missing pending_body_frame_loops entries for loops in Running
/// phase with active_body_frame_id = None.
#[test]
fn test_recovery_adds_missing_pending_body_frame_loops() {
    let mut run = minimal_run_with_schema_v2();

    // A LoopSnapshot in Running phase with active_body_frame_id = None
    // → should be added to pending_body_frame_loops.
    let loop_snap = LoopSnapshot {
        kernel_state: loop_iteration::State {
            phase: loop_iteration::Phase::Running,
            active_body_frame_id: None,
            ..loop_iteration::initial_state()
        },
    };
    run.loops
        .insert(LoopInstanceId::from("loop-inst-1"), loop_snap);

    // Do NOT insert into pending_body_frame_loops (simulating missing entry).

    reconcile_run_state(&mut run).expect("reconcile");

    let pending = get_pending_body_frame_loops_from_run_state(&run.flow_state);
    assert!(
        pending.contains(&LoopInstanceId::from("loop-inst-1")),
        "Missing loop-inst-1 should be added to pending_body_frame_loops; got: {pending:?}"
    );
}

/// Recovery drops stale pending_body_frame_loops entries for loops whose
/// active_body_frame_id is already set (body frame already started).
#[test]
fn test_recovery_drops_stale_pending_body_frame_loops() {
    let mut run = minimal_run_with_schema_v2();

    // A LoopSnapshot in Running phase with active_body_frame_id = Some(frame_id)
    // → should NOT be in pending_body_frame_loops.
    let loop_snap = LoopSnapshot {
        kernel_state: loop_iteration::State {
            phase: loop_iteration::Phase::Running,
            active_body_frame_id: Some(meerkat_mob::FrameId::from("body-frame-1")),
            ..loop_iteration::initial_state()
        },
    };
    run.loops
        .insert(LoopInstanceId::from("loop-inst-2"), loop_snap);

    // Manually add loop-inst-2 to pending_body_frame_loops as a stale entry.
    run.flow_state.pending_body_frame_loops = vec![LoopInstanceId::from("loop-inst-2")];
    run.flow_state.pending_body_frame_loop_membership =
        [LoopInstanceId::from("loop-inst-2")].into_iter().collect();

    reconcile_run_state(&mut run).expect("reconcile");

    let pending = get_pending_body_frame_loops_from_run_state(&run.flow_state);
    assert!(
        !pending.contains(&LoopInstanceId::from("loop-inst-2")),
        "Stale loop-inst-2 should be removed from pending_body_frame_loops; got: {pending:?}"
    );
}
