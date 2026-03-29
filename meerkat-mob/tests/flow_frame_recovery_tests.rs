//! Phase 4 recovery tests: REQ-10, REQ-11, REQ-12, REQ-13
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use indexmap::IndexMap;
use meerkat_machine_kernels::{KernelState, KernelValue};
use meerkat_mob::ids::{FrameId, LoopId, RunId, StepId};
use meerkat_mob::run::{FlowContext, FrameSnapshot, LoopContextHistory, MobRun, MobRunStatus};
use meerkat_mob::runtime::recovery::{RestoreIncompatible, reconcile_run_state};
use std::collections::{BTreeMap, BTreeSet};

// ─── Helpers ────────────────────────────────────────────────────────────────

fn minimal_run_with_schema_v2() -> MobRun {
    use meerkat_machine_kernels::generated::flow_run;
    let flow_state = flow_run::initial_state().expect("init");
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
        schema_version: 2,
        root_step_outputs: IndexMap::new(),
        loop_iteration_outputs: BTreeMap::new(),
    }
}

/// Build a FrameSnapshot with a specific ready_queue and matching node_status.
fn frame_snapshot_with_ready_queue(frame_id: &str, ready_nodes: &[&str]) -> FrameSnapshot {
    let ready_queue_seq = KernelValue::Seq(
        ready_nodes
            .iter()
            .map(|s| KernelValue::String(s.to_string()))
            .collect(),
    );
    let tracked: BTreeSet<KernelValue> = ready_nodes
        .iter()
        .map(|s| KernelValue::String(s.to_string()))
        .collect();
    let node_status: BTreeMap<KernelValue, KernelValue> = ready_nodes
        .iter()
        .map(|s| {
            (
                KernelValue::String(s.to_string()),
                KernelValue::NamedVariant {
                    enum_name: "NodeRunStatus".into(),
                    variant: "Ready".into(),
                },
            )
        })
        .collect();

    FrameSnapshot {
        frame_id: FrameId::from(frame_id),
        kernel_state: KernelState {
            phase: "Running".into(),
            fields: BTreeMap::from([
                ("ready_queue".into(), ready_queue_seq),
                ("tracked_nodes".into(), KernelValue::Set(tracked)),
                ("node_status".into(), KernelValue::Map(node_status)),
            ]),
        },
    }
}

/// Insert a frame_id into the ready_frames Seq and ready_frame_membership Set of flow_state.
fn insert_frame_to_ready_queue(flow_state: &mut KernelState, frame_id: &str) {
    if let Some(KernelValue::Seq(seq)) = flow_state.fields.get_mut("ready_frames") {
        seq.push(KernelValue::String(frame_id.to_string()));
    }
    if let Some(KernelValue::Set(set)) = flow_state.fields.get_mut("ready_frame_membership") {
        set.insert(KernelValue::String(frame_id.to_string()));
    }
}

fn get_ready_frames_from_run_state(flow_state: &KernelState) -> Vec<String> {
    match flow_state.fields.get("ready_frames") {
        Some(KernelValue::Seq(seq)) => seq
            .iter()
            .filter_map(|v| {
                if let KernelValue::String(s) = v {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .collect(),
        _ => vec![],
    }
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
        !ready_frames.contains(&"frame-1".to_string()),
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
        ready_frames.contains(&"frame-2".to_string()),
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
        frame_id: FrameId::from("frame-bad"),
        kernel_state: KernelState {
            phase: "Running".into(),
            fields: BTreeMap::from([
                (
                    "ready_queue".into(),
                    KernelValue::Seq(vec![]), // empty — node-a is NOT here
                ),
                (
                    "tracked_nodes".into(),
                    KernelValue::Set([KernelValue::String("node-a".into())].into_iter().collect()),
                ),
                (
                    "node_status".into(),
                    KernelValue::Map(BTreeMap::from([(
                        KernelValue::String("node-a".into()),
                        // node-a has Ready status but is NOT in ready_queue → VIOLATION
                        KernelValue::NamedVariant {
                            enum_name: "NodeRunStatus".into(),
                            variant: "Ready".into(),
                        },
                    )])),
                ),
            ]),
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

// ─── REQ-13: Pre-v2 runs are rejected with typed hard-cut error ──────────────

/// REQ-13: Pre-v2 pending/running runs rejected with a typed hard-cut error.
#[test]
fn test_hard_cut_rejects_pre_v2_run() {
    let mut run = minimal_run_with_schema_v2();
    run.schema_version = 0; // Simulate a legacy run.
    run.status = MobRunStatus::Running; // Active pre-v2 run.

    let result = reconcile_run_state(&mut run);
    assert!(
        matches!(result, Err(RestoreIncompatible::PreV2Schema { .. })),
        "Pre-v2 run should be rejected, got: {result:?}"
    );
}

/// REQ-13: A pre-v2 run in Pending state is accepted (not yet active).
#[test]
fn test_pre_v2_pending_run_is_accepted() {
    let mut run = minimal_run_with_schema_v2();
    run.schema_version = 0;
    run.status = MobRunStatus::Pending;

    let result = reconcile_run_state(&mut run);
    assert!(result.is_ok(), "Pending pre-v2 run should be accepted");
}
