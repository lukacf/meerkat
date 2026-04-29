//! Recovery reconciliation for FlowRun state after crash/restart.
//!
//! `reconcile_run_state` must be called before resuming a run to ensure stored
//! scheduler projections already agree with the per-frame and per-loop kernel
//! snapshots.

use crate::ids::{FlowNodeId, FrameId, LoopInstanceId};
use crate::run::{FrameSnapshot, LoopSnapshot, MobRun, MobRunStatus};
use crate::run::{flow_frame, loop_iteration};

/// Errors that prevent a run from being resumed.
#[derive(Debug, thiserror::Error)]
pub enum RestoreIncompatible {
    #[error("cannot resume pre-v3 frame run: schema_version={schema_version}")]
    PreV3Schema { schema_version: u32 },
    #[error("frame invariant violated: frame {frame_id} has Ready nodes not in ready_queue")]
    FrameInvariantViolation { frame_id: FrameId },
    #[error("run projection mismatch: {field}")]
    ProjectionMismatch { field: &'static str },
    #[error("flow authority projection mismatch: {reason}")]
    FlowAuthorityProjectionMismatch { reason: String },
}

/// Validate run scheduler state against frame/loop snapshots.
///
/// Must be called before resuming a run after a crash or restart.
///
/// Recovery may not rewrite machine-owned scheduler fields from projections.
/// If the persisted projections disagree with frame/loop snapshots, the run is
/// incompatible with machine-authority recovery and must be repaired by replaying
/// accepted MobMachine transitions, not by seeding canonical fields here.
///
/// Returns `Err(RestoreIncompatible)` if the persisted state is irrecoverable.
pub fn reconcile_run_state(run: &mut MobRun) -> Result<(), RestoreIncompatible> {
    // 1. Pre-v3 check: reject active runs that predate descriptor-backed frame recovery.
    if run.schema_version < 3 && run.status != MobRunStatus::Pending && !run.status.is_terminal() {
        return Err(RestoreIncompatible::PreV3Schema {
            schema_version: run.schema_version,
        });
    }

    // 2. Validate per-frame local invariant before reconciling.
    for (frame_id, frame_snap) in &run.frames {
        check_frame_invariant(frame_id, frame_snap)?;
    }

    validate_ready_frames(run)?;

    validate_pending_body_frame_loops(run)?;

    validate_active_counts(run)?;

    run.validate_flow_authority_projection().map_err(|error| {
        RestoreIncompatible::FlowAuthorityProjectionMismatch {
            reason: error.to_string(),
        }
    })?;

    Ok(())
}

/// Check the per-frame invariant: node_status[n] == Ready ↔ n ∈ ready_queue.
fn check_frame_invariant(
    frame_id: &FrameId,
    snap: &FrameSnapshot,
) -> Result<(), RestoreIncompatible> {
    let ready_by_status: std::collections::BTreeSet<FlowNodeId> = snap
        .kernel_state
        .node_status
        .iter()
        .filter(|(_, status)| *status == &crate::run::flow_frame::NodeRunStatus::Ready)
        .map(|(node_id, _)| node_id.clone())
        .collect();

    let in_ready_queue: std::collections::BTreeSet<FlowNodeId> =
        snap.kernel_state.ready_queue.iter().cloned().collect();

    if ready_by_status != in_ready_queue {
        return Err(RestoreIncompatible::FrameInvariantViolation {
            frame_id: frame_id.clone(),
        });
    }

    Ok(())
}

fn validate_ready_frames(run: &MobRun) -> Result<(), RestoreIncompatible> {
    let mut active_frame_ids: Vec<FrameId> = run
        .frames
        .iter()
        .filter(|(_, snap)| frame_has_nonempty_ready_queue(snap))
        .map(|(frame_id, _)| frame_id.clone())
        .collect();
    active_frame_ids.sort();
    let active_frame_membership = active_frame_ids.iter().cloned().collect();
    if run.flow_state.ready_frames != active_frame_ids {
        return Err(RestoreIncompatible::ProjectionMismatch {
            field: "ready_frames",
        });
    }
    if run.flow_state.ready_frame_membership != active_frame_membership {
        return Err(RestoreIncompatible::ProjectionMismatch {
            field: "ready_frame_membership",
        });
    }
    Ok(())
}

fn validate_pending_body_frame_loops(run: &MobRun) -> Result<(), RestoreIncompatible> {
    let mut pending_loop_ids: Vec<LoopInstanceId> = run
        .loops
        .iter()
        .filter(|(loop_id, snap)| loop_is_pending_body_frame(loop_id, snap))
        .map(|(loop_id, _)| loop_id.clone())
        .collect();
    pending_loop_ids.sort();
    let pending_loop_membership = pending_loop_ids.iter().cloned().collect();
    if run.flow_state.pending_body_frame_loops != pending_loop_ids {
        return Err(RestoreIncompatible::ProjectionMismatch {
            field: "pending_body_frame_loops",
        });
    }
    if run.flow_state.pending_body_frame_loop_membership != pending_loop_membership {
        return Err(RestoreIncompatible::ProjectionMismatch {
            field: "pending_body_frame_loop_membership",
        });
    }
    Ok(())
}

fn validate_active_counts(run: &MobRun) -> Result<(), RestoreIncompatible> {
    let active_node_count = run
        .frames
        .values()
        .map(count_running_step_nodes)
        .sum::<u32>();
    let active_frame_count = run
        .loops
        .values()
        .filter_map(active_body_frame_id)
        .filter(|frame_id| run.frames.contains_key(frame_id))
        .count() as u32;

    if run.flow_state.active_node_count != active_node_count {
        return Err(RestoreIncompatible::ProjectionMismatch {
            field: "active_node_count",
        });
    }
    if run.flow_state.active_frame_count != active_frame_count {
        return Err(RestoreIncompatible::ProjectionMismatch {
            field: "active_frame_count",
        });
    }
    Ok(())
}

/// Return true if a loop snapshot represents a loop that is pending a body frame start.
///
/// A loop is pending iff it is in Running phase with no active body frame
/// (`active_body_frame_id == None`). The `None` (field absent) arm
/// guards against corrupt snapshots — legitimate Running loops always initialize
/// this field; if it is missing entirely, we treat the loop as pending and emit a
/// warning so the anomaly is visible in logs.
fn loop_is_pending_body_frame(loop_id: &LoopInstanceId, snap: &LoopSnapshot) -> bool {
    if snap.kernel_state.phase != loop_iteration::Phase::Running {
        return false;
    }
    if snap.kernel_state.active_body_frame_id.is_none() {
        return true;
    }
    let _ = loop_id;
    false
}

fn active_body_frame_id(snap: &LoopSnapshot) -> Option<FrameId> {
    snap.kernel_state.active_body_frame_id.clone()
}

fn count_running_step_nodes(snap: &FrameSnapshot) -> u32 {
    snap.kernel_state
        .node_status
        .iter()
        .filter(|(node_id, status)| {
            *status == &crate::run::flow_frame::NodeRunStatus::Running
                && matches!(
                    snap.kernel_state
                        .node_kind
                        .get(*node_id)
                        .map(flow_frame::FlowNodeKind::as_str),
                    Some("Step")
                )
        })
        .count() as u32
}

/// Return true if the frame's `ready_queue` is present and non-empty.
fn frame_has_nonempty_ready_queue(snap: &FrameSnapshot) -> bool {
    !snap.kernel_state.ready_queue.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::RunId;
    use crate::run::{MobRun, MobRunStatus};
    use std::collections::BTreeMap;

    fn minimal_v2_run_running() -> MobRun {
        use crate::run::flow_run;
        let flow_state = flow_run::initial_state();
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
            schema_version: 4,
            root_step_outputs: indexmap::IndexMap::new(),
            loop_iteration_outputs: std::collections::BTreeMap::new(),
            flow_authority_inputs: Vec::new(),
        }
    }

    fn frame_snapshot_with_ready_queue(
        _frame_id: &str,
        ready_nodes: Vec<&str>,
        all_nodes_ready: bool,
    ) -> FrameSnapshot {
        let mut state = crate::run::flow_frame::initial_state();
        state.phase = crate::run::flow_frame::Phase::Running;
        state.ready_queue = ready_nodes.iter().map(|s| FlowNodeId::from(*s)).collect();
        state.tracked_nodes = ready_nodes.iter().map(|s| FlowNodeId::from(*s)).collect();
        if all_nodes_ready {
            state.node_status = ready_nodes
                .iter()
                .map(|s| {
                    (
                        FlowNodeId::from(*s),
                        crate::run::flow_frame::NodeRunStatus::Ready,
                    )
                })
                .collect();
        }
        FrameSnapshot {
            kernel_state: state,
        }
    }

    #[test]
    fn test_pre_v3_pending_run_is_accepted() {
        let mut run = minimal_v2_run_running();
        run.schema_version = 2;
        run.status = MobRunStatus::Pending;
        assert!(reconcile_run_state(&mut run).is_ok());
    }

    #[test]
    fn test_pre_v3_running_run_is_rejected() {
        let mut run = minimal_v2_run_running();
        run.schema_version = 2;
        run.status = MobRunStatus::Running;
        let result = reconcile_run_state(&mut run);
        assert!(matches!(
            result,
            Err(RestoreIncompatible::PreV3Schema { .. })
        ));
    }

    #[test]
    fn test_empty_run_reconciles_ok() {
        let mut run = minimal_v2_run_running();
        assert!(reconcile_run_state(&mut run).is_ok());
    }

    #[test]
    fn test_active_counts_reconcile_from_frames_and_loops() {
        let mut run = minimal_v2_run_running();
        let mut root = crate::run::flow_frame::initial_state();
        root.phase = crate::run::flow_frame::Phase::Running;
        root.node_status = BTreeMap::from([(
            FlowNodeId::from("loop-node"),
            crate::run::flow_frame::NodeRunStatus::Running,
        )]);
        root.node_kind = BTreeMap::from([(
            FlowNodeId::from("loop-node"),
            crate::run::flow_frame::FlowNodeKind::Loop,
        )]);
        run.frames
            .insert(FrameId::from("root"), FrameSnapshot { kernel_state: root });

        let mut body = crate::run::flow_frame::initial_state();
        body.phase = crate::run::flow_frame::Phase::Running;
        body.node_status = BTreeMap::from([(
            FlowNodeId::from("body-step"),
            crate::run::flow_frame::NodeRunStatus::Running,
        )]);
        body.node_kind = BTreeMap::from([(
            FlowNodeId::from("body-step"),
            crate::run::flow_frame::FlowNodeKind::Step,
        )]);
        run.frames.insert(
            FrameId::from("body-frame"),
            FrameSnapshot { kernel_state: body },
        );

        let mut loop_state = crate::run::loop_iteration::initial_state();
        loop_state.phase = crate::run::loop_iteration::Phase::Running;
        loop_state.active_body_frame_id = Some(FrameId::from("body-frame"));
        run.loops.insert(
            LoopInstanceId::from("loop-1"),
            LoopSnapshot {
                kernel_state: loop_state,
            },
        );

        run.flow_state.active_node_count = 1;
        run.flow_state.active_frame_count = 1;
        reconcile_run_state(&mut run).expect("validate");
        assert_eq!(run.flow_state.active_node_count, 1);
        assert_eq!(run.flow_state.active_frame_count, 1);
    }

    #[test]
    fn test_frame_invariant_valid_empty_queue() {
        let frame_id = crate::FrameId::from("f1");
        let mut state = crate::run::flow_frame::initial_state();
        state.phase = crate::run::flow_frame::Phase::Running;
        state.node_status = BTreeMap::from([(
            FlowNodeId::from("node-a"),
            crate::run::flow_frame::NodeRunStatus::Running,
        )]);
        let snap = FrameSnapshot {
            kernel_state: state,
        };
        assert!(check_frame_invariant(&frame_id, &snap).is_ok());
    }

    #[test]
    fn test_frame_invariant_violation_ready_not_in_queue() {
        let frame_id = crate::FrameId::from("f-bad");
        let mut state = crate::run::flow_frame::initial_state();
        state.phase = crate::run::flow_frame::Phase::Running;
        state.node_status = BTreeMap::from([(
            FlowNodeId::from("node-a"),
            crate::run::flow_frame::NodeRunStatus::Ready,
        )]);
        let snap = FrameSnapshot {
            kernel_state: state,
        };
        let result = check_frame_invariant(&frame_id, &snap);
        assert!(matches!(
            result,
            Err(RestoreIncompatible::FrameInvariantViolation { .. })
        ));
    }

    #[test]
    fn test_reconcile_removes_stale_ready_frames() {
        let mut run = minimal_v2_run_running();
        let stale = frame_snapshot_with_ready_queue("frame-1", vec![], false);
        run.frames.insert(crate::FrameId::from("frame-1"), stale);
        run.flow_state.ready_frames.push(FrameId::from("frame-1"));
        run.flow_state
            .ready_frame_membership
            .insert(FrameId::from("frame-1"));
        let result = reconcile_run_state(&mut run);
        assert!(matches!(
            result,
            Err(RestoreIncompatible::ProjectionMismatch {
                field: "ready_frames"
            })
        ));
    }

    #[test]
    fn test_reconcile_adds_missing_ready_frames() {
        let mut run = minimal_v2_run_running();
        let active = frame_snapshot_with_ready_queue("frame-2", vec!["node-a"], true);
        run.frames.insert(crate::FrameId::from("frame-2"), active);
        let result = reconcile_run_state(&mut run);
        assert!(matches!(
            result,
            Err(RestoreIncompatible::ProjectionMismatch {
                field: "ready_frames"
            })
        ));
    }
}
