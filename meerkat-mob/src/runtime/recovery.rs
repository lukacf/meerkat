//! Recovery reconciliation for FlowRun state after crash/restart.
//!
//! `reconcile_run_state` must be called before resuming a run to ensure the
//! scheduler's `ready_frames` and `pending_body_frame_loops` fields are
//! consistent with the per-frame and per-loop kernel snapshots.

use crate::flow_machine_types::{frame_id, local_frame_id, loop_instance_id};
use crate::ids::{FrameId, LoopInstanceId};
use crate::run::{FrameSnapshot, LoopSnapshot, MobRun, MobRunStatus};
use meerkat_machine_kernels::compat_generated::loop_iteration;
use meerkat_machine_schema::compat::types as kernel_types;

/// Errors that prevent a run from being resumed.
#[derive(Debug, thiserror::Error)]
pub enum RestoreIncompatible {
    #[error("cannot resume pre-row22 frame run: schema_version={schema_version}")]
    PreRow22Schema { schema_version: u32 },
    #[error("frame invariant violated: frame {frame_id} has Ready nodes not in ready_queue")]
    FrameInvariantViolation { frame_id: FrameId },
    #[error("stale queue entries could not be reconciled")]
    StaleQueueState,
}

/// Reconcile run scheduler state from frame/loop snapshots.
///
/// Must be called before resuming a run after a crash or restart.
///
/// RMAT exception:
/// `ready_frames`, `pending_body_frame_loops`, `active_node_count`, and
/// `active_frame_count` are rebuildable scheduler projections, not canonical
/// semantic truth. Recovery is therefore allowed to overwrite them directly
/// from the authoritative frame/loop snapshots after a crash.
///
/// Returns `Err(RestoreIncompatible)` if the persisted state is irrecoverable.
pub fn reconcile_run_state(run: &mut MobRun) -> Result<(), RestoreIncompatible> {
    // 1. Hard cut: reject active runs that predate the row-22 typed snapshot cut.
    if run.schema_version < 5 && run.status != MobRunStatus::Pending && !run.status.is_terminal() {
        return Err(RestoreIncompatible::PreRow22Schema {
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

    // 5. Reconcile active scheduler counters from persisted frame/loop snapshots.
    reconcile_active_counts(run);

    Ok(())
}

/// Check the per-frame invariant: node_status[n] == Ready ↔ n ∈ ready_queue.
fn check_frame_invariant(
    frame_id: &FrameId,
    snap: &FrameSnapshot,
) -> Result<(), RestoreIncompatible> {
    // Collect the set of node IDs with status == Ready.
    let ready_by_status: std::collections::BTreeSet<String> = snap
        .kernel_state
        .node_status
        .iter()
        .filter_map(|(node_id, status)| {
            if is_ready_variant(status) {
                Some(node_id.to_string())
            } else {
                None
            }
        })
        .collect();

    // Collect the set of node IDs in ready_queue.
    let in_ready_queue: std::collections::BTreeSet<String> = snap
        .kernel_state
        .ready_queue
        .iter()
        .map(ToString::to_string)
        .collect();

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

    run.flow_state.ready_frames = active_frame_ids
        .iter()
        .map(|value| frame_id(&FrameId::from(value.as_str())))
        .collect();
    run.flow_state.ready_frame_membership = active_frame_ids
        .iter()
        .map(|value| frame_id(&FrameId::from(value.as_str())))
        .collect();
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
        .filter(|(loop_id, snap)| loop_is_pending_body_frame(loop_id, snap))
        .map(|(loop_id, _)| loop_id.to_string())
        .collect();
    pending_loop_ids.sort();

    run.flow_state.pending_body_frame_loops = pending_loop_ids
        .iter()
        .map(|value| loop_instance_id(&LoopInstanceId::from(value.as_str())))
        .collect();
    run.flow_state.pending_body_frame_loop_membership = pending_loop_ids
        .iter()
        .map(|value| loop_instance_id(&LoopInstanceId::from(value.as_str())))
        .collect();
}

/// Rebuild the run-level active counters from persisted snapshots.
///
/// `active_node_count` counts running step nodes across all frames. Loop nodes
/// are excluded because once a loop hands off to `LoopIterationMachine`, the
/// run scheduler tracks the active body frame/step work instead of charging the
/// parent loop node indefinitely.
///
/// `active_frame_count` counts body-frame grants that have not yet been released
/// through `FrameTerminated`. A loop with `active_body_frame_id = Some(frame)`
/// still owns that slot even if the body frame snapshot itself is already
/// terminal and recovery must finish the handoff.
fn reconcile_active_counts(run: &mut MobRun) {
    let active_node_count = run
        .frames
        .values()
        .map(count_running_step_nodes)
        .sum::<u64>();
    let active_frame_count = run
        .loops
        .values()
        .filter_map(active_body_frame_id)
        .filter(|frame_id| run.frames.contains_key(frame_id))
        .count() as u64;

    run.flow_state.active_node_count = u32::try_from(active_node_count).unwrap_or(u32::MAX);
    run.flow_state.active_frame_count = u32::try_from(active_frame_count).unwrap_or(u32::MAX);
}

/// Return true if a loop snapshot represents a loop that is pending a body frame start.
///
/// A loop is pending iff it is in Running phase with no active body frame
/// (`active_body_frame_id == KernelValue::None`). The `None` (field absent) arm
/// guards against corrupt snapshots — legitimate Running loops always initialize
/// this field; if it is missing entirely, we treat the loop as pending and emit a
/// warning so the anomaly is visible in logs.
fn loop_is_pending_body_frame(loop_id: &LoopInstanceId, snap: &LoopSnapshot) -> bool {
    if snap.kernel_state.phase != loop_iteration::Phase::Running {
        return false;
    }
    if snap.kernel_state.active_body_frame_id.is_none() {
        if snap.kernel_state.parent_frame_id.as_str().is_empty() {
            tracing::warn!(
                loop_instance_id = %loop_id,
                "loop snapshot is missing active_body_frame_id field; \
                 treating as pending body frame — snapshot may be corrupt"
            );
        }
        true
    } else {
        false
    }
}

fn active_body_frame_id(snap: &LoopSnapshot) -> Option<FrameId> {
    snap.kernel_state
        .active_body_frame_id
        .as_ref()
        .map(local_frame_id)
}

fn count_running_step_nodes(snap: &FrameSnapshot) -> u64 {
    snap.kernel_state
        .node_status
        .iter()
        .filter(|(node_id, status)| {
            **status == kernel_types::NodeRunStatus::Running
                && matches!(
                    snap.kernel_state.node_kind.get(*node_id),
                    Some(kind) if *kind == kernel_types::FlowNodeKind::Step
                )
        })
        .count() as u64
}

/// Return true if the frame's `ready_queue` is present and non-empty.
fn frame_has_nonempty_ready_queue(snap: &FrameSnapshot) -> bool {
    !snap.kernel_state.ready_queue.is_empty()
}

fn is_ready_variant(v: &kernel_types::NodeRunStatus) -> bool {
    *v == kernel_types::NodeRunStatus::Ready
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::RunId;
    use crate::run::{MobRun, MobRunStatus};
    use meerkat_machine_kernels::compat_generated;
    use meerkat_machine_kernels::test_oracle::legacy_generated::flow_run as raw_flow_run;
    use meerkat_machine_kernels::test_oracle::{KernelState, KernelValue};
    use std::collections::BTreeMap;

    fn flow_run_state_from_raw(mut state: KernelState) -> compat_generated::flow_run::State {
        let mut seeded =
            meerkat_machine_kernels::test_oracle::legacy_generated::flow_run::initial_state()
                .expect("raw flow_run init");
        seeded.phase = state.phase;
        seeded.fields.extend(state.fields);
        state = seeded;
        compat_generated::flow_run::from_test_oracle_state(state).expect("typed flow_run state")
    }

    fn flow_run_state_to_raw(state: &compat_generated::flow_run::State) -> KernelState {
        compat_generated::flow_run::to_test_oracle_state(state)
    }

    fn flow_frame_state_from_raw(mut state: KernelState) -> compat_generated::flow_frame::State {
        let mut seeded =
            meerkat_machine_kernels::test_oracle::legacy_generated::flow_frame::initial_state()
                .expect("raw flow_frame init");
        seeded.phase = state.phase;
        seeded.fields.extend(state.fields);
        state = seeded;
        compat_generated::flow_frame::from_test_oracle_state(state).expect("typed flow_frame state")
    }

    fn loop_iteration_state_from_raw(
        mut state: KernelState,
    ) -> compat_generated::loop_iteration::State {
        let mut seeded =
            meerkat_machine_kernels::test_oracle::legacy_generated::loop_iteration::initial_state()
                .expect("raw loop_iteration init");
        seeded.phase = state.phase;
        seeded.fields.extend(state.fields);
        state = seeded;
        compat_generated::loop_iteration::from_test_oracle_state(state)
            .expect("typed loop_iteration state")
    }

    fn minimal_v2_run_running() -> MobRun {
        let flow_state = flow_run_state_from_raw(raw_flow_run::initial_state().expect("init"));
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
            schema_version: 5,
            root_step_outputs: indexmap::IndexMap::new(),
            loop_iteration_outputs: std::collections::BTreeMap::new(),
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
            kernel_state: flow_frame_state_from_raw(KernelState {
                phase: "Running".into(),
                fields: BTreeMap::from([
                    ("ready_queue".into(), ready_queue),
                    ("tracked_nodes".into(), KernelValue::Set(tracked)),
                    ("node_status".into(), node_status),
                ]),
            }),
        }
    }

    #[test]
    fn test_pre_row22_pending_run_is_accepted() {
        // Pending runs (not active) are fine even before schema_version 5.
        let mut run = minimal_v2_run_running();
        run.schema_version = 4;
        run.status = MobRunStatus::Pending;
        assert!(reconcile_run_state(&mut run).is_ok());
    }

    #[test]
    fn test_pre_row22_running_run_is_rejected() {
        let mut run = minimal_v2_run_running();
        run.schema_version = 4;
        run.status = MobRunStatus::Running;
        let result = reconcile_run_state(&mut run);
        assert!(
            matches!(result, Err(RestoreIncompatible::PreRow22Schema { .. })),
            "Expected PreRow22Schema, got: {result:?}"
        );
    }

    #[test]
    fn test_empty_run_reconciles_ok() {
        let mut run = minimal_v2_run_running();
        assert!(reconcile_run_state(&mut run).is_ok());
    }

    #[test]
    fn test_active_counts_reconcile_from_frames_and_loops() {
        let mut run = minimal_v2_run_running();
        run.frames.insert(
            FrameId::from("root"),
            FrameSnapshot {
                kernel_state: flow_frame_state_from_raw(KernelState {
                    phase: "Running".into(),
                    fields: BTreeMap::from([
                        (
                            "node_status".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("loop-node".into()),
                                KernelValue::NamedVariant {
                                    enum_name: "NodeRunStatus".into(),
                                    variant: "Running".into(),
                                },
                            )])),
                        ),
                        (
                            "node_kind".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("loop-node".into()),
                                KernelValue::NamedVariant {
                                    enum_name: "FlowNodeKind".into(),
                                    variant: "Loop".into(),
                                },
                            )])),
                        ),
                        ("ready_queue".into(), KernelValue::Seq(vec![])),
                    ]),
                }),
            },
        );
        run.frames.insert(
            FrameId::from("body-frame"),
            FrameSnapshot {
                kernel_state: flow_frame_state_from_raw(KernelState {
                    phase: "Running".into(),
                    fields: BTreeMap::from([
                        (
                            "node_status".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("body-step".into()),
                                KernelValue::NamedVariant {
                                    enum_name: "NodeRunStatus".into(),
                                    variant: "Running".into(),
                                },
                            )])),
                        ),
                        (
                            "node_kind".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("body-step".into()),
                                KernelValue::NamedVariant {
                                    enum_name: "FlowNodeKind".into(),
                                    variant: "Step".into(),
                                },
                            )])),
                        ),
                        ("ready_queue".into(), KernelValue::Seq(vec![])),
                    ]),
                }),
            },
        );
        run.loops.insert(
            LoopInstanceId::from("loop-1"),
            LoopSnapshot {
                kernel_state: loop_iteration_state_from_raw(KernelState {
                    phase: "Running".into(),
                    fields: BTreeMap::from([(
                        "active_body_frame_id".into(),
                        KernelValue::String("body-frame".into()),
                    )]),
                }),
            },
        );

        reconcile_run_state(&mut run).expect("reconcile");

        assert_eq!(run.flow_state.active_node_count, 1);
        assert_eq!(run.flow_state.active_frame_count, 1);
    }

    #[test]
    fn test_frame_invariant_valid_empty_queue() {
        // Frame with empty ready_queue and no Ready nodes is valid.
        let frame_id = crate::FrameId::from("f1");
        let snap = FrameSnapshot {
            kernel_state: flow_frame_state_from_raw(KernelState {
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
            }),
        };
        assert!(check_frame_invariant(&frame_id, &snap).is_ok());
    }

    #[test]
    fn test_frame_invariant_violation_ready_not_in_queue() {
        let frame_id = crate::FrameId::from("f-bad");
        let snap = FrameSnapshot {
            kernel_state: flow_frame_state_from_raw(KernelState {
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
            }),
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
        let mut raw = flow_run_state_to_raw(&run.flow_state);
        if let Some(KernelValue::Seq(seq)) = raw.fields.get_mut("ready_frames") {
            seq.push(KernelValue::String("frame-1".into()));
        }
        if let Some(KernelValue::Set(set)) = raw.fields.get_mut("ready_frame_membership") {
            set.insert(KernelValue::String("frame-1".into()));
        }
        run.flow_state = flow_run_state_from_raw(raw);

        reconcile_run_state(&mut run).expect("reconcile");

        let raw = flow_run_state_to_raw(&run.flow_state);
        match raw.fields.get("ready_frames") {
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

        let raw = flow_run_state_to_raw(&run.flow_state);
        match raw.fields.get("ready_frames") {
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
