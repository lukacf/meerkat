//! Recovery reconciliation for FlowRun state after crash/restart.
//!
//! `reconcile_run_state` must be called before resuming a run to ensure the
//! scheduler's `ready_frames` and `pending_body_frame_loops` fields are
//! consistent with the per-frame and per-loop kernel snapshots.

use crate::ids::FrameId;
use crate::run::{FrameSnapshot, LoopSnapshot, MobRun, MobRunStatus};
use meerkat_machine_kernels::KernelValue;

/// Errors that prevent a run from being resumed.
#[derive(Debug, thiserror::Error)]
pub enum RestoreIncompatible {
    #[error("cannot resume pre-v2 run: schema_version={schema_version}")]
    PreV2Schema { schema_version: u32 },
    #[error("frame invariant violated: frame {frame_id} has Ready nodes not in ready_queue")]
    FrameInvariantViolation { frame_id: FrameId },
    #[error("stale queue entries could not be reconciled")]
    StaleQueueState,
}

/// Reconcile run scheduler state from frame/loop snapshots.
///
/// Must be called before resuming a run after a crash or restart.
/// Returns `Err(RestoreIncompatible)` if the persisted state is irrecoverable.
pub fn reconcile_run_state(run: &mut MobRun) -> Result<(), RestoreIncompatible> {
    // 1. Pre-v2 check: reject active pre-v2 runs.
    if run.schema_version < 2 && run.status != MobRunStatus::Pending && !run.status.is_terminal() {
        return Err(RestoreIncompatible::PreV2Schema {
            schema_version: run.schema_version,
        });
    }

    // 2. Validate per-frame local invariant before reconciling.
    for (frame_id, frame_snap) in &run.frames {
        check_frame_invariant(frame_id, frame_snap)?;
    }

    // 3. Reconcile ready_frames in flow_state from frame snapshots.
    //
    //    A frame belongs in ready_frames iff its ready_queue is non-empty.
    //    We rebuild both ready_frames (Seq) and ready_frame_membership (Set)
    //    from scratch to avoid partial state.
    reconcile_ready_frames(run);

    // 4. Reconcile pending_body_frame_loops in flow_state from loop snapshots.
    reconcile_pending_body_frame_loops(run);

    Ok(())
}

/// Check the per-frame invariant: node_status[n] == Ready ↔ n ∈ ready_queue.
fn check_frame_invariant(
    frame_id: &FrameId,
    snap: &FrameSnapshot,
) -> Result<(), RestoreIncompatible> {
    let fields = &snap.kernel_state.fields;

    // Collect the set of node IDs with status == Ready.
    let ready_by_status: std::collections::BTreeSet<String> = match fields.get("node_status") {
        Some(KernelValue::Map(map)) => map
            .iter()
            .filter_map(|(k, v)| {
                if is_ready_variant(v) {
                    kernel_value_string(k)
                } else {
                    None
                }
            })
            .collect(),
        _ => std::collections::BTreeSet::new(),
    };

    // Collect the set of node IDs in ready_queue.
    let in_ready_queue: std::collections::BTreeSet<String> = match fields.get("ready_queue") {
        Some(KernelValue::Seq(seq)) => seq.iter().filter_map(kernel_value_string).collect(),
        _ => std::collections::BTreeSet::new(),
    };

    // Invariant: ready_by_status == in_ready_queue.
    if ready_by_status != in_ready_queue {
        return Err(RestoreIncompatible::FrameInvariantViolation {
            frame_id: frame_id.clone(),
        });
    }

    Ok(())
}

/// Rebuild `ready_frames` and `ready_frame_membership` in `run.flow_state`
/// from the per-frame snapshots.
///
/// A frame is active (belongs in ready_frames) iff:
/// - it exists in `run.frames`, AND
/// - its `ready_queue` is non-empty.
fn reconcile_ready_frames(run: &mut MobRun) {
    // Compute the correct set of active frame IDs (non-empty ready_queue).
    let mut active_frame_ids: Vec<String> = run
        .frames
        .iter()
        .filter(|(_, snap)| frame_has_nonempty_ready_queue(snap))
        .map(|(frame_id, _)| frame_id.to_string())
        .collect();

    // Preserve stable ordering (BTreeMap gives us sorted keys).
    active_frame_ids.sort();

    let new_seq = KernelValue::Seq(
        active_frame_ids
            .iter()
            .map(|s| KernelValue::String(s.clone()))
            .collect(),
    );
    let new_set = KernelValue::Set(
        active_frame_ids
            .iter()
            .map(|s| KernelValue::String(s.clone()))
            .collect(),
    );

    run.flow_state
        .fields
        .insert("ready_frames".to_string(), new_seq);
    run.flow_state
        .fields
        .insert("ready_frame_membership".to_string(), new_set);
}

/// Rebuild `pending_body_frame_loops` and `pending_body_frame_loop_membership`
/// in `run.flow_state` from the per-loop snapshots.
///
/// A loop instance belongs in pending_body_frame_loops iff:
/// - it exists in `run.loops`
/// - its kernel state phase is "Running"
/// - its `active_body_frame_id` field is `None` (requested body frame but not yet started)
fn reconcile_pending_body_frame_loops(run: &mut MobRun) {
    let mut pending_loop_ids: Vec<String> = run
        .loops
        .iter()
        .filter(|(_, snap)| loop_is_pending_body_frame(snap))
        .map(|(loop_id, _)| loop_id.to_string())
        .collect();
    pending_loop_ids.sort();

    let new_seq = KernelValue::Seq(
        pending_loop_ids
            .iter()
            .map(|s| KernelValue::String(s.clone()))
            .collect(),
    );
    let new_set = KernelValue::Set(
        pending_loop_ids
            .iter()
            .map(|s| KernelValue::String(s.clone()))
            .collect(),
    );

    run.flow_state
        .fields
        .insert("pending_body_frame_loops".to_string(), new_seq);
    run.flow_state
        .fields
        .insert("pending_body_frame_loop_membership".to_string(), new_set);
}

/// Return true if a loop snapshot represents a loop that is pending a body frame.
fn loop_is_pending_body_frame(snap: &LoopSnapshot) -> bool {
    if snap.kernel_state.phase != "Running" {
        return false;
    }
    matches!(
        snap.kernel_state.fields.get("active_body_frame_id"),
        Some(KernelValue::None) | None
    )
}

/// Return true if the frame's `ready_queue` is present and non-empty.
fn frame_has_nonempty_ready_queue(snap: &FrameSnapshot) -> bool {
    match snap.kernel_state.fields.get("ready_queue") {
        Some(KernelValue::Seq(seq)) => !seq.is_empty(),
        _ => false,
    }
}

/// Return true if a KernelValue is a `NamedVariant { variant: "Ready" }`.
fn is_ready_variant(v: &KernelValue) -> bool {
    matches!(v, KernelValue::NamedVariant { variant, .. } if variant == "Ready")
}

/// Extract the inner string from a `KernelValue::String`.
fn kernel_value_string(v: &KernelValue) -> Option<String> {
    match v {
        KernelValue::String(s) => Some(s.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::RunId;
    use crate::run::{MobRun, MobRunStatus};
    use meerkat_machine_kernels::{KernelState, KernelValue};
    use std::collections::BTreeMap;

    fn minimal_v2_run_running() -> MobRun {
        use meerkat_machine_kernels::generated::flow_run;
        let flow_state = flow_run::initial_state().expect("init");
        MobRun {
            run_id: RunId::new(),
            mob_id: crate::MobId::from("test-mob"),
            flow_id: crate::FlowId::from("test-flow"),
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
            root_step_outputs: indexmap::IndexMap::new(),
            loop_iteration_outputs: indexmap::IndexMap::new(),
        }
    }

    fn frame_snapshot_with_ready_queue(
        _frame_id: &str,
        ready_nodes: Vec<&str>,
        all_nodes_ready: bool,
    ) -> FrameSnapshot {
        let ready_queue = KernelValue::Seq(
            ready_nodes
                .iter()
                .map(|s| KernelValue::String(s.to_string()))
                .collect(),
        );
        let tracked: std::collections::BTreeSet<KernelValue> = ready_nodes
            .iter()
            .map(|s| KernelValue::String(s.to_string()))
            .collect();
        let node_status = if all_nodes_ready {
            KernelValue::Map(
                ready_nodes
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
                    .collect(),
            )
        } else {
            KernelValue::Map(BTreeMap::new())
        };

        FrameSnapshot {
            kernel_state: KernelState {
                phase: "Running".into(),
                fields: BTreeMap::from([
                    ("ready_queue".into(), ready_queue),
                    ("tracked_nodes".into(), KernelValue::Set(tracked)),
                    ("node_status".into(), node_status),
                ]),
            },
        }
    }

    #[test]
    fn test_pre_v2_pending_run_is_accepted() {
        // Pending runs (not active) are fine even at schema_version 0.
        let mut run = minimal_v2_run_running();
        run.schema_version = 0;
        run.status = MobRunStatus::Pending;
        assert!(reconcile_run_state(&mut run).is_ok());
    }

    #[test]
    fn test_pre_v2_running_run_is_rejected() {
        let mut run = minimal_v2_run_running();
        run.schema_version = 0;
        run.status = MobRunStatus::Running;
        let result = reconcile_run_state(&mut run);
        assert!(
            matches!(result, Err(RestoreIncompatible::PreV2Schema { .. })),
            "Expected PreV2Schema, got: {result:?}"
        );
    }

    #[test]
    fn test_empty_run_reconciles_ok() {
        let mut run = minimal_v2_run_running();
        assert!(reconcile_run_state(&mut run).is_ok());
    }

    #[test]
    fn test_frame_invariant_valid_empty_queue() {
        // Frame with empty ready_queue and no Ready nodes is valid.
        let frame_id = crate::FrameId::from("f1");
        let snap = FrameSnapshot {
            kernel_state: KernelState {
                phase: "Running".into(),
                fields: BTreeMap::from([
                    ("ready_queue".into(), KernelValue::Seq(vec![])),
                    (
                        "node_status".into(),
                        KernelValue::Map(BTreeMap::from([(
                            KernelValue::String("node-a".into()),
                            KernelValue::NamedVariant {
                                enum_name: "NodeRunStatus".into(),
                                variant: "Running".into(),
                            },
                        )])),
                    ),
                ]),
            },
        };
        assert!(check_frame_invariant(&frame_id, &snap).is_ok());
    }

    #[test]
    fn test_frame_invariant_violation_ready_not_in_queue() {
        let frame_id = crate::FrameId::from("f-bad");
        let snap = FrameSnapshot {
            kernel_state: KernelState {
                phase: "Running".into(),
                fields: BTreeMap::from([
                    ("ready_queue".into(), KernelValue::Seq(vec![])),
                    (
                        "node_status".into(),
                        KernelValue::Map(BTreeMap::from([(
                            KernelValue::String("node-a".into()),
                            KernelValue::NamedVariant {
                                enum_name: "NodeRunStatus".into(),
                                variant: "Ready".into(),
                            },
                        )])),
                    ),
                ]),
            },
        };
        let result = check_frame_invariant(&frame_id, &snap);
        assert!(
            matches!(
                result,
                Err(RestoreIncompatible::FrameInvariantViolation { .. })
            ),
            "Expected FrameInvariantViolation, got: {result:?}"
        );
    }

    #[test]
    fn test_reconcile_removes_stale_ready_frames() {
        let mut run = minimal_v2_run_running();
        // Frame-1 has empty ready_queue → should NOT be in ready_frames.
        let stale = frame_snapshot_with_ready_queue("frame-1", vec![], false);
        run.frames.insert(crate::FrameId::from("frame-1"), stale);

        // Manually insert frame-1 into ready_frames as a stale entry.
        if let Some(KernelValue::Seq(seq)) = run.flow_state.fields.get_mut("ready_frames") {
            seq.push(KernelValue::String("frame-1".into()));
        }
        if let Some(KernelValue::Set(set)) = run.flow_state.fields.get_mut("ready_frame_membership")
        {
            set.insert(KernelValue::String("frame-1".into()));
        }

        reconcile_run_state(&mut run).expect("reconcile");

        match run.flow_state.fields.get("ready_frames") {
            Some(KernelValue::Seq(seq)) => {
                assert!(
                    !seq.contains(&KernelValue::String("frame-1".into())),
                    "Stale frame-1 should be removed"
                );
            }
            _ => panic!("ready_frames field missing"),
        }
    }

    #[test]
    fn test_reconcile_adds_missing_ready_frames() {
        let mut run = minimal_v2_run_running();
        // Frame-2 has non-empty ready_queue → should be in ready_frames.
        let active = frame_snapshot_with_ready_queue("frame-2", vec!["node-a"], true);
        run.frames.insert(crate::FrameId::from("frame-2"), active);

        // Don't insert frame-2 into ready_frames (simulating missing entry).

        reconcile_run_state(&mut run).expect("reconcile");

        match run.flow_state.fields.get("ready_frames") {
            Some(KernelValue::Seq(seq)) => {
                assert!(
                    seq.contains(&KernelValue::String("frame-2".into())),
                    "Missing frame-2 should be added"
                );
            }
            _ => panic!("ready_frames field missing"),
        }
    }
}
